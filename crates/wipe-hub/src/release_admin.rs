//! Releases (software-downloads story 1, CAP-1 and CAP-5, ADR-0007 §5): the
//! station software the vendor publishes, one file (artifact) per platform.
//! Vendor admins manage them through the routes in [`crate::admin_routes`];
//! `wipe-hub release publish` ([`publish_from_cli`]) does create, upload and
//! publish in one step from the command line. Customers' listing and
//! downloads are story 2.
//!
//! - **Lifecycle** `draft → published → withdrawn`, plus `draft →
//!   withdrawn`; `withdrawn` is terminal and nothing is ever deleted. A
//!   withdrawn release keeps its files and stays listed to admins.
//! - **Files change only while a release is a draft**, and publishing needs
//!   at least one, so a checksum a customer saw never changes.
//! - **A version names one set of files forever**: SemVer 2.0.0, unique
//!   across every release, withdrawn ones included.
//! - **The hub computes every checksum** while the bytes stream into the
//!   [`ArtifactStore`] ([`store_artifact`]); a client never supplies one.
//! - **Uploads store first, then commit**: the file is streamed to the store
//!   before any transaction opens (no row lock is held during a long
//!   upload); one transaction then locks the release, re-checks it is a
//!   draft and the platform is free, inserts the row and audits. Any refusal
//!   or failure after the store write deletes the stored file.
//! - **Every change is audited in its transaction**, subject
//!   `release:<id>`: `release.created {version, channel}`,
//!   `release.artifact_added` / `release.artifact_removed {artifact_id,
//!   platform, filename, size_bytes, sha256}`, `release.published
//!   {artifact_ids}`, `release.withdrawn {previous_status}`. Admin reads are
//!   audited through [`audit_read`] as `admin.releases_listed {release_ids}`
//!   and `admin.release_viewed {}`. The actor is `user:<admin id>`, or
//!   [`CLI_ACTOR`] with `{os_user}` added to the detail. A refused request,
//!   or an unknown id, writes nothing.
//!
//! Migration 0017 repeats every lifecycle rule as a CHECK or trigger.
//! Timestamps are RFC 3339 UTC strings formatted in SQL.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::postgres::{PgConnection, PgPool};
use sqlx::Connection;
use tokio::io::AsyncRead;
use uuid::Uuid;

use crate::admin::{audit_read, os_user, VendorAdmin, CONNECT_TIMEOUT, RFC3339_UTC};
use crate::artifacts::{
    store_artifact, store_for, ArtifactStore, StoreError, StoredArtifact, ARTIFACT_MAX_BYTES,
};
use crate::audit::{self, AuditEntry, AuditError};
use crate::auth::actor;
use crate::config::{resolve_artifact_dir, ReleasePublishArgs};

/// The audit actor of `wipe-hub release publish`.
pub const CLI_ACTOR: &str = "cli:release-publish";

/// Longest version, in characters.
pub const VERSION_MAX_CHARS: usize = 64;
/// Longest release notes, in characters, after trimming.
pub const NOTES_MAX_CHARS: usize = 10_000;
/// Longest platform label.
pub const PLATFORM_MAX_CHARS: usize = 64;
/// Longest file name.
pub const FILENAME_MAX_CHARS: usize = 128;

pub const CHANNELS: [&str; 2] = ["stable", "preview"];

pub const MSG_NO_RELEASE: &str = "no such release";
pub const MSG_NO_ARTIFACT: &str = "no such artifact in this release";
pub const MSG_VERSION_TAKEN: &str =
    "a release with this version already exists (withdrawn releases keep their version)";
pub const MSG_PLATFORM_TAKEN: &str =
    "this release already has a file for this platform; remove it first";
pub const MSG_RELEASE_NOT_DRAFT: &str =
    "files can be added to or removed from a draft release only";
pub const MSG_PUBLISH_NOT_DRAFT: &str = "only a draft release can be published";
pub const MSG_NO_ARTIFACTS: &str = "a release needs at least one file before it can be published";
pub const MSG_ALREADY_WITHDRAWN: &str = "this release is already withdrawn";
pub const MSG_ARTIFACT_TOO_LARGE: &str = "the file is larger than the hub's upload limit (1 GiB)";
pub const MSG_EMPTY_ARTIFACT: &str = "the file is empty";
pub const MSG_UPLOAD_INTERRUPTED: &str = "the upload was interrupted before the whole file arrived";
pub const MSG_STORAGE_UNAVAILABLE: &str = "file storage is unavailable; try again later";

