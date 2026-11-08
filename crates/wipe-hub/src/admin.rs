//! The admin data layer (ADR-0007 §3, §6; portal-accounts story 5): the only
//! code in the hub that reads across organizations, plus `wipe-hub admin
//! create`, the only way a `vendor_admin` comes to exist.
//!
//! - **The vendor organization** is the one `organizations` row with
//!   `is_vendor` (migration 0007; a partial unique index allows at most one).
//!   Its name is [`VENDOR_ORGANIZATION_NAME`]. Sign-up and invitations never
//!   create or join it.
//! - **A vendor admin** is a user whose role is `vendor_admin`, whose
//!   organization is the vendor organization and whose email is verified,
//!   read live on every request by the extractor in
//!   [`crate::admin_routes`], the only code that builds a [`VendorAdmin`].
//! - **Every read is audited in the reader's transaction.** Each function
//!   here takes a `&VendorAdmin` and a connection (a transaction in
//!   practice) and records its own audit row (actor `user:<admin id>`)
//!   before it returns data, so no admin read can return data without its
//!   row; if the audit write fails, the caller's request is a 500 with no
//!   data. A lookup that finds nothing writes nothing, since nothing was seen.
//! - **Vendor staff are excluded** from every result: the vendor
//!   organization never appears, and no `vendor_admin` user is listed or
//!   counted, wherever they are.
//! - Searches match case-insensitive substrings with LIKE wildcards escaped
//!   ([`escape_like`]), return at most [`SEARCH_LIMIT`] rows and say whether
//!   more matched. Timestamps are RFC 3339 UTC strings formatted in SQL.
//! - Read-only: nothing here writes customer data. The sales-lead admin
//!   functions, which do write (status, notes, erasure), live in
//!   [`crate::lead_admin`] under the same rules.

use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::postgres::PgConnection;
use sqlx::Connection;
use uuid::Uuid;

use crate::accounts::{self, Organization, Role};
use crate::audit::{self, AuditEntry, AuditError};
use crate::auth::{actor, portal_link};
use crate::config::{parse_base_url, parse_mail_from, AdminCreateArgs};
use crate::mail::Mailer;

/// Name of the vendor organization, set when `admin create` first makes it.
pub const VENDOR_ORGANIZATION_NAME: &str = "Vendor staff";

/// Most rows a search returns; `truncated` says whether more matched.
pub const SEARCH_LIMIT: i64 = 100;

/// Longest search query, in characters, after trimming.
pub const QUERY_MAX_CHARS: usize = 200;

pub const MSG_BAD_QUERY: &str = "query must be at most 200 characters without control characters";

/// The audit actor of `wipe-hub admin create`.
pub const CLI_ACTOR: &str = "cli:admin-create";

/// How long `admin create` waits for its database connection.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// `to_char` format for RFC 3339 UTC, whole seconds (as `team.rs`).
pub(crate) const RFC3339_UTC: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// A signed-in vendor admin. Only the `/api/admin/*` extractor builds one,
/// after checking the role, the vendor organization and a verified email in
/// one query; holding one is what lets code call the functions below.
#[derive(Debug, Clone, Copy)]
pub struct VendorAdmin {
    pub(crate) user_id: Uuid,
}

impl VendorAdmin {
    pub fn user_id(&self) -> Uuid {
        self.user_id
    }
}

// ---------------------------------------------------------------------------
// Query handling

/// `%`, `_` and `\` escaped with `\`, so the query matches literally inside
/// `LIKE … ESCAPE '\'`.
pub fn escape_like(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The trimmed query, or [`MSG_BAD_QUERY`] when it has any control
/// character (anywhere, edges included) or is longer than
/// [`QUERY_MAX_CHARS`] after trimming. A missing query is empty, which
/// matches everything.
pub fn validate_query(q: Option<&str>) -> Result<String, String> {
    let q = q.unwrap_or("");
    if q.chars().any(char::is_control) {
        return Err(MSG_BAD_QUERY.into());
    }
    let q = q.trim();
    if q.chars().count() > QUERY_MAX_CHARS {
        return Err(MSG_BAD_QUERY.into());
    }
    Ok(q.to_string())
}

/// `LIKE` pattern matching `q` as a substring, lower-cased by the SQL.
fn contains_pattern(q: &str) -> String {
    format!("%{}%", escape_like(q))
}

// ---------------------------------------------------------------------------
// Shapes

/// An organization in search results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminOrganization {
    pub id: Uuid,
    pub name: String,
    /// RFC 3339, UTC.
    pub created_at: String,
    /// Customer users (`owner`s and `member`s).
    pub user_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationSearch {
    pub organizations: Vec<AdminOrganization>,
    pub truncated: bool,
}

/// A user in search results, with their organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminUser {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub role: Role,
    pub email_verified: bool,
    /// RFC 3339, UTC.
    pub created_at: String,
    pub organization: Organization,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSearch {
    pub users: Vec<AdminUser>,
    pub truncated: bool,
}

/// The organization on its detail page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminOrganizationRecord {
    pub id: Uuid,
    pub name: String,
    /// RFC 3339, UTC.
    pub created_at: String,
}

