//! Entitled downloads (software-downloads story 2, CAP-2 and CAP-3, ADR-0007
//! §5, §6): customers of an organization with a current entitlement list the
//! published releases and fetch their files through short-lived signed
//! links; every fetch is recorded. The routes are in
//! [`crate::download_routes`].
//!
//! - **Entitled** means the organization has at least one current
//!   entitlement: not refunded and `term_end > now()`, the rule
//!   [`crate::entitlements::list`] reports as `current`, from the same SQL
//!   ([`crate::entitlements::CURRENT_SQL`]) (a cancelled one stays
//!   current until its term ends). The vendor organization is never
//!   entitled. The check runs when a link is issued and again when it is
//!   fetched.
//! - **Customers see published releases only**, both channels. A draft or
//!   withdrawn release's files can be neither linked nor fetched
//!   ([`MSG_NO_DOWNLOAD`], the same as an unknown id).
//! - **A download link** is a bearer credential: `/api/downloads/<artifact
//!   id>/file?user=<user id>&expires=<unix seconds>&sig=<signature>`, where
//!   the signature is HMAC-SHA256 over [`signed_string`] with a key the hub
//!   draws at start ([`DownloadSigner`]), base64url without padding. It is
//!   good for [`crate::config::Config::download_link_ttl`] (300 s), needs no
//!   session, and is re-checked against its user and their organization's
//!   entitlement when fetched. A restart voids every outstanding link.
//! - **A fetch is recorded once the file is open and before it streams**:
//!   one transaction inserts a `release_downloads` row (migration 0018) and
//!   a `release.downloaded` audit row (actor `user:<id>`, subject
//!   `release:<id>`, detail `{artifact_id, platform, filename, sha256,
//!   organization_id}`). A refused fetch, or one whose file the store cannot
//!   open, writes nothing; an interrupted transfer still counts.
//! - Listing and issuing links are not audited; only the fetch is.

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::Mac;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::postgres::PgConnection;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin::RFC3339_UTC;
use crate::audit::{self, AuditEntry, AuditError};
use crate::auth::actor;
use crate::entitlements::CURRENT_SQL;
use crate::payments::HmacSha256;
use crate::release_admin;

/// How long a download link is good for, unless a test changes
/// [`crate::config::Config::download_link_ttl`].
pub const DOWNLOAD_LINK_TTL: Duration = Duration::from_secs(300);

/// The portal matches this text (`NOT_ENTITLED` in
/// `apps/portal/src/api/downloads.ts`) to offer "buy a licence" rather than
/// an error; change both together.
pub const MSG_NOT_ENTITLED: &str =
    "your organization has no current entitlement, so it cannot download releases";
pub const MSG_NO_DOWNLOAD: &str = "no such download";
pub const MSG_LINK_INVALID: &str = "this download link is invalid or has expired";

/// The download routes' common prefix (a link is `{DOWNLOADS}/<id>/file?…`).
const DOWNLOADS: &str = "/api/downloads";

/// Domain separation for the link signature.
const SIGNED_PREFIX: &str = "wipe-hub/download/v1";

/// What a link signature covers.
pub fn signed_string(artifact_id: Uuid, user_id: Uuid, expires: i64) -> String {
    format!("{SIGNED_PREFIX}\n{artifact_id}\n{user_id}\n{expires}")
}

/// A refusal, a storage failure, or a database failure (500).
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    /// 403 [`MSG_NOT_ENTITLED`].
    #[error("{MSG_NOT_ENTITLED}")]
    NotEntitled,
    /// 404 [`MSG_NO_DOWNLOAD`].
    #[error("{MSG_NO_DOWNLOAD}")]
    NoDownload,
    /// 403 [`MSG_LINK_INVALID`].
    #[error("{MSG_LINK_INVALID}")]
    LinkInvalid,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

// ---------------------------------------------------------------------------
// Signing

/// Signs and verifies download links with 32 random bytes drawn from the OS
/// when the hub starts. The key never leaves the process and is never
/// logged.
#[derive(Clone)]
pub struct DownloadSigner {
    key: [u8; 32],
}

impl std::fmt::Debug for DownloadSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DownloadSigner { key: <redacted> }")
    }
}

impl DownloadSigner {
    /// A signer with a fresh random key.
    pub fn random() -> Self {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        Self { key }
    }