pub const MSG_BAD_PLATFORM: &str = "platform must be 1 to 64 characters of lowercase letters, \
     digits, '.', '_' or '-', starting with a letter or digit (e.g. linux-x86_64)";
pub const MSG_BAD_FILENAME: &str = "filename must be 1 to 128 characters of letters, digits, \
     '.', '_', '+' or '-', not starting with '.'";

/// The unique index on `version` (migration 0017).
const VERSION_KEY: &str = "releases_version_key";
/// The unique index on `(release_id, platform)` (migration 0017).
const PLATFORM_KEY: &str = "release_artifacts_platform_key";

/// Audit subject of one release.
pub fn subject(id: Uuid) -> String {
    format!("release:{id}")
}

// ---------------------------------------------------------------------------
// Requests and validation

/// A create body. Unknown keys are refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRequest {
    pub version: String,
    pub notes: String,
    pub channel: String,
}

/// A validated [`ReleaseRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRelease {
    pub version: String,
    pub notes: String,
    pub channel: String,
}

/// A SemVer 2.0.0 version without build metadata, of at most
/// [`VERSION_MAX_CHARS`], trimmed.
pub fn validate_version(v: &str) -> Result<String, String> {
    let v = v.trim();
    if v.is_empty() {
        return Err("version must not be blank".into());
    }
    if v.chars().count() > VERSION_MAX_CHARS {
        return Err(format!(
            "version must be at most {VERSION_MAX_CHARS} characters"
        ));
    }
    let parsed = semver::Version::parse(v)
        .map_err(|e| format!("version must be a SemVer version such as 1.2.3: {e}"))?;
    // SemVer ignores build metadata when ranking versions, so `1.0.0` and
    // `1.0.0+build.5` would both be "latest".
    if !parsed.build.is_empty() {
        return Err("version must not carry build metadata (+…)".into());
    }
    Ok(v.to_string())
}

/// Trimmed, non-blank notes of at most [`NOTES_MAX_CHARS`] with no control
/// characters but newline and tab.
pub fn validate_notes(n: &str) -> Result<String, String> {
    let n = n.trim();
    if n.is_empty() {
        return Err("notes must not be blank".into());
    }
    if n.chars().count() > NOTES_MAX_CHARS {
        return Err(format!(
            "notes must be at most {NOTES_MAX_CHARS} characters"
        ));
    }
    if n.chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err("notes must not contain control characters other than newline and tab".into());
    }
    Ok(n.to_string())
}

/// One of [`CHANNELS`].
pub fn validate_channel(c: &str) -> Result<String, String> {
    if CHANNELS.contains(&c) {
        Ok(c.to_string())
    } else {
        Err(format!("channel must be one of {}", CHANNELS.join(", ")))
    }
}

/// Validate a body; the error is the 400 message, naming the field.
pub fn validate(req: &ReleaseRequest) -> Result<NewRelease, String> {
    Ok(NewRelease {
        version: validate_version(&req.version)?,
        notes: validate_notes(&req.notes)?,
        channel: validate_channel(&req.channel)?,
    })
}

/// `^[a-z0-9][a-z0-9._-]{0,63}$`, or [`MSG_BAD_PLATFORM`].
pub fn validate_platform(p: &str) -> Result<String, String> {
    let ok = !p.is_empty()
        && p.len() <= PLATFORM_MAX_CHARS
        && p.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || (i > 0 && matches!(b, b'.' | b'_' | b'-'))
        });
    if ok {
        Ok(p.to_string())
    } else {
        Err(MSG_BAD_PLATFORM.into())
    }
}

/// `^[A-Za-z0-9_+-][A-Za-z0-9._+-]{0,127}$`, or [`MSG_BAD_FILENAME`].
pub fn validate_filename(f: &str) -> Result<String, String> {
    let ok = !f.is_empty()
        && f.len() <= FILENAME_MAX_CHARS
        && f.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-') || (i > 0 && b == b'.')
        });
    if ok {
        Ok(f.to_string())
    } else {
        Err(MSG_BAD_FILENAME.into())
    }
}

// ---------------------------------------------------------------------------
// Shapes

