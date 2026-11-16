//! The licence admin data layer (licence-delivery story 3, CAP-4, ROADMAP
//! A1.2, A-R1, A-R7): what vendor admins see of the licences the hub holds,
//! and the validation of what they ask to issue or revoke. The routes are in
//! [`crate::admin_routes`]; the writes themselves (vendor issue,
//! revocation) are in [`crate::stations`], the only code that writes
//! `licences`.
//!
//! The rules of [`crate::admin`] apply: every read takes a `&VendorAdmin`
//! and a connection (a transaction in practice) and records its own audit
//! row (actor `user:<admin id>`) before it returns data; a lookup that finds
//! nothing writes nothing.
//!
//! - **List** ([`list_for_organization`]): every licence of a customer
//!   organization, station and vendor-issued, newest first, audited as
//!   `admin.organization_licences_viewed {licence_ids}`.
//! - **View** ([`detail`]): one licence, the decoded body of its stored file
//!   and whether that file verifies against the hub's vendor root
//!   ([`Verification`]), audited as `admin.licence_viewed`.
//! - **Inspect** ([`inspect_document`]): the same for a pasted licence file
//!   (at most [`INSPECT_MAX_BYTES`]), with the hub's record of it if it holds
//!   one, audited as `admin.licence_file_inspected`. Verification uses only
//!   the hub's own root.
//! - **Download** ([`document`]): the stored file, byte for byte, audited as
//!   `licence.downloaded {organization_id, by: "vendor"}`. Unlike the
//!   customer download, a revoked licence's file is still handed to a vendor
//!   admin.
//!
//! Revocation is recorded in the hub only: stations are not told, and a
//! revoked file keeps verifying offline until its own `not_after`
//! (ADR-0005 consequences, ADR-0007 §5).

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgConnection;
use uuid::Uuid;
use wipe_license::{Feature, LicenseBody, LicenseCertificate, VendorRoot};

use crate::accounts::Organization;
use crate::admin::{audit_read, VendorAdmin, RFC3339_UTC};
use crate::offering_admin::{FEATURES, QUOTA_MAX_ERASURES, SCOPES};
use crate::stations::{
    is_instance_key_id, licence_subject, StationError, MSG_BAD_INSTANCE_KEY,
    MSG_STATION_ID_REQUIRED, TEXT_MAX_CHARS,
};

/// The longest licence file accepted for inspection, in bytes.
pub const INSPECT_MAX_BYTES: usize = 64 * 1024;
/// The longest vendor-issued licence, in days (three years).
pub const DAYS_MAX: i64 = 1095;
/// The longest issue or revocation reason, in characters, after trimming.
pub const REASON_MAX_CHARS: usize = 500;

pub const MSG_NOT_A_LICENCE: &str =
    "this is not a licence file: paste the whole JSON file a station installs, at most 64 KiB";

// ---------------------------------------------------------------------------
// Requests and validation

/// What a vendor admin asks to issue. Unknown keys are refused, so a
/// misspelt field never silently falls back to a default.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueRequest {
    pub instance_key_id: String,
    /// `machine` or `site`.
    pub scope: String,
    /// The bench's `--station-id`; required for machine scope.
    #[serde(default)]
    pub station_id: Option<String>,
    /// Site scope only; blank is the organization id.
    #[serde(default)]
    pub site: Option<String>,
    /// Required, `null` for unlimited: the outer `None` is a missing key.
    #[serde(default, deserialize_with = "present")]
    pub quota_erasures: Option<Option<i64>>,
    pub features: Vec<String>,
    pub days: i64,
    pub reason: String,
}

/// A present key, `null` or not, as `Some(value)`.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<i64>>, D::Error> {
    Option::<i64>::deserialize(d).map(Some)
}

/// A validated [`IssueRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewVendorLicence {
    pub instance_key_id: String,
    /// One of [`SCOPES`].
    pub scope: String,
    /// Set for machine scope.
    pub station_id: Option<String>,
    /// Site scope only.
    pub site: Option<String>,
    /// `None` is unlimited.
    pub quota_erasures: Option<i64>,
    /// [`FEATURES`] names, sorted and de-duplicated.
    pub features: Vec<String>,
    /// 1..=[`DAYS_MAX`].
    pub days: u16,
    pub reason: String,
}

/// What a vendor admin sends to revoke a licence.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeRequest {
    pub reason: String,
}

/// A pasted licence file to inspect.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub document: String,
}