/// A user on an organization's detail page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminMember {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub role: Role,
    pub email_verified: bool,
    /// RFC 3339, UTC.
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminOrganizationDetail {
    pub organization: AdminOrganizationRecord,
    pub users: Vec<AdminMember>,
    /// `evaluation` or `stop`: what the organization's stations do when a
    /// licence lapses (ADR-0012). Read-only here: only the customer's owner
    /// changes it.
    pub lapse_policy: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AdminReadError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

fn decode_role(s: &str) -> Result<Role, sqlx::Error> {
    Role::from_str(s).map_err(|e| sqlx::Error::Decode(e.into()))
}

/// Append one admin read (or, from [`crate::lead_admin`], action) to the
/// audit log, as `admin`.
pub(crate) async fn audit_read(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    action: &str,
    subject: &str,
    detail: serde_json::Value,
) -> Result<(), AuditError> {
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &actor(admin.user_id),
            action,
            subject,
            detail,
        },
    )
    .await?;
    Ok(())
}

/// Split one extra row off a `LIMIT SEARCH_LIMIT + 1` result.
pub(crate) fn truncate<T>(mut rows: Vec<T>) -> (Vec<T>, bool) {
    let truncated = rows.len() as i64 > SEARCH_LIMIT;
    rows.truncate(SEARCH_LIMIT as usize);
    (rows, truncated)
}

// ---------------------------------------------------------------------------
// Reads

/// Customer organizations whose name, or any customer user's email or name,
/// contains `query` (case-insensitive), ordered by lower-cased name then id.
/// Audited as `admin.organizations_searched` with the query and the returned
/// ids. `query` must already be [`validate_query`]d.
pub async fn search_organizations(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    query: &str,
) -> Result<OrganizationSearch, AdminReadError> {
    let rows: Vec<(Uuid, String, String, i64)> = sqlx::query_as(&format!(
        "SELECT o.id, o.name, to_char(o.created_at AT TIME ZONE 'UTC', {RFC3339_UTC}), \
                (SELECT count(*) FROM users c \
                 WHERE c.organization_id = o.id AND c.role <> 'vendor_admin') \
         FROM organizations o \
         WHERE NOT o.is_vendor AND ( \
             lower(o.name) LIKE lower($1) ESCAPE '\\' \
             OR EXISTS (SELECT 1 FROM users u \
                        WHERE u.organization_id = o.id AND u.role <> 'vendor_admin' \
                          AND (lower(u.email) LIKE lower($1) ESCAPE '\\' \
                               OR lower(u.name) LIKE lower($1) ESCAPE '\\'))) \
         ORDER BY lower(o.name), o.id \
         LIMIT $2"
    ))
    .bind(contains_pattern(query))
    .bind(SEARCH_LIMIT + 1)
    .fetch_all(&mut *conn)
    .await?;
    let (rows, truncated) = truncate(rows);
    let organizations: Vec<AdminOrganization> = rows
        .into_iter()
        .map(|(id, name, created_at, user_count)| AdminOrganization {
            id,
            name,
            created_at,
            user_count,
        })
        .collect();
    let ids: Vec<Uuid> = organizations.iter().map(|o| o.id).collect();
    audit_read(
        conn,
        admin,
        "admin.organizations_searched",
        "search:organizations",
        json!({ "query": query, "organization_ids": ids }),
    )
    .await?;
    Ok(OrganizationSearch {
        organizations,
        truncated,
    })
}