/// One file of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct ReleaseArtifact {
    pub id: Uuid,
    pub platform: String,
    pub filename: String,
    pub size_bytes: i64,
    /// Lowercase hex SHA-256, computed by the hub.
    pub sha256: String,
    /// RFC 3339, UTC.
    pub created_at: String,
}

/// A release as the admin area shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub id: Uuid,
    pub version: String,
    pub notes: String,
    /// One of [`CHANNELS`].
    pub channel: String,
    /// `draft`, `published` or `withdrawn`.
    pub status: String,
    /// RFC 3339, UTC.
    pub created_at: String,
    pub published_at: Option<String>,
    pub withdrawn_at: Option<String>,
    /// By platform.
    pub artifacts: Vec<ReleaseArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseList {
    /// Newest first, every status.
    pub releases: Vec<Release>,
}

#[derive(sqlx::FromRow)]
struct ReleaseRow {
    id: Uuid,
    version: String,
    notes: String,
    channel: String,
    status: String,
    created_at: String,
    published_at: Option<String>,
    withdrawn_at: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ArtifactRow {
    release_id: Uuid,
    #[sqlx(flatten)]
    artifact: ReleaseArtifact,
}

/// A refusal (404 or 409), a storage failure, or a database failure (500).
#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    /// The 404 message.
    #[error("{0}")]
    NotFound(&'static str),
    /// The 409 message.
    #[error("{0}")]
    Conflict(&'static str),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Db(sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

impl From<sqlx::Error> for ReleaseError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            sqlx::Error::Database(d) if d.constraint() == Some(VERSION_KEY) => {
                Self::Conflict(MSG_VERSION_TAKEN)
            }
            sqlx::Error::Database(d) if d.constraint() == Some(PLATFORM_KEY) => {
                Self::Conflict(MSG_PLATFORM_TAKEN)
            }
            _ => Self::Db(e),
        }
    }
}

/// Who is acting, for `created_by` and the audit row.
#[derive(Debug, Clone)]
enum Actor {
    Admin(VendorAdmin),
    Cli { os_user: Option<String> },
}

impl Actor {
    fn name(&self) -> String {
        match self {
            Actor::Admin(a) => actor(a.user_id()),
            Actor::Cli { .. } => CLI_ACTOR.to_string(),
        }
    }

    fn created_by(&self) -> Option<Uuid> {
        match self {
            Actor::Admin(a) => Some(a.user_id()),
            Actor::Cli { .. } => None,
        }
    }

    async fn record(
        &self,
        conn: &mut PgConnection,
        action: &str,
        id: Uuid,
        mut detail: Value,
    ) -> Result<(), AuditError> {
        if let Actor::Cli { os_user } = self {
            detail["os_user"] = json!(os_user);
        }
        audit::record(
            &mut *conn,
            &AuditEntry {
                actor: &self.name(),
                action,
                subject: &subject(id),
                detail,
            },
        )
        .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Queries

fn select() -> String {
    format!(
        "SELECT id, version, notes, channel, status, \
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS created_at, \
                to_char(published_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS published_at, \
                to_char(withdrawn_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS withdrawn_at \
         FROM releases"
    )
}

/// The rows with their artifacts, in the rows' order.
async fn with_artifacts(
    conn: &mut PgConnection,
    rows: Vec<ReleaseRow>,
) -> Result<Vec<Release>, sqlx::Error> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let artifacts: Vec<ArtifactRow> = sqlx::query_as(&format!(
        "SELECT release_id, id, platform, filename, size_bytes, sha256, \
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS created_at \
         FROM release_artifacts WHERE release_id = ANY($1) ORDER BY platform, id"
    ))
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Release {
            artifacts: artifacts
                .iter()
                .filter(|a| a.release_id == r.id)
                .map(|a| a.artifact.clone())
                .collect(),
            id: r.id,
            version: r.version,
            notes: r.notes,
            channel: r.channel,
            status: r.status,
            created_at: r.created_at,
            published_at: r.published_at,
            withdrawn_at: r.withdrawn_at,
        })
        .collect())
}

async fn by_id(conn: &mut PgConnection, id: Uuid) -> Result<Option<Release>, sqlx::Error> {
    let row: Option<ReleaseRow> = sqlx::query_as(&format!("{} WHERE id = $1", select()))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    match row {
        Some(r) => Ok(with_artifacts(conn, vec![r]).await?.pop()),
        None => Ok(None),
    }
}