    fn mac(&self, artifact_id: Uuid, user_id: Uuid, expires: i64) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("HMAC takes any key length");
        mac.update(signed_string(artifact_id, user_id, expires).as_bytes());
        mac
    }

    /// The link signature, base64url without padding.
    pub fn sign(&self, artifact_id: Uuid, user_id: Uuid, expires: i64) -> String {
        URL_SAFE_NO_PAD.encode(
            self.mac(artifact_id, user_id, expires)
                .finalize()
                .into_bytes(),
        )
    }

    /// The artifact and user a link names, if its fields parse, its
    /// signature matches (compared in constant time) and it expires after
    /// `now` (unix seconds); otherwise [`DownloadError::LinkInvalid`].
    pub fn verify(
        &self,
        artifact: &str,
        user: Option<&str>,
        expires: Option<&str>,
        sig: Option<&str>,
        now: i64,
    ) -> Result<(Uuid, Uuid), DownloadError> {
        let invalid = || DownloadError::LinkInvalid;
        let artifact_id = Uuid::parse_str(artifact).map_err(|_| invalid())?;
        let user_id = Uuid::parse_str(user.ok_or_else(invalid)?).map_err(|_| invalid())?;
        let expires: i64 = expires
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let sig = URL_SAFE_NO_PAD
            .decode(sig.ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        self.mac(artifact_id, user_id, expires)
            .verify_slice(&sig)
            .map_err(|_| invalid())?;
        if expires <= now {
            return Err(invalid());
        }
        Ok((artifact_id, user_id))
    }
}

/// Now, in unix seconds.
pub fn unix_now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// A signed link, as the link route answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadLink {
    /// Path-relative: `/api/downloads/<id>/file?user=…&expires=…&sig=…`.
    pub url: String,
    /// RFC 3339, UTC.
    pub expires_at: String,
}

/// Sign a link to `artifact_id` for `user_id`, good for `ttl` from `now`.
pub fn link_for(
    signer: &DownloadSigner,
    ttl: Duration,
    artifact_id: Uuid,
    user_id: Uuid,
    now: i64,
) -> DownloadLink {
    let expires = now.saturating_add(i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX));
    let sig = signer.sign(artifact_id, user_id, expires);
    let expires_at = OffsetDateTime::from_unix_timestamp(expires)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default();
    DownloadLink {
        url: format!("{DOWNLOADS}/{artifact_id}/file?user={user_id}&expires={expires}&sig={sig}"),
        expires_at,
    }
}

// ---------------------------------------------------------------------------
// Shapes

/// One file of a published release, as a customer sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct DownloadArtifact {
    pub id: Uuid,
    pub platform: String,
    pub filename: String,
    pub size_bytes: i64,
    /// Lowercase hex SHA-256, computed by the hub when it stored the file.
    pub sha256: String,
}

/// A published release, as a customer sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadRelease {
    pub id: Uuid,
    pub version: String,
    pub notes: String,
    /// `stable` or `preview`.
    pub channel: String,
    /// RFC 3339, UTC.
    pub published_at: String,
    /// By platform.
    pub artifacts: Vec<DownloadArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadList {
    /// Published only, newest publication first.
    pub releases: Vec<DownloadRelease>,
}

#[derive(sqlx::FromRow)]
struct ReleaseRow {
    id: Uuid,
    version: String,
    notes: String,
    channel: String,
    published_at: String,
}

#[derive(sqlx::FromRow)]
struct ArtifactRow {
    release_id: Uuid,
    #[sqlx(flatten)]
    artifact: DownloadArtifact,
}

// ---------------------------------------------------------------------------
// Queries

/// Whether `organization_id` has a current entitlement (and is not the
/// vendor organization).
pub async fn is_entitled(
    conn: &mut PgConnection,
    organization_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT EXISTS ( \
             SELECT 1 FROM entitlements e JOIN organizations o ON o.id = e.organization_id \
             WHERE e.organization_id = $1 AND NOT o.is_vendor AND {CURRENT_SQL})"
    ))
    .bind(organization_id)
    .fetch_one(&mut *conn)
    .await
}

/// [`DownloadError::NotEntitled`] unless [`is_entitled`].
pub async fn require_entitled(
    conn: &mut PgConnection,
    organization_id: Uuid,
) -> Result<(), DownloadError> {
    if is_entitled(conn, organization_id).await? {
        Ok(())
    } else {
        Err(DownloadError::NotEntitled)
    }
}