/// Customer users whose email or name contains `query` (case-insensitive),
/// with their organization, ordered by lower-cased email then id. Audited
/// as `admin.users_searched` with the query and the returned ids. `query`
/// must already be [`validate_query`]d.
pub async fn search_users(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    query: &str,
) -> Result<UserSearch, AdminReadError> {
    type Row = (Uuid, String, String, String, bool, String, Uuid, String);
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT u.id, u.email, u.name, u.role, u.email_verified_at IS NOT NULL, \
                to_char(u.created_at AT TIME ZONE 'UTC', {RFC3339_UTC}), o.id, o.name \
         FROM users u JOIN organizations o ON o.id = u.organization_id \
         WHERE NOT o.is_vendor AND u.role <> 'vendor_admin' \
           AND (lower(u.email) LIKE lower($1) ESCAPE '\\' \
                OR lower(u.name) LIKE lower($1) ESCAPE '\\') \
         ORDER BY lower(u.email), u.id \
         LIMIT $2"
    ))
    .bind(contains_pattern(query))
    .bind(SEARCH_LIMIT + 1)
    .fetch_all(&mut *conn)
    .await?;
    let (rows, truncated) = truncate(rows);
    let users = rows
        .into_iter()
        .map(
            |(id, email, name, role, email_verified, created_at, org_id, org_name)| {
                Ok(AdminUser {
                    id,
                    email,
                    name,
                    role: decode_role(&role)?,
                    email_verified,
                    created_at,
                    organization: Organization {
                        id: org_id,
                        name: org_name,
                    },
                })
            },
        )
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let ids: Vec<Uuid> = users.iter().map(|u| u.id).collect();
    audit_read(
        conn,
        admin,
        "admin.users_searched",
        "search:users",
        json!({ "query": query, "user_ids": ids }),
    )
    .await?;
    Ok(UserSearch { users, truncated })
}