async fn fetch(conn: &mut PgConnection, id: Uuid) -> Result<Release, ReleaseError> {
    by_id(conn, id)
        .await?
        .ok_or(ReleaseError::NotFound(MSG_NO_RELEASE))
}

/// The release's status, locking its row; [`MSG_NO_RELEASE`] when none.
async fn lock_status(conn: &mut PgConnection, id: Uuid) -> Result<String, ReleaseError> {
    sqlx::query_scalar("SELECT status FROM releases WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(ReleaseError::NotFound(MSG_NO_RELEASE))
}

// ---------------------------------------------------------------------------
// Reads

/// Every release, newest first, with its artifacts. Audited as
/// `admin.releases_listed {release_ids}`.
pub async fn list(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
) -> Result<ReleaseList, ReleaseError> {
    let rows: Vec<ReleaseRow> = sqlx::query_as(&format!(
        // Qualified: a bare `created_at` would sort by the formatted text,
        // whole seconds only.
        "{} ORDER BY releases.created_at DESC, releases.id DESC",
        select()
    ))
    .fetch_all(&mut *conn)
    .await?;
    let releases = with_artifacts(conn, rows).await?;
    let ids: Vec<Uuid> = releases.iter().map(|r| r.id).collect();
    audit_read(
        conn,
        admin,
        "admin.releases_listed",
        "search:releases",
        json!({ "release_ids": ids }),
    )
    .await?;
    Ok(ReleaseList { releases })
}

/// One release; [`MSG_NO_RELEASE`] (no audit row) when there is none.
/// Audited as `admin.release_viewed`.
pub async fn detail(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
) -> Result<Release, ReleaseError> {
    let release = fetch(conn, id).await?;
    audit_read(conn, admin, "admin.release_viewed", &subject(id), json!({})).await?;
    Ok(release)
}

// ---------------------------------------------------------------------------
// Changes

async fn create_as(
    conn: &mut PgConnection,
    who: &Actor,
    new: &NewRelease,
) -> Result<Uuid, ReleaseError> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO releases (version, notes, channel, created_by) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(&new.version)
    .bind(&new.notes)
    .bind(&new.channel)
    .bind(who.created_by())
    .fetch_one(&mut *conn)
    .await?;
    who.record(
        conn,
        "release.created",
        id,
        json!({ "version": new.version, "channel": new.channel }),
    )
    .await?;
    Ok(id)
}

/// Create a draft release. A version already used is [`MSG_VERSION_TAKEN`].
pub async fn create(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    new: &NewRelease,
) -> Result<Release, ReleaseError> {
    let id = create_as(conn, &Actor::Admin(*admin), new).await?;
    fetch(conn, id).await
}

/// A stored file to record against a release.
#[derive(Debug, Clone)]
struct NewArtifact {
    id: Uuid,
    platform: String,
    filename: String,
    stored: StoredArtifact,
}

fn artifact_detail(a: &ReleaseArtifact) -> Value {
    json!({
        "artifact_id": a.id,
        "platform": a.platform,
        "filename": a.filename,
        "size_bytes": a.size_bytes,
        "sha256": a.sha256,
    })
}

/// Lock the release, re-check it is a draft and the platform is free,
/// insert the row and audit it.
async fn add_artifact_as(
    conn: &mut PgConnection,
    who: &Actor,
    release_id: Uuid,
    new: &NewArtifact,
) -> Result<ReleaseArtifact, ReleaseError> {
    if lock_status(conn, release_id).await? != "draft" {
        return Err(ReleaseError::Conflict(MSG_RELEASE_NOT_DRAFT));
    }
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM release_artifacts WHERE release_id = $1 AND platform = $2)",
    )
    .bind(release_id)
    .bind(&new.platform)
    .fetch_one(&mut *conn)
    .await?;
    if taken {
        return Err(ReleaseError::Conflict(MSG_PLATFORM_TAKEN));
    }
    let size_bytes = i64::try_from(new.stored.size_bytes)
        .map_err(|_| ReleaseError::Store(StoreError::TooLarge(ARTIFACT_MAX_BYTES)))?;
    let artifact: ReleaseArtifact = sqlx::query_as(&format!(
        "INSERT INTO release_artifacts (id, release_id, platform, filename, size_bytes, sha256) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, platform, filename, size_bytes, sha256, \
                   to_char(created_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS created_at"
    ))
    .bind(new.id)
    .bind(release_id)
    .bind(&new.platform)
    .bind(&new.filename)
    .bind(size_bytes)
    .bind(&new.stored.sha256)
    .fetch_one(&mut *conn)
    .await?;
    who.record(
        conn,
        "release.artifact_added",
        release_id,
        artifact_detail(&artifact),
    )
    .await?;
    Ok(artifact)
}