/// Every published release with its files, newest publication first, for
/// an entitled organization ([`DownloadError::NotEntitled`] otherwise).
pub async fn list_for(
    conn: &mut PgConnection,
    organization_id: Uuid,
) -> Result<DownloadList, DownloadError> {
    require_entitled(conn, organization_id).await?;
    let rows: Vec<ReleaseRow> = sqlx::query_as(&format!(
        "SELECT id, version, notes, channel, \
                to_char(published_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS published_at \
         FROM releases WHERE status = 'published' \
         ORDER BY releases.published_at DESC, releases.id DESC"
    ))
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let artifacts: Vec<ArtifactRow> = sqlx::query_as(
        "SELECT release_id, id, platform, filename, size_bytes, sha256 \
         FROM release_artifacts WHERE release_id = ANY($1) ORDER BY platform, id",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let releases = rows
        .into_iter()
        .map(|r| DownloadRelease {
            artifacts: artifacts
                .iter()
                .filter(|a| a.release_id == r.id)
                .map(|a| a.artifact.clone())
                .collect(),
            id: r.id,
            version: r.version,
            notes: r.notes,
            channel: r.channel,
            published_at: r.published_at,
        })
        .collect();
    Ok(DownloadList { releases })
}

/// A published release's file, with its release, or `None`. `lock` takes a
/// share lock on the release row, so it cannot be withdrawn until the
/// caller's transaction ends.
async fn published_artifact(
    conn: &mut PgConnection,
    artifact_id: Uuid,
    lock: bool,
) -> Result<Option<(Uuid, DownloadArtifact)>, sqlx::Error> {
    let row: Option<ArtifactRow> = sqlx::query_as(&format!(
        "SELECT a.release_id, a.id, a.platform, a.filename, a.size_bytes, a.sha256 \
         FROM release_artifacts a JOIN releases r ON r.id = a.release_id \
         WHERE a.id = $1 AND r.status = 'published'{}",
        if lock { " FOR SHARE OF r" } else { "" }
    ))
    .bind(artifact_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|r| (r.release_id, r.artifact)))
}

/// A link to `artifact` (the path segment, any string) for a signed-in
/// user of `organization_id`: entitlement first ([`DownloadError::NotEntitled`]),
/// then a published release's file ([`DownloadError::NoDownload`] for a
/// draft, withdrawn, unknown or non-UUID id). Not audited.
pub async fn issue_link(
    conn: &mut PgConnection,
    signer: &DownloadSigner,
    ttl: Duration,
    organization_id: Uuid,
    user_id: Uuid,
    artifact: &str,
) -> Result<DownloadLink, DownloadError> {
    require_entitled(conn, organization_id).await?;
    let artifact_id = Uuid::parse_str(artifact).map_err(|_| DownloadError::NoDownload)?;
    if published_artifact(conn, artifact_id, false)
        .await?
        .is_none()
    {
        return Err(DownloadError::NoDownload);
    }
    Ok(link_for(signer, ttl, artifact_id, user_id, unix_now()))
}

/// What a verified link resolves to, ready to record and stream.
#[derive(Debug, Clone)]
pub struct Fetch {
    pub release_id: Uuid,
    pub artifact: DownloadArtifact,
    pub user_id: Uuid,
    pub organization_id: Uuid,
}

/// Resolve a verified link inside the caller's transaction: its user
/// (gone: [`DownloadError::LinkInvalid`]), their organization's entitlement
/// ([`DownloadError::NotEntitled`]), then the file of a published release,
/// share-locked ([`DownloadError::NoDownload`]).
pub async fn resolve_fetch(
    conn: &mut PgConnection,
    artifact_id: Uuid,
    user_id: Uuid,
) -> Result<Fetch, DownloadError> {
    let organization_id: Option<Uuid> =
        sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(organization_id) = organization_id else {
        return Err(DownloadError::LinkInvalid);
    };
    require_entitled(conn, organization_id).await?;
    let Some((release_id, artifact)) = published_artifact(conn, artifact_id, true).await? else {
        return Err(DownloadError::NoDownload);
    };
    Ok(Fetch {
        release_id,
        artifact,
        user_id,
        organization_id,
    })
}

