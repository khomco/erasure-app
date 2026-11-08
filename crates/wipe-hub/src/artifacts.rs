//! Where release files live (software-downloads story 1, CAP-4, ADR-0007
//! §5): the [`ArtifactStore`] seam and its local-directory implementation,
//! [`LocalDirStore`]. Object storage can replace it later without code
//! changes elsewhere.
//!
//! - **Keys are artifact ids**: the canonical hyphenated form of a UUID.
//!   [`LocalDirStore`] refuses any other key, so no key can name a path
//!   outside its directory.
//! - **The hub computes size and SHA-256 itself**, in [`store_artifact`],
//!   while the bytes stream into whichever store is configured; nothing a
//!   client claims about a file is used. A file over the limit or an empty
//!   one is refused and nothing is kept.
//! - **A reader never sees a partial file**: [`LocalDirStore::put`] writes to
//!   a temporary file in the same directory, fsyncs it, then renames it into
//!   place.

use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};
use uuid::Uuid;

/// The largest artifact the hub accepts: 1 GiB.
pub const ARTIFACT_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// A reader a store can consume.
pub type ArtifactReader<'a> = dyn AsyncRead + Unpin + Send + 'a;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The key is not an artifact id.
    #[error("artifact key {0:?} is not a canonical UUID")]
    BadKey(String),
    /// No file is stored under the key.
    #[error("no artifact is stored under this key")]
    NotFound,
    /// The file is over the limit; nothing was kept.
    #[error("the file is larger than {0} bytes")]
    TooLarge(u64),
    /// The file is empty; nothing was kept.
    #[error("the file is empty")]
    Empty,
    /// Reading the incoming bytes failed (a client hung up mid-upload, a
    /// local file could not be read); nothing was kept.
    #[error("the file could not be read: {0}")]
    Source(String),
    /// The store itself failed (disk full, permission denied).
    #[error("artifact storage failed: {0}")]
    Io(#[from] io::Error),
}

/// Where release files are kept. Every implementation gets the same size
/// limit and checksum, computed by [`store_artifact`], never by the store.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Store everything `reader` yields under `key`, replacing nothing a
    /// reader could see half-written. On error nothing is kept under `key`.
    async fn put(&self, key: &str, reader: &mut ArtifactReader<'_>) -> Result<(), StoreError>;

    /// The bytes stored under `key`, or [`StoreError::NotFound`].
    async fn open(&self, key: &str) -> Result<Box<ArtifactReader<'static>>, StoreError>;

    /// Remove what is stored under `key`. A missing key is not an error.
    async fn delete(&self, key: &str) -> Result<(), StoreError>;
}

/// What [`store_artifact`] measured while storing a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtifact {
    pub size_bytes: u64,
    /// Lowercase hex SHA-256 of the stored bytes.
    pub sha256: String,
}

/// Stream `reader` into `store` under `key`, hashing and counting each chunk
/// on the way. More than `limit` bytes is [`StoreError::TooLarge`] and no
/// bytes is [`StoreError::Empty`]; on any error nothing is left under `key`.
pub async fn store_artifact<R>(
    store: &dyn ArtifactStore,
    key: &str,
    reader: R,
    limit: u64,
) -> Result<StoredArtifact, StoreError>
where
    R: AsyncRead + Unpin + Send,
{
    let mut measured = Measured {
        inner: reader,
        hasher: Sha256::new(),
        size: 0,
        limit,
        failure: None,
    };
    if let Err(e) = store.put(key, &mut measured).await {
        let _ = store.delete(key).await;
        return Err(match measured.failure.take() {
            Some(Failure::TooLarge) => StoreError::TooLarge(limit),
            Some(Failure::Source(m)) => StoreError::Source(m),
            None => e,
        });
    }
    if measured.size == 0 {
        store.delete(key).await?;
        return Err(StoreError::Empty);
    }
    Ok(StoredArtifact {
        size_bytes: measured.size,
        sha256: hex::encode(measured.hasher.finalize()),
    })
}

enum Failure {
    TooLarge,
    Source(String),
}

/// A reader that hashes and counts what passes through, and fails once
/// more than `limit` bytes have come.
struct Measured<R> {
    inner: R,
    hasher: Sha256,
    size: u64,
    limit: u64,
    failure: Option<Failure>,
}

impl<R: AsyncRead + Unpin> AsyncRead for Measured<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.failure.is_some() {
            return Poll::Ready(Err(io::Error::other("artifact refused")));
        }
        let before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let chunk = &buf.filled()[before..];
                let size = this.size + chunk.len() as u64;
                if size > this.limit {
                    this.failure = Some(Failure::TooLarge);
                    // Hand nothing of the over-limit chunk to the store.
                    buf.set_filled(before);
                    return Poll::Ready(Err(io::Error::other("artifact over the size limit")));
                }
                this.hasher.update(chunk);
                this.size = size;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => {
                this.failure = Some(Failure::Source(e.to_string()));
                Poll::Ready(Err(e))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// The canonical form of an artifact key, or [`StoreError::BadKey`].
pub fn check_key(key: &str) -> Result<Uuid, StoreError> {
    match Uuid::parse_str(key) {
        Ok(id) if id.hyphenated().to_string() == key => Ok(id),
        _ => Err(StoreError::BadKey(key.to_string())),
    }
}

/// The hub's artifact store for `dir` (`--artifact-dir`): the one place
/// that picks an implementation, used by `serve` and the CLI alike.
pub fn store_for(dir: impl Into<PathBuf>) -> std::sync::Arc<dyn ArtifactStore> {
    std::sync::Arc::new(LocalDirStore::new(dir))
}

/// Files in one local directory, named by key.
#[derive(Debug, Clone)]
pub struct LocalDirStore {
    dir: PathBuf,
}

impl LocalDirStore {
    /// A store in `dir`, which must already exist (the configuration
    /// creates it at start-up).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_of(&self, key: &str) -> Result<PathBuf, StoreError> {
        check_key(key)?;
        Ok(self.dir.join(key))
    }
}