/// Fail early, before an upload is streamed, when the release is missing
/// ([`MSG_NO_RELEASE`]) or not a draft ([`MSG_RELEASE_NOT_DRAFT`]). Not
/// authoritative: [`upload`] checks again under a lock.
async fn check_draft(conn: &mut PgConnection, id: Uuid) -> Result<(), ReleaseError> {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM releases WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    match status.as_deref() {
        None => Err(ReleaseError::NotFound(MSG_NO_RELEASE)),
        Some("draft") => Ok(()),
        Some(_) => Err(ReleaseError::Conflict(MSG_RELEASE_NOT_DRAFT)),
    }
}

/// Delete `key` from the store after a failed write; a failure here only
/// leaves an orphan file, so it is logged, not returned.
async fn discard(store: &dyn ArtifactStore, key: &str) {
    if let Err(e) = store.delete(key).await {
        tracing::warn!(key, error = %e, "could not delete an artifact file after a failed write");
    }
}

/// Add a file to a draft release (the upload route): check the release,
/// stream `reader` into `store` (at most `limit` bytes, hashed on the way),
/// then record it in one transaction. Audited as `release.artifact_added`.
/// Any error after the file is stored deletes it.
#[allow(clippy::too_many_arguments)]
pub async fn upload<R>(
    pool: &PgPool,
    store: &dyn ArtifactStore,
    limit: u64,
    admin: &VendorAdmin,
    release_id: Uuid,
    platform: &str,
    filename: &str,
    reader: R,
) -> Result<ReleaseArtifact, ReleaseError>
where
    R: AsyncRead + Unpin + Send,
{
    {
        let mut conn = pool.acquire().await?;
        check_draft(&mut conn, release_id).await?;
    }
    let id = Uuid::new_v4();
    let key = id.to_string();
    let stored = store_artifact(store, &key, reader, limit).await?;
    let new = NewArtifact {
        id,
        platform: platform.to_string(),
        filename: filename.to_string(),
        stored,
    };
    let result = async {
        let mut tx = pool.begin().await?;
        let artifact = add_artifact_as(&mut tx, &Actor::Admin(*admin), release_id, &new).await?;
        tx.commit().await?;
        Ok::<_, ReleaseError>(artifact)
    }
    .await;
    if result.is_err() {
        discard(store, &key).await;
    }
    result
}

/// Remove a file from a draft release and then from the store. Unknown
/// release [`MSG_NO_RELEASE`], unknown artifact (or one of another release)
/// [`MSG_NO_ARTIFACT`], not a draft [`MSG_RELEASE_NOT_DRAFT`]. Audited as
/// `release.artifact_removed`.
pub async fn remove_artifact(
    pool: &PgPool,
    store: &dyn ArtifactStore,
    admin: &VendorAdmin,
    release_id: Uuid,
    artifact_id: Uuid,
) -> Result<Release, ReleaseError> {
    let mut tx = pool.begin().await?;
    let status = lock_status(&mut tx, release_id).await?;
    let artifact: Option<ReleaseArtifact> = sqlx::query_as(&format!(
        "SELECT id, platform, filename, size_bytes, sha256, \
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS created_at \
         FROM release_artifacts WHERE id = $1 AND release_id = $2"
    ))
    .bind(artifact_id)
    .bind(release_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(artifact) = artifact else {
        return Err(ReleaseError::NotFound(MSG_NO_ARTIFACT));
    };
    if status != "draft" {
        return Err(ReleaseError::Conflict(MSG_RELEASE_NOT_DRAFT));
    }
    sqlx::query("DELETE FROM release_artifacts WHERE id = $1")
        .bind(artifact_id)
        .execute(&mut *tx)
        .await?;
    Actor::Admin(*admin)
        .record(
            &mut tx,
            "release.artifact_removed",
            release_id,
            artifact_detail(&artifact),
        )
        .await?;
    let release = fetch(&mut tx, release_id).await?;
    tx.commit().await?;
    // After the commit: the row is gone, so the file is no longer offered.
    discard(store, &artifact_id.to_string()).await;
    Ok(release)
}

async fn publish_as(conn: &mut PgConnection, who: &Actor, id: Uuid) -> Result<(), ReleaseError> {
    if lock_status(conn, id).await? != "draft" {
        return Err(ReleaseError::Conflict(MSG_PUBLISH_NOT_DRAFT));
    }
    let artifact_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM release_artifacts WHERE release_id = $1 ORDER BY platform, id",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    if artifact_ids.is_empty() {
        return Err(ReleaseError::Conflict(MSG_NO_ARTIFACTS));
    }
    sqlx::query("UPDATE releases SET status = 'published', published_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    who.record(
        conn,
        "release.published",
        id,
        json!({ "artifact_ids": artifact_ids }),
    )
    .await?;
    Ok(())
}