/// Record `fetch`: a `release_downloads` row and its `release.downloaded`
/// audit row, in the caller's transaction.
pub async fn record(conn: &mut PgConnection, fetch: &Fetch) -> Result<(), DownloadError> {
    sqlx::query(
        "INSERT INTO release_downloads \
             (release_id, artifact_id, organization_id, user_id) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(fetch.release_id)
    .bind(fetch.artifact.id)
    .bind(fetch.organization_id)
    .bind(fetch.user_id)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &actor(fetch.user_id),
            action: "release.downloaded",
            subject: &release_admin::subject(fetch.release_id),
            detail: json!({
                "artifact_id": fetch.artifact.id,
                "platform": fetch.artifact.platform,
                "filename": fetch.artifact.filename,
                "sha256": fetch.artifact.sha256,
                "organization_id": fetch.organization_id,
            }),
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn ids() -> (Uuid, Uuid) {
        (Uuid::new_v4(), Uuid::new_v4())
    }

    /// The query fields of `url` as `(path artifact, user, expires, sig)`.
    fn parts(url: &str) -> (String, String, String, String) {
        let (path, query) = url.split_once('?').unwrap();
        let artifact = path
            .strip_prefix(DOWNLOADS)
            .and_then(|p| p.strip_prefix('/'))
            .and_then(|p| p.strip_suffix("/file"))
            .unwrap()
            .to_string();
        let get = |k: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(&format!("{k}=")))
                .unwrap()
                .to_string()
        };
        (artifact, get("user"), get("expires"), get("sig"))
    }

    #[test]
    fn a_signed_link_verifies_until_it_expires() {
        let s = DownloadSigner::random();
        let (artifact, user) = ids();
        let link = link_for(&s, Duration::from_secs(300), artifact, user, NOW);
        let (a, u, e, sig) = parts(&link.url);
        assert_eq!(e, (NOW + 300).to_string());
        assert_eq!(link.expires_at, "2027-01-15T08:05:00Z");
        assert_eq!(
            s.verify(&a, Some(&u), Some(&e), Some(&sig), NOW).unwrap(),
            (artifact, user)
        );
        assert_eq!(
            s.verify(&a, Some(&u), Some(&e), Some(&sig), NOW + 299)
                .unwrap(),
            (artifact, user)
        );
        for now in [NOW + 300, NOW + 301] {
            assert!(matches!(
                s.verify(&a, Some(&u), Some(&e), Some(&sig), now),
                Err(DownloadError::LinkInvalid)
            ));
        }
        // A zero lifetime is already expired.
        let zero = link_for(&s, Duration::ZERO, artifact, user, NOW);
        let (a, u, e, sig) = parts(&zero.url);
        assert!(s.verify(&a, Some(&u), Some(&e), Some(&sig), NOW).is_err());
        // Base64url without padding: safe in a query string as is.
        assert!(sig
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    }

    /// `verify`'s arguments: artifact, user, expires, sig.
    type Case<'a> = (&'a str, Option<&'a str>, Option<&'a str>, Option<&'a str>);

    #[test]
    fn any_tampered_field_or_another_key_is_refused() {
        let s = DownloadSigner::random();
        let (artifact, user) = ids();
        let link = link_for(&s, Duration::from_secs(300), artifact, user, NOW);
        let (a, u, e, sig) = parts(&link.url);
        let other = Uuid::new_v4().to_string();
        let mut flipped = sig.clone().into_bytes();
        flipped[0] = if flipped[0] == b'A' { b'B' } else { b'A' };
        let flipped = String::from_utf8(flipped).unwrap();
        let later = (NOW + 3600).to_string();
        let cases: Vec<Case> = vec![
            (&other, Some(&u), Some(&e), Some(&sig)),
            (&a, Some(&other), Some(&e), Some(&sig)),
            (&a, Some(&u), Some(&later), Some(&sig)),
            (&a, Some(&u), Some(&e), Some(&flipped)),
            // Missing fields.
            (&a, None, Some(&e), Some(&sig)),
            (&a, Some(&u), None, Some(&sig)),
            (&a, Some(&u), Some(&e), None),
            // Malformed fields.
            ("not-a-uuid", Some(&u), Some(&e), Some(&sig)),
            (&a, Some("x"), Some(&e), Some(&sig)),
            (&a, Some(&u), Some("soon"), Some(&sig)),
            (&a, Some(&u), Some(&e), Some("!!!not base64!!!")),
            (&a, Some(&u), Some(&e), Some("")),
            (&a, Some(&u), Some(&e), Some(&sig[..10])),
        ];
        for (a, u, e, sig) in cases {
            assert!(
                matches!(s.verify(a, u, e, sig, NOW), Err(DownloadError::LinkInvalid)),
                "{a} {u:?} {e:?} {sig:?}"
            );
        }
        // Padded base64 is not the encoding links use.
        let padded = format!("{sig}=");
        assert!(s
            .verify(&a, Some(&u), Some(&e), Some(&padded), NOW)
            .is_err());
        // Another hub process (another key) refuses the link.
        let restarted = DownloadSigner::random();
        assert!(restarted
            .verify(&a, Some(&u), Some(&e), Some(&sig), NOW)
            .is_err());
    }

    #[test]
    fn the_signed_string_is_domain_separated() {
        let (a, u) = ids();
        assert_eq!(
            signed_string(a, u, 7),
            format!("wipe-hub/download/v1\n{a}\n{u}\n7")
        );
    }

    #[test]
    fn portal_not_entitled_matches_the_hub_message() {
        // The portal offers "buy a licence" on exactly this 403 text; pin the
        // copy so a change on either side fails here, not in a browser.
        let portal = include_str!("../../../apps/portal/src/api/downloads.ts");
        assert!(
            portal.contains("export const NOT_ENTITLED ="),
            "portal must declare NOT_ENTITLED"
        );
        let quoted = format!("\"{MSG_NOT_ENTITLED}\";");
        assert!(portal.contains(&quoted), "portal must quote {quoted}");
    }

    #[test]
    fn the_key_is_never_printed() {
        assert_eq!(
            format!("{:?}", DownloadSigner::random()),
            "DownloadSigner { key: <redacted> }"
        );
    }
}