/// `field` trimmed: not blank, at most [`REASON_MAX_CHARS`] characters, no
/// control characters. The error is the 400 message.
pub fn validate_reason(field: &str, value: &str) -> Result<String, String> {
    let v = value.trim();
    if v.is_empty() {
        return Err(format!("{field} must not be blank"));
    }
    if v.chars().count() > REASON_MAX_CHARS {
        return Err(format!(
            "{field} must be at most {REASON_MAX_CHARS} characters"
        ));
    }
    if v.chars().any(char::is_control) {
        return Err(format!("{field} must not contain control characters"));
    }
    Ok(v.to_string())
}

/// Trimmed optional text, blank as `None`, at most [`TEXT_MAX_CHARS`]
/// characters without control characters.
fn optional_text(field: &str, value: Option<&str>) -> Result<Option<String>, String> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > TEXT_MAX_CHARS {
        return Err(format!(
            "{field} must be at most {TEXT_MAX_CHARS} characters"
        ));
    }
    if v.chars().any(char::is_control) {
        return Err(format!("{field} must not contain control characters"));
    }
    Ok(Some(v.to_string()))
}

impl IssueRequest {
    /// Validate a body; the error is the 400 message, naming the field.
    pub fn validate(&self) -> Result<NewVendorLicence, String> {
        let instance_key_id = self.instance_key_id.trim();
        if !is_instance_key_id(instance_key_id) {
            return Err(MSG_BAD_INSTANCE_KEY.into());
        }
        if !SCOPES.contains(&self.scope.as_str()) {
            return Err(format!("scope must be one of {}", SCOPES.join(", ")));
        }
        let station_id = optional_text("station_id", self.station_id.as_deref())?;
        if self.scope == "machine" && station_id.is_none() {
            return Err(MSG_STATION_ID_REQUIRED.into());
        }
        let site = if self.scope == "site" {
            optional_text("site", self.site.as_deref())?
        } else {
            None
        };
        let quota_erasures = match self.quota_erasures {
            None => return Err("quota_erasures is required: null for unlimited, or a count".into()),
            Some(None) => None,
            Some(Some(n)) if (1..=QUOTA_MAX_ERASURES).contains(&n) => Some(n),
            Some(Some(_)) => {
                return Err(format!(
                    "quota_erasures must be between 1 and {QUOTA_MAX_ERASURES}"
                ))
            }
        };
        let mut features = BTreeSet::new();
        for f in &self.features {
            // `wipe_license::Feature` decides; FEATURES only words the error.
            if serde_json::from_value::<Feature>(Value::String(f.clone())).is_err() {
                return Err(format!(
                    "features must contain only {}",
                    FEATURES.join(", ")
                ));
            }
            features.insert(f.clone());
        }
        if !(1..=DAYS_MAX).contains(&self.days) {
            return Err(format!("days must be between 1 and {DAYS_MAX}"));
        }
        let reason = validate_reason("reason", &self.reason)?;
        Ok(NewVendorLicence {
            instance_key_id: instance_key_id.to_string(),
            scope: self.scope.clone(),
            station_id,
            site,
            quota_erasures,
            features: features.into_iter().collect(),
            days: self.days as u16,
            reason,
        })
    }
}

// ---------------------------------------------------------------------------
// Shapes

/// The station a licence was issued for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminLicenceStation {
    /// The `stations` row.
    pub id: Uuid,
    /// The bench's `--station-id`, as entered.
    pub station_id: Option<String>,
    pub label: Option<String>,
}

/// A licence as the admin area shows it (never the file itself). Timestamps
/// are RFC 3339 UTC; absent values are `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminLicence {
    /// The signed body's `license_id`.
    pub id: Uuid,
    pub organization: Organization,
    /// `station` (registered against an entitlement) or `vendor` (issued by
    /// a vendor admin, outside any entitlement and seat).
    pub source: String,
    /// `null` for a vendor-issued licence.
    pub station: Option<AdminLicenceStation>,
    pub entitlement_id: Option<Uuid>,
    pub instance_key_id: String,
    pub root_key_id: String,
    pub not_after: String,
    pub issued_at: String,
    /// `active`, `superseded` or `revoked`.
    pub status: String,
    pub superseded_at: Option<String>,
    pub revoked_at: Option<String>,
    /// The vendor admin's reason for a vendor-issued licence.
    pub issue_reason: Option<String>,
    /// Vendor-internal: never shown to the customer.
    pub revocation_reason: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    organization_id: Uuid,
    organization_name: String,
    source: String,
    station: Option<Uuid>,
    bench_station_id: Option<String>,
    station_label: Option<String>,
    entitlement_id: Option<Uuid>,
    instance_key_id: String,
    root_key_id: String,
    not_after: String,
    issued_at: String,
    status: String,
    superseded_at: Option<String>,
    revoked_at: Option<String>,
    issue_reason: Option<String>,
    revocation_reason: Option<String>,
}