/// Publish a draft with at least one file ([`MSG_PUBLISH_NOT_DRAFT`],
/// [`MSG_NO_ARTIFACTS`]); its files freeze. Audited as `release.published`.
pub async fn publish(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
) -> Result<Release, ReleaseError> {
    publish_as(conn, &Actor::Admin(*admin), id).await?;
    fetch(conn, id).await
}

/// Withdraw a draft or published release, for good; its files are kept and
/// it stays listed to admins. A second time is [`MSG_ALREADY_WITHDRAWN`].
/// Audited as `release.withdrawn {previous_status}`.
pub async fn withdraw(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
) -> Result<Release, ReleaseError> {
    let previous = lock_status(conn, id).await?;
    if previous == "withdrawn" {
        return Err(ReleaseError::Conflict(MSG_ALREADY_WITHDRAWN));
    }
    sqlx::query("UPDATE releases SET status = 'withdrawn', withdrawn_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Actor::Admin(*admin)
        .record(
            conn,
            "release.withdrawn",
            id,
            json!({ "previous_status": previous }),
        )
        .await?;
    fetch(conn, id).await
}

// ---------------------------------------------------------------------------
// `wipe-hub release publish`

/// One `--file <platform>=<path>`, checked.
#[derive(Debug, Clone)]
struct CliFile {
    spec: String,
    platform: String,
    filename: String,
    path: PathBuf,
}

/// Parse and check every `--file`: the platform label, a regular non-empty
/// file within the size limit whose name is a valid file name, and no
/// platform twice.
fn cli_files(specs: &[String], limit: u64) -> anyhow::Result<Vec<CliFile>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for spec in specs {
        let (platform, path) = spec
            .split_once('=')
            .ok_or_else(|| anyhow!("--file {spec:?}: expected <platform>=<path>"))?;
        let platform = validate_platform(platform).map_err(|e| anyhow!("--file {spec:?}: {e}"))?;
        if !seen.insert(platform.clone()) {
            bail!("--file {spec:?}: platform {platform} is given more than once");
        }
        let path = PathBuf::from(path);
        let meta = std::fs::metadata(&path)
            .map_err(|e| anyhow!("--file {spec:?}: cannot read {}: {e}", path.display()))?;
        if !meta.is_file() {
            bail!("--file {spec:?}: {} is not a regular file", path.display());
        }
        if meta.len() == 0 {
            bail!("--file {spec:?}: {} is empty", path.display());
        }
        if meta.len() > limit {
            bail!(
                "--file {spec:?}: {} is {} bytes, over the {limit}-byte limit",
                path.display(),
                meta.len()
            );
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("--file {spec:?}: the path has no usable file name"))?;
        let filename = validate_filename(name).map_err(|e| anyhow!("--file {spec:?}: {e}"))?;
        out.push(CliFile {
            spec: spec.clone(),
            platform,
            filename,
            path,
        });
    }
    Ok(out)
}

async fn discard_all(store: &dyn ArtifactStore, keys: &[Uuid]) {
    for k in keys {
        discard(store, &k.to_string()).await;
    }
}