/// One customer organization and its customer users, ordered by name then
/// email. `None` (and no audit row) for an unknown id or the vendor
/// organization; otherwise audited as `admin.organization_viewed` with the
/// returned user ids.
pub async fn organization_detail(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    organization_id: Uuid,
) -> Result<Option<AdminOrganizationDetail>, AdminReadError> {
    let org: Option<(Uuid, String, String, String)> = sqlx::query_as(&format!(
        "SELECT id, name, to_char(created_at AT TIME ZONE 'UTC', {RFC3339_UTC}), lapse_policy \
         FROM organizations WHERE id = $1 AND NOT is_vendor"
    ))
    .bind(organization_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, name, created_at, lapse_policy)) = org else {
        return Ok(None);
    };
    let rows: Vec<(Uuid, String, String, String, bool, String)> = sqlx::query_as(&format!(
        "SELECT id, email, name, role, email_verified_at IS NOT NULL, \
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339_UTC}) \
         FROM users WHERE organization_id = $1 AND role <> 'vendor_admin' \
         ORDER BY name, lower(email), id"
    ))
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let users = rows
        .into_iter()
        .map(|(id, email, name, role, email_verified, created_at)| {
            Ok(AdminMember {
                id,
                email,
                name,
                role: decode_role(&role)?,
                email_verified,
                created_at,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let ids: Vec<Uuid> = users.iter().map(|u| u.id).collect();
    audit_read(
        conn,
        admin,
        "admin.organization_viewed",
        &format!("organization:{id}"),
        json!({ "user_ids": ids }),
    )
    .await?;
    Ok(Some(AdminOrganizationDetail {
        organization: AdminOrganizationRecord {
            id,
            name,
            created_at,
        },
        users,
        lapse_policy,
    }))
}

// ---------------------------------------------------------------------------
// `wipe-hub admin create`

/// What `admin create` did.
#[derive(Debug, Clone)]
pub struct CreatedAdmin {
    pub user_id: Uuid,
    pub email: String,
    pub organization_id: Uuid,
    pub vendor_organization_created: bool,
    /// `Err(reason)` when the set-password email could not be sent.
    pub email_sent: Result<(), String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateAdminError {
    /// Bad `--email`, `--name`, `--mail-from` or `--public-base-url`.
    #[error("{0}")]
    Invalid(String),
    #[error("an account with this email already exists")]
    CustomerEmail,
    #[error("{0} is already a vendor admin; use Forgot password on the portal")]
    AlreadyAdmin(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<sqlx::Error> for CreateAdminError {
    fn from(e: sqlx::Error) -> Self {
        Self::Other(e.into())
    }
}

impl From<AuditError> for CreateAdminError {
    fn from(e: AuditError) -> Self {
        Self::Other(e.into())
    }
}

/// The OS user running the command, for the audit row only: `$USER`, else
/// `$USERNAME`, else `None`. Informational; never trusted.
pub(crate) fn os_user() -> Option<String> {
    ["USER", "USERNAME"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
}

/// The part of `email` before its last `@`.
fn local_part(email: &str) -> &str {
    email.rsplit_once('@').map_or(email, |(local, _)| local)
}

/// Whether the address belongs to a vendor admin (role and vendor
/// organization) rather than to a customer.
async fn is_vendor_admin_email(
    conn: &mut PgConnection,
    email: &str,
) -> Result<Option<bool>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT u.role = 'vendor_admin' AND o.is_vendor \
         FROM users u JOIN organizations o ON o.id = u.organization_id \
         WHERE lower(u.email) = lower($1)",
    )
    .bind(email)
    .fetch_optional(&mut *conn)
    .await
}

fn duplicate_error(email: &str, is_admin: bool) -> CreateAdminError {
    if is_admin {
        CreateAdminError::AlreadyAdmin(email.to_string())
    } else {
        CreateAdminError::CustomerEmail
    }
}

/// `wipe-hub admin create`: validate, apply migrations, then in one
/// transaction get or create the vendor organization, insert the
/// `vendor_admin` (unusable password, unverified email), issue a password
/// reset token and audit `admin.created`; after commit, email the reset
/// link. An SMTP failure is reported in [`CreatedAdmin::email_sent`], not as
/// an error. Any error leaves the database unchanged. The raw token is
/// never printed, logged or audited.
pub async fn create_vendor_admin(args: AdminCreateArgs) -> Result<CreatedAdmin, CreateAdminError> {
    let email = accounts::validate_email(&args.email)
        .map_err(|e| CreateAdminError::Invalid(format!("--email: {e}")))?;
    let name = match &args.name {
        Some(n) => accounts::validate_name("name", n),
        None => accounts::validate_name("name", local_part(&email)),
    }
    .map_err(|e| CreateAdminError::Invalid(format!("--name: {e}")))?;
    let mail_from =
        parse_mail_from(&args.mail_from).map_err(|e| CreateAdminError::Invalid(e.to_string()))?;
    let base = parse_base_url(&args.public_base_url)
        .map_err(|e| CreateAdminError::Invalid(e.to_string()))?;

    crate::db::migrate_url(&args.database_url).await?;

    let mut conn = match tokio::time::timeout(
        CONNECT_TIMEOUT,
        PgConnection::connect(&args.database_url),
    )
    .await
    {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            return Err(anyhow::anyhow!(
                "cannot connect to {}: {e}",
                crate::db::redacted(&args.database_url)
            )
            .into())
        }
        Err(_) => {
            return Err(anyhow::anyhow!(
                "cannot connect to {}: no connection within {}s",
                crate::db::redacted(&args.database_url),
                CONNECT_TIMEOUT.as_secs()
            )
            .into())
        }
    };

    let result = insert_admin(&mut conn, &email, &name).await;
    let (created, token) = match result {
        Ok(v) => v,
        // Lost a race with a concurrent insert of the same address: the
        // transaction rolled back; classify it on a fresh statement.
        Err(InsertError::Duplicate) => {
            let is_admin = is_vendor_admin_email(&mut conn, &email)
                .await?
                .unwrap_or(false);
            let _ = conn.close().await;
            return Err(duplicate_error(&email, is_admin));
        }
        Err(InsertError::Other(e)) => {
            let _ = conn.close().await;
            return Err(e);
        }
    };
    let _ = conn.close().await;

    let link = portal_link(&base, "reset-password", &token);
    let mailer = Mailer::new(&args.smtp_host, args.smtp_port, mail_from);
    let email_sent = mailer
        .send_password_reset(&email, &link)
        .await
        .map_err(|e| e.to_string());
    Ok(CreatedAdmin {
        email_sent,
        ..created
    })
}

enum InsertError {
    Duplicate,
    Other(CreateAdminError),
}

impl From<sqlx::Error> for InsertError {
    fn from(e: sqlx::Error) -> Self {
        Self::Other(e.into())
    }
}

impl From<AuditError> for InsertError {
    fn from(e: AuditError) -> Self {
        Self::Other(e.into())
    }
}

/// The `admin create` transaction; returns the admin and the raw token.
async fn insert_admin(
    conn: &mut PgConnection,
    email: &str,
    name: &str,
) -> Result<(CreatedAdmin, String), InsertError> {
    let mut tx = conn.begin().await?;
    if let Some(is_admin) = is_vendor_admin_email(&mut tx, email).await? {
        return Err(InsertError::Other(duplicate_error(email, is_admin)));
    }

    // A concurrent run waits on the index entry, then does nothing; the
    // SELECT (a new snapshot under READ COMMITTED) then sees its row.
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO organizations (name, is_vendor) VALUES ($1, true) \
         ON CONFLICT ((true)) WHERE is_vendor DO NOTHING RETURNING id",
    )
    .bind(VENDOR_ORGANIZATION_NAME)
    .fetch_optional(&mut *tx)
    .await?;
    let (organization_id, vendor_organization_created) = match inserted {
        Some(id) => (id, true),
        None => {
            let id: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE is_vendor")
                .fetch_one(&mut *tx)
                .await?;
            (id, false)
        }
    };

    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (organization_id, email, name, password_hash, role) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(organization_id)
    .bind(email)
    .bind(name)
    .bind(accounts::UNUSABLE_PASSWORD_HASH)
    .bind(Role::VendorAdmin.as_str())
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("users_email_lower_key") => {
            InsertError::Duplicate
        }
        _ => InsertError::from(e),
    })?;

    let token = accounts::create_token(
        &mut *tx,
        user_id,
        accounts::PURPOSE_PASSWORD_RESET,
        accounts::PASSWORD_RESET_TTL,
    )
    .await?;
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: CLI_ACTOR,
            action: "admin.created",
            subject: &actor(user_id),
            detail: json!({
                "email": email,
                "organization_id": organization_id,
                "vendor_organization_created": vendor_organization_created,
                "os_user": os_user(),
            }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok((
        CreatedAdmin {
            user_id,
            email: email.to_string(),
            organization_id,
            vendor_organization_created,
            email_sent: Ok(()),
        },
        token,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_wildcards_are_escaped() {
        assert_eq!(escape_like("acme"), "acme");
        assert_eq!(escape_like("%"), "\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
        assert_eq!(escape_like("c:\\d"), "c:\\\\d");
        assert_eq!(escape_like("100%_\\"), "100\\%\\_\\\\");
        assert_eq!(escape_like("Zoë"), "Zoë");
        assert_eq!(contains_pattern("x%"), "%x\\%%");
        assert_eq!(contains_pattern(""), "%%");
    }

    #[test]
    fn queries_are_trimmed_and_bounded() {
        assert_eq!(validate_query(None).unwrap(), "");
        assert_eq!(validate_query(Some("  acme ")).unwrap(), "acme");
        assert_eq!(validate_query(Some(&"a".repeat(200))).unwrap().len(), 200);
        assert_eq!(
            validate_query(Some(&format!("  {}  ", "é".repeat(200))))
                .unwrap()
                .chars()
                .count(),
            200
        );
        for bad in [
            "a".repeat(201),
            "ac\nme".into(),
            "acme\n".into(),
            "\tacme".into(),
            "a\u{0}".into(),
            "a\u{85}b".into(),
        ] {
            assert_eq!(
                validate_query(Some(&bad)).unwrap_err(),
                MSG_BAD_QUERY,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn local_part_is_before_the_last_at() {
        assert_eq!(local_part("ada@example.com"), "ada");
        assert_eq!(local_part("\"a@b\"@example.com"), "\"a@b\"");
    }

    #[test]
    fn truncation_keeps_the_limit() {
        let (rows, t) = truncate((0..101).collect::<Vec<_>>());
        assert_eq!((rows.len(), t), (100, true));
        let (rows, t) = truncate((0..100).collect::<Vec<_>>());
        assert_eq!((rows.len(), t), (100, false));
    }

    #[test]
    fn duplicates_name_the_right_problem() {
        assert_eq!(
            duplicate_error("a@b.com", false).to_string(),
            "an account with this email already exists"
        );
        assert_eq!(
            duplicate_error("a@b.com", true).to_string(),
            "a@b.com is already a vendor admin; use Forgot password on the portal"
        );
    }
}