impl From<Row> for AdminLicence {
    fn from(r: Row) -> Self {
        AdminLicence {
            id: r.id,
            organization: Organization {
                id: r.organization_id,
                name: r.organization_name,
            },
            source: r.source,
            station: r.station.map(|id| AdminLicenceStation {
                id,
                station_id: r.bench_station_id,
                label: r.station_label,
            }),
            entitlement_id: r.entitlement_id,
            instance_key_id: r.instance_key_id,
            root_key_id: r.root_key_id,
            not_after: r.not_after,
            issued_at: r.issued_at,
            status: r.status,
            superseded_at: r.superseded_at,
            revoked_at: r.revoked_at,
            issue_reason: r.issue_reason,
            revocation_reason: r.revocation_reason,
        }
    }
}

/// `SELECT … FROM licences l …`, every column of [`Row`]; callers add the
/// `WHERE` on `l`.
fn select_licences() -> String {
    format!(
        "SELECT l.id, l.organization_id, o.name AS organization_name, l.source, \
                l.station_id AS station, s.station_id AS bench_station_id, \
                s.label AS station_label, l.entitlement_id, l.instance_key_id, l.root_key_id, \
                to_char(l.not_after AT TIME ZONE 'UTC', {RFC3339_UTC}) AS not_after, \
                to_char(l.issued_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS issued_at, \
                l.status, \
                to_char(l.superseded_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS superseded_at, \
                to_char(l.revoked_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS revoked_at, \
                l.issue_reason, l.revocation_reason \
         FROM licences l \
         JOIN organizations o ON o.id = l.organization_id \
         LEFT JOIN stations s ON s.id = l.station_id"
    )
}

/// Whether a licence file verifies against the hub's vendor root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    /// `verified`, `failed` (tampered, signed by another root, or malformed)
    /// or `no_root` (this hub has no vendor root to check against).
    pub status: String,
    /// The hub's vendor root key id; `null` with `no_root`.
    pub root_key_id: Option<String>,
    /// Why it failed; `null` otherwise.
    pub detail: Option<String>,
}

/// Verify `certificate` against `root` alone.
pub fn verification(certificate: &LicenseCertificate, root: Option<&VendorRoot>) -> Verification {
    let Some(root) = root else {
        return Verification {
            status: "no_root".into(),
            root_key_id: None,
            detail: None,
        };
    };
    let root_key_id = Some(root.public_key_id());
    match certificate.verify(&[root.verifying_key()]) {
        Ok(_) => Verification {
            status: "verified".into(),
            root_key_id,
            detail: None,
        },
        Err(e) => Verification {
            status: "failed".into(),
            root_key_id,
            detail: Some(e.to_string()),
        },
    }
}

/// One licence's page: the record, its file's decoded body and whether that
/// file verifies.
#[derive(Debug, Clone, Serialize)]
pub struct LicenceDetail {
    pub licence: AdminLicence,
    pub body: LicenseBody,
    pub verification: Verification,
}

/// A pasted licence file, decoded and checked, with the hub's record of it.
#[derive(Debug, Clone, Serialize)]
pub struct Inspected {
    pub body: LicenseBody,
    pub verification: Verification,
    /// The licence the hub holds with this `license_id`, if any.
    pub licence: Option<AdminLicence>,
}

// ---------------------------------------------------------------------------
// Reads

/// Licence `id`, unaudited: for a write's answer, which its own audit row
/// covers.
pub(crate) async fn get(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<AdminLicence>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(&format!("{} WHERE l.id = $1", select_licences()))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.map(AdminLicence::from))
}

/// Every licence of customer organization `organization_id`, newest first.
/// `None` (and no audit row) for an unknown id or the vendor organization;
/// otherwise audited as `admin.organization_licences_viewed {licence_ids}`.
pub async fn list_for_organization(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    organization_id: Uuid,
) -> Result<Option<Vec<AdminLicence>>, StationError> {
    let found: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM organizations WHERE id = $1 AND NOT is_vendor)",
    )
    .bind(organization_id)
    .fetch_one(&mut *conn)
    .await?;
    if !found {
        return Ok(None);
    }
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "{} WHERE l.organization_id = $1 ORDER BY l.issued_at DESC, l.id DESC",
        select_licences()
    ))
    .bind(organization_id)
    .fetch_all(&mut *conn)
    .await?;
    let licences: Vec<AdminLicence> = rows.into_iter().map(AdminLicence::from).collect();
    let ids: Vec<Uuid> = licences.iter().map(|l| l.id).collect();
    audit_read(
        conn,
        admin,
        "admin.organization_licences_viewed",
        &format!("organization:{organization_id}"),
        json!({ "licence_ids": ids }),
    )
    .await?;
    Ok(Some(licences))
}