/// `wipe-hub release publish`: validate every flag and file, apply
/// migrations, store each file, then create, add and publish in one
/// transaction audited as [`CLI_ACTOR`]. Any failure commits nothing and
/// deletes the files it stored. Returns the published release.
pub async fn publish_from_cli(args: ReleasePublishArgs) -> anyhow::Result<Release> {
    let new = NewRelease {
        version: validate_version(&args.version).map_err(|e| anyhow!("--version: {e}"))?,
        notes: validate_notes(&args.notes).map_err(|e| anyhow!("--notes: {e}"))?,
        channel: validate_channel(&args.channel).map_err(|e| anyhow!("--channel: {e}"))?,
    };
    let limit = ARTIFACT_MAX_BYTES;
    let files = cli_files(&args.files, limit)?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dir = resolve_artifact_dir(&args.artifact_dir, &cwd)?;
    let store = store_for(dir);

    crate::db::migrate_url(&args.database_url).await?;
    let redacted = crate::db::redacted(&args.database_url);
    let mut conn = match tokio::time::timeout(
        CONNECT_TIMEOUT,
        PgConnection::connect(&args.database_url),
    )
    .await
    {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => bail!("cannot connect to {redacted}: {e}"),
        Err(_) => bail!(
            "cannot connect to {redacted}: no connection within {}s",
            CONNECT_TIMEOUT.as_secs()
        ),
    };

    let result = publish_files(&mut conn, store.as_ref(), &new, &files, limit).await;
    let _ = conn.close().await;
    result
}