#[async_trait]
impl ArtifactStore for LocalDirStore {
    async fn put(&self, key: &str, reader: &mut ArtifactReader<'_>) -> Result<(), StoreError> {
        let path = self.path_of(key)?;
        // A dot-file in the same directory, so the rename is atomic and an
        // interrupted upload never looks like an artifact.
        let tmp = self
            .dir
            .join(format!(".{key}.{}.part", Uuid::new_v4().simple()));
        let result = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .await?;
            tokio::io::copy(reader, &mut file).await?;
            file.flush().await?;
            file.sync_all().await?;
            drop(file);
            tokio::fs::rename(&tmp, &path).await?;
            sync_dir(&self.dir).await;
            Ok::<_, io::Error>(())
        }
        .await;
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(StoreError::Io(e));
        }
        Ok(())
    }

    async fn open(&self, key: &str) -> Result<Box<ArtifactReader<'static>>, StoreError> {
        let path = self.path_of(key)?;
        match tokio::fs::File::open(&path).await {
            Ok(f) => Ok(Box::new(f)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StoreError::NotFound),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        let path = self.path_of(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io(e)),
        }
    }
}

/// Persist the rename itself; best effort (not every platform can fsync a
/// directory).
async fn sync_dir(dir: &Path) {
    let dir = dir.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(d) = std::fs::File::open(&dir) {
            let _ = d.sync_all();
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    async fn read_all(store: &LocalDirStore, key: &str) -> Vec<u8> {
        let mut r = store.open(key).await.unwrap();
        let mut out = Vec::new();
        r.read_to_end(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn put_open_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalDirStore::new(dir.path());
        let key = Uuid::new_v4().to_string();
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let stored = store_artifact(&store, &key, &bytes[..], ARTIFACT_MAX_BYTES)
            .await
            .unwrap();
        assert_eq!(stored.size_bytes, bytes.len() as u64);
        assert_eq!(stored.sha256, sha256_hex(&bytes));
        assert_eq!(read_all(&store, &key).await, bytes);
        assert_eq!(entries(dir.path()), vec![key.clone()]);
        store.delete(&key).await.unwrap();
        assert!(entries(dir.path()).is_empty());
        assert!(matches!(store.open(&key).await, Err(StoreError::NotFound)));
        // Deleting a missing key is not an error.
        store.delete(&key).await.unwrap();
    }

    #[tokio::test]
    async fn the_known_checksum_of_abc() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalDirStore::new(dir.path());
        let key = Uuid::new_v4().to_string();
        let stored = store_artifact(&store, &key, &b"abc"[..], 3).await.unwrap();
        assert_eq!(
            stored.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(stored.size_bytes, 3);
    }

    #[tokio::test]
    async fn keys_that_are_not_uuids_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalDirStore::new(dir.path());
        let upper = Uuid::new_v4().to_string().to_uppercase();
        let simple = Uuid::new_v4().simple().to_string();
        for key in ["../x", "a/b", "", ".", "..", "/etc/passwd", &upper, &simple] {
            assert!(
                matches!(
                    store.put(key, &mut &b"x"[..]).await,
                    Err(StoreError::BadKey(_))
                ),
                "{key}"
            );
            assert!(matches!(store.open(key).await, Err(StoreError::BadKey(_))));
            assert!(matches!(
                store.delete(key).await,
                Err(StoreError::BadKey(_))
            ));
        }
        assert!(entries(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn over_the_limit_keeps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalDirStore::new(dir.path());
        let key = Uuid::new_v4().to_string();
        let err = store_artifact(&store, &key, &[7u8; 101][..], 100)
            .await
            .unwrap_err();
        assert!(matches!(err, StoreError::TooLarge(100)), "{err}");
        assert!(entries(dir.path()).is_empty());
        // Exactly the limit is fine.
        let stored = store_artifact(&store, &key, &[7u8; 100][..], 100)
            .await
            .unwrap();
        assert_eq!(stored.size_bytes, 100);
    }

    #[tokio::test]
    async fn an_empty_file_keeps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalDirStore::new(dir.path());
        let key = Uuid::new_v4().to_string();
        let err = store_artifact(&store, &key, &b""[..], 100)
            .await
            .unwrap_err();
        assert!(matches!(err, StoreError::Empty), "{err}");
        assert!(entries(dir.path()).is_empty());
    }

    /// Yields some bytes, then fails, like a client hanging up.
    struct Broken(bool);

    impl AsyncRead for Broken {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.0 {
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::ConnectionReset, "gone")));
            }
            self.0 = true;
            buf.put_slice(b"partial");
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_failing_source_keeps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalDirStore::new(dir.path());
        let key = Uuid::new_v4().to_string();
        let err = store_artifact(&store, &key, Broken(false), 100)
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::Source(ref m) if m.contains("gone")),
            "{err}"
        );
        assert!(entries(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn a_missing_directory_is_a_storage_failure() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalDirStore::new(dir.path().join("absent"));
        let key = Uuid::new_v4().to_string();
        let err = store_artifact(&store, &key, &b"abc"[..], 100)
            .await
            .unwrap_err();
        assert!(matches!(err, StoreError::Io(_)), "{err}");
    }
}