/// The stored file of licence `id` and its document, `None` if unknown.
async fn stored(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<(AdminLicence, String)>, StationError> {
    let Some(licence) = get(conn, id).await? else {
        return Ok(None);
    };
    let document: String = sqlx::query_scalar("SELECT document FROM licences WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(Some((licence, document)))
}

/// Licence `id`, its stored file's body and that file's verification
/// against `root`. `None` (and no audit row) for an unknown id; otherwise
/// audited as `admin.licence_viewed {organization_id}`.
pub async fn detail(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    root: Option<&VendorRoot>,
    id: Uuid,
) -> Result<Option<LicenceDetail>, StationError> {
    let Some((licence, document)) = stored(conn, id).await? else {
        return Ok(None);
    };
    let certificate: LicenseCertificate = serde_json::from_str(&document)
        .map_err(|e| StationError::Issue(format!("stored licence {id} does not parse: {e}")))?;
    let verification = verification(&certificate, root);
    audit_read(
        conn,
        admin,
        "admin.licence_viewed",
        &licence_subject(id),
        json!({ "organization_id": licence.organization.id }),
    )
    .await?;
    Ok(Some(LicenceDetail {
        licence,
        body: certificate.body,
        verification,
    }))
}

/// Decode a pasted licence file, verify it against `root` and find the
/// hub's record of it. 400 [`MSG_NOT_A_LICENCE`] (nothing audited) when it
/// is over [`INSPECT_MAX_BYTES`] or not a licence; otherwise audited as
/// `admin.licence_file_inspected {license_id, canonical_sha256_hex,
/// verified, known}` (subject `inspect:licence-file`): `license_id` only when
/// it is a UUID (else null), and the hash computed by the hub over the
/// pasted licence body, not the one the file claims. A pasted
/// `{license, credit_grant}` file is inspected by its licence.
pub async fn inspect_document(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    root: Option<&VendorRoot>,
    document: &str,
) -> Result<Inspected, StationError> {
    let not_a_licence = || StationError::BadRequest(MSG_NOT_A_LICENCE.into());
    if document.len() > INSPECT_MAX_BYTES {
        return Err(not_a_licence());
    }
    // Either form a customer downloads: the bare licence or the
    // `{license, credit_grant}` file; only the licence is inspected.
    let certificate: LicenseCertificate = wipe_license::parse_license_file(document)
        .map_err(|_| not_a_licence())?
        .license;
    let verification = verification(&certificate, root);
    let parsed_id = Uuid::parse_str(&certificate.body.license_id).ok();
    let licence = match parsed_id {
        Some(id) => get(conn, id).await?,
        None => None,
    };
    // The hash of what was actually pasted, computed here: the file's own
    // `canonical_sha256_hex` is a claim, and wrong for a tampered file.
    let canonical_sha256_hex = wipe_cert::canonical::canonical_bytes(&certificate.body)
        .map(|b| hex::encode(Sha256::digest(&b)))
        .map_err(|_| not_a_licence())?;
    audit_read(
        conn,
        admin,
        "admin.licence_file_inspected",
        "inspect:licence-file",
        json!({
            // Only a UUID goes into the append-only log: a pasted id is
            // untrusted text of any length.
            "license_id": parsed_id,
            "canonical_sha256_hex": canonical_sha256_hex,
            "verified": verification.status == "verified",
            "known": licence.is_some(),
        }),
    )
    .await?;
    Ok(Inspected {
        body: certificate.body,
        verification,
        licence,
    })
}

/// The file of licence `id`, the same one the customer download serves
/// (the stored licence byte for byte, or the `{license, credit_grant}` file
/// when it was issued with a credit grant), audited as
/// `licence.downloaded {organization_id, by: "vendor"}`; `None` (nothing
/// audited) for an unknown id. A revoked licence's file is still returned.
pub async fn document(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
) -> Result<Option<String>, StationError> {
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT organization_id, document FROM licences WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((organization_id, document)) = row else {
        return Ok(None);
    };
    audit_read(
        conn,
        admin,
        "licence.downloaded",
        &licence_subject(id),
        json!({ "organization_id": organization_id, "by": "vendor" }),
    )
    .await?;
    Ok(Some(
        crate::credits::licence_file(conn, id, document).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> IssueRequest {
        IssueRequest {
            instance_key_id: wipe_cert::SigningKey::generate().public_key_id(),
            scope: "machine".into(),
            station_id: Some(" bench-1 ".into()),
            site: None,
            quota_erasures: Some(None),
            features: vec![
                "pdf_certificates".into(),
                "hub_sync".into(),
                "hub_sync".into(),
            ],
            days: 30,
            reason: "  evaluation for Acme ".into(),
        }
    }

    #[test]
    fn a_good_request_is_trimmed_and_sorted() {
        let v = req().validate().unwrap();
        assert_eq!(v.station_id.as_deref(), Some("bench-1"));
        assert_eq!(v.features, ["hub_sync", "pdf_certificates"]);
        assert_eq!(v.reason, "evaluation for Acme");
        assert_eq!(v.days, 30);
        assert_eq!(v.quota_erasures, None);
    }

    #[test]
    fn bad_requests_name_the_field() {
        let cases: Vec<(IssueRequest, &str)> = vec![
            (
                IssueRequest {
                    instance_key_id: "ed25519:short".into(),
                    ..req()
                },
                "instance key",
            ),
            (
                IssueRequest {
                    scope: "galaxy".into(),
                    ..req()
                },
                "scope",
            ),
            (
                IssueRequest {
                    station_id: Some("  ".into()),
                    ..req()
                },
                "station",
            ),
            (
                IssueRequest {
                    quota_erasures: None,
                    ..req()
                },
                "quota_erasures",
            ),
            (
                IssueRequest {
                    quota_erasures: Some(Some(0)),
                    ..req()
                },
                "quota_erasures",
            ),
            (
                IssueRequest {
                    quota_erasures: Some(Some(QUOTA_MAX_ERASURES + 1)),
                    ..req()
                },
                "quota_erasures",
            ),
            (
                IssueRequest {
                    features: vec!["teleport".into()],
                    ..req()
                },
                "features",
            ),
            (IssueRequest { days: 0, ..req() }, "days"),
            (
                IssueRequest {
                    days: DAYS_MAX + 1,
                    ..req()
                },
                "days",
            ),
            (
                IssueRequest {
                    reason: "  ".into(),
                    ..req()
                },
                "reason",
            ),
            (
                IssueRequest {
                    reason: "x".repeat(REASON_MAX_CHARS + 1),
                    ..req()
                },
                "reason",
            ),
            (
                IssueRequest {
                    reason: "two\nlines".into(),
                    ..req()
                },
                "reason",
            ),
        ];
        for (r, field) in cases {
            let err = r.validate().unwrap_err();
            assert!(err.contains(field), "{field}: {err}");
        }
        assert!(IssueRequest {
            days: DAYS_MAX,
            quota_erasures: Some(Some(QUOTA_MAX_ERASURES)),
            ..req()
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn a_site_licence_needs_no_station_id_and_ignores_none() {
        let v = IssueRequest {
            scope: "site".into(),
            station_id: None,
            site: Some("  ".into()),
            ..req()
        }
        .validate()
        .unwrap();
        assert_eq!(v.site, None);
    }

    #[test]
    fn verification_reads_verified_failed_and_no_root() {
        let root = VendorRoot::generate();
        let other = VendorRoot::generate();
        let now = time::OffsetDateTime::now_utc();
        let ent = wipe_license::Entitlements {
            customer_id: "c".into(),
            customer_name: "C".into(),
            quota: wipe_license::Quota::Unlimited,
            scope: wipe_license::Scope::Site {
                site_id: "s".into(),
            },
            not_before: now,
            not_after: now + time::Duration::days(1),
            features: vec![],
            allowed_methods: wipe_license::AllowedMethods::All,
            machine_binding: None,
            paid_through: None,
        };
        let cert = root.issue("l", "k", ent, now).unwrap();
        assert_eq!(verification(&cert, Some(&root)).status, "verified");
        assert_eq!(verification(&cert, None).status, "no_root");
        let foreign = verification(&cert, Some(&other));
        assert_eq!(foreign.status, "failed");
        assert!(foreign.detail.unwrap().contains("untrusted root"));
        let mut tampered = cert.clone();
        tampered.body.entitlements.customer_name = "Mallory".into();
        let t = verification(&tampered, Some(&root));
        assert_eq!(t.status, "failed");
        assert!(t.detail.unwrap().contains("tampered"));
    }
}