async fn publish_files(
    conn: &mut PgConnection,
    store: &dyn ArtifactStore,
    new: &NewRelease,
    files: &[CliFile],
    limit: u64,
) -> anyhow::Result<Release> {
    // Cheap early refusal before any file is copied; the unique index
    // decides in the transaction below.
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM releases WHERE version = $1)")
            .bind(&new.version)
            .fetch_one(&mut *conn)
            .await?;
    if taken {
        bail!("--version {}: {MSG_VERSION_TAKEN}", new.version);
    }

    let mut stored_keys = Vec::new();
    let mut artifacts = Vec::new();
    for f in files {
        let id = Uuid::new_v4();
        let file = match tokio::fs::File::open(&f.path).await {
            Ok(file) => file,
            Err(e) => {
                discard_all(store, &stored_keys).await;
                bail!("--file {:?}: cannot open {}: {e}", f.spec, f.path.display());
            }
        };
        match store_artifact(store, &id.to_string(), file, limit).await {
            Ok(stored) => {
                stored_keys.push(id);
                artifacts.push(NewArtifact {
                    id,
                    platform: f.platform.clone(),
                    filename: f.filename.clone(),
                    stored,
                });
            }
            Err(e) => {
                discard_all(store, &stored_keys).await;
                bail!("--file {:?}: {e}", f.spec);
            }
        }
    }

    let who = Actor::Cli { os_user: os_user() };
    let result = async {
        let mut tx = conn.begin().await?;
        let id = create_as(&mut tx, &who, new).await?;
        for a in &artifacts {
            add_artifact_as(&mut tx, &who, id, a).await?;
        }
        publish_as(&mut tx, &who, id).await?;
        let release = fetch(&mut tx, id).await?;
        tx.commit().await?;
        Ok::<_, ReleaseError>(release)
    }
    .await;
    match result {
        Ok(release) => Ok(release),
        Err(e) => {
            discard_all(store, &stored_keys).await;
            Err(match e {
                ReleaseError::Conflict(m) if m == MSG_VERSION_TAKEN => {
                    anyhow!("--version {}: {m}", new.version)
                }
                ReleaseError::Conflict(m) => anyhow!("{m}"),
                e => anyhow::Error::new(e).context("publishing the release failed"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(version: &str, notes: &str, channel: &str) -> ReleaseRequest {
        ReleaseRequest {
            version: version.into(),
            notes: notes.into(),
            channel: channel.into(),
        }
    }

    #[test]
    fn a_good_body_is_trimmed() {
        let r = validate(&req(
            " 0.2.0 ",
            "  First build.\n\n- fixes\tthings  ",
            "stable",
        ))
        .unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.notes, "First build.\n\n- fixes\tthings");
        assert_eq!(r.channel, "stable");
        for v in ["1.0.0-rc.1", "0.0.1-alpha.1", "10.20.30"] {
            assert_eq!(validate_version(v).unwrap(), v);
        }
        assert!(validate(&req("1.0.0", &"x".repeat(NOTES_MAX_CHARS), "preview")).is_ok());
    }

    #[test]
    fn bad_bodies_name_the_field() {
        for v in [
            "",
            " ",
            "1.0",
            "v1.0.0",
            "1.0.0.0",
            "01.0.0",
            "1.0.0-",
            "latest",
            "1.0.0+build.5",
            "0.0.1-alpha.1+sha.abc",
        ] {
            assert!(
                validate_version(v).unwrap_err().starts_with("version"),
                "{v}"
            );
        }
        let long = format!("1.0.0-{}", "a".repeat(60));
        assert!(validate_version(&long).unwrap_err().contains("at most 64"));
        assert_eq!(
            validate_notes("  \n ").unwrap_err(),
            "notes must not be blank"
        );
        assert!(validate_notes(&"x".repeat(NOTES_MAX_CHARS + 1))
            .unwrap_err()
            .starts_with("notes must be at most"));
        for n in ["a\rb", "a\u{7}", "a\u{0}b", "a\u{85}b"] {
            assert!(validate_notes(n).unwrap_err().starts_with("notes"), "{n:?}");
        }
        for c in ["", "Stable", "beta", "stable "] {
            assert!(
                validate_channel(c).unwrap_err().starts_with("channel"),
                "{c}"
            );
        }
    }

    #[test]
    fn unknown_keys_are_refused() {
        let ok = json!({"version": "1.0.0", "notes": "n", "channel": "stable"});
        assert!(serde_json::from_value::<ReleaseRequest>(ok.clone()).is_ok());
        let mut bad = ok.clone();
        bad["status"] = json!("published");
        let err = serde_json::from_value::<ReleaseRequest>(bad).unwrap_err();
        assert!(err.to_string().contains("status"), "{err}");
    }

    #[test]
    fn platform_labels() {
        for p in ["linux-x86_64", "a", "windows.arm64", "0", &"a".repeat(64)] {
            assert_eq!(validate_platform(p).unwrap(), p);
        }
        for p in [
            "",
            "Linux",
            "-x",
            ".x",
            "_x",
            "linux x86",
            "linux/x86",
            "é",
            &"a".repeat(65),
        ] {
            assert_eq!(validate_platform(p).unwrap_err(), MSG_BAD_PLATFORM, "{p}");
        }
    }

    #[test]
    fn file_names() {
        for f in [
            "wipestation",
            "wipestation-0.2.0.tar.gz",
            "Setup+x64.msi",
            "_a",
            "-a",
            &"a".repeat(128),
        ] {
            assert_eq!(validate_filename(f).unwrap(), f);
        }
        for f in [
            "",
            ".hidden",
            "..",
            "a/b",
            "a\\b",
            "a b",
            "naïve",
            "a\u{0}",
            &"a".repeat(129),
        ] {
            assert_eq!(validate_filename(f).unwrap_err(), MSG_BAD_FILENAME, "{f}");
        }
    }

    #[test]
    fn cli_files_are_checked_before_anything_is_stored() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("wipestation");
        std::fs::write(&good, b"binary").unwrap();
        let empty = dir.path().join("empty");
        std::fs::write(&empty, b"").unwrap();
        let bad_name = dir.path().join(".hidden");
        std::fs::write(&bad_name, b"x").unwrap();
        let g = good.display();
        let ok = cli_files(&[format!("linux-x86_64={g}")], 100).unwrap();
        assert_eq!(ok[0].platform, "linux-x86_64");
        assert_eq!(ok[0].filename, "wipestation");
        for (specs, needle) in [
            (vec![format!("{g}")], "expected <platform>=<path>"),
            (vec![format!("Linux={g}")], "platform must be"),
            (
                vec![format!("a={g}"), format!("a={g}")],
                "given more than once",
            ),
            (
                vec![format!("a={}", dir.path().join("absent").display())],
                "cannot read",
            ),
            (
                vec![format!("a={}", dir.path().display())],
                "not a regular file",
            ),
            (vec![format!("a={}", empty.display())], "is empty"),
            (
                vec![format!("a={}", bad_name.display())],
                "filename must be",
            ),
        ] {
            let err = cli_files(&specs, 100).unwrap_err().to_string();
            assert!(err.contains(needle), "{specs:?}: {err}");
            assert!(err.starts_with("--file"), "{err}");
        }
        let err = cli_files(&[format!("a={g}")], 3).unwrap_err().to_string();
        assert!(err.contains("over the 3-byte limit"), "{err}");
    }

    #[test]
    fn cli_audit_rows_carry_the_os_user() {
        let who = Actor::Cli {
            os_user: Some("ci".into()),
        };
        assert_eq!(who.name(), CLI_ACTOR);
        assert_eq!(who.created_by(), None);
    }
}
