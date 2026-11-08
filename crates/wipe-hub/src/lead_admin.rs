//! The sales-lead admin data layer (lead-capture story 2, CAP-3 and the
//! erasure half of CAP-4): vendor admins list, view, work, export and erase
//! sales leads. The routes are in [`crate::admin_routes`].
//!
//! The rules of [`crate::admin`] apply: every function takes a
//! `&VendorAdmin` and a connection (a transaction in practice) and writes
//! its own audit row (actor `user:<admin id>`) in it, so no lead data is
//! returned, and no lead changed, without its row; a lookup that finds no
//! lead writes nothing. Timestamps are RFC 3339 UTC strings formatted in
//! SQL.
//!
//! - **No personal data in audit rows.** Rows written here hold ids, filter
//!   values, counts and status names only, never a name, email, company,
//!   role, message, consent text or note body, so an erased lead leaves no
//!   personal data in the append-only log. That is why the list filters on
//!   segment, status and submission date, never on free text.
//! - **Mutations lock the lead first** (`SELECT … FOR UPDATE`), so one that
//!   races an erasure finds no lead (a 404), never a foreign-key error.
//! - **Notes are append-only** and attributed to their author; they are
//!   deleted with the lead.
//! - **Every response carrying lead data is audited**, including a status
//!   "change" to the status the lead already has (audited as a view).
//! - **Erasure keeps nothing**: the lead, its submissions and its notes are
//!   deleted, as is the email-send throttle row keyed by its address, and no
//!   suppression record is kept (it would retain the address). A later
//!   submission from the same address is a new lead. Audit rows written
//!   before the erasure stay: the log is append-only.

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::admin::{audit_read, truncate, AdminReadError, VendorAdmin, RFC3339_UTC, SEARCH_LIMIT};
use crate::auth::{email_key, MAIL_TO_PREFIX};
use crate::leads::{self, SEGMENTS};

/// The statuses a vendor admin can give a lead (migration 0008's CHECK).
pub const STATUSES: [&str; 4] = ["new", "contacted", "qualified", "closed"];

/// The `segment` filter value matching leads with no segment (contact-only
/// leads).
pub const SEGMENT_NONE: &str = "none";

pub const MSG_BAD_STATUS: &str = "status must be one of new, contacted, qualified, closed";
pub const MSG_BLANK_NOTE: &str = "note must not be blank";

// ---------------------------------------------------------------------------
// Filters

/// The filters as sent: the list's query string, or the export's body.
/// Every field is optional; empty means any. An unknown key is refused, so a
/// misspelt filter never silently matches every lead.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterParams {
    pub segment: Option<String>,
    pub status: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Validated filters. `None` matches any lead. Serialized as the audit
/// row's `filter` (no personal data: fixed vocabularies and dates only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LeadFilter {
    /// One of [`SEGMENTS`] or [`SEGMENT_NONE`].
    pub segment: Option<String>,
    /// One of [`STATUSES`].
    pub status: Option<String>,
    /// `YYYY-MM-DD`, an inclusive UTC day.
    pub from: Option<String>,
    /// `YYYY-MM-DD`, an inclusive UTC day, never before `from`.
    pub to: Option<String>,
}

/// A filter value outside its vocabulary, a malformed or impossible date, or
/// `from` after `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadFilter;

fn given(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

/// Whether `s` is a real calendar date written `YYYY-MM-DD`.
pub fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap_or(0);
    let (year, month, day) = (num(0..4), num(5..7), num(8..10));
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    year >= 1 && (1..=days).contains(&day)
}

impl LeadFilter {
    pub fn parse(p: &FilterParams) -> Result<Self, BadFilter> {
        let segment = given(&p.segment);
        if segment.is_some_and(|s| s != SEGMENT_NONE && !SEGMENTS.contains(&s)) {
            return Err(BadFilter);
        }
        let status = given(&p.status);
        if status.is_some_and(|s| !STATUSES.contains(&s)) {
            return Err(BadFilter);
        }
        let (from, to) = (given(&p.from), given(&p.to));
        if from.is_some_and(|d| !is_iso_date(d)) || to.is_some_and(|d| !is_iso_date(d)) {
            return Err(BadFilter);
        }
        // Same fixed-width format, so string order is date order.
        if let (Some(f), Some(t)) = (from, to) {
            if f > t {
                return Err(BadFilter);
            }
        }
        Ok(Self {
            segment: segment.map(str::to_string),
            status: status.map(str::to_string),
            from: from.map(str::to_string),
            to: to.map(str::to_string),
        })
    }
}

/// A status from a request body, or [`MSG_BAD_STATUS`].
pub fn validate_status(s: &str) -> Result<&'static str, &'static str> {
    STATUSES
        .iter()
        .copied()
        .find(|v| *v == s)
        .ok_or(MSG_BAD_STATUS)
}

/// A trimmed, non-blank note body under [`leads::validate_message`]'s rules;
/// the error is the 400 message.
pub fn validate_note(body: &str) -> Result<String, String> {
    match leads::validate_message(Some(body)) {
        Ok(Some(b)) => Ok(b),
        Ok(None) => Err(MSG_BLANK_NOTE.into()),
        Err(e) => Err(e.replacen("message", "note", 1)),
    }
}

// ---------------------------------------------------------------------------
// Shapes

/// A sales lead as the admin area shows it (list rows, detail, export).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct AdminLead {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub company: String,
    pub role: Option<String>,
    pub segment: Option<String>,
    pub status: String,
    /// RFC 3339, UTC.
    pub created_at: String,
    pub updated_at: String,
    /// The newest submission's consent time.
    pub last_submitted_at: Option<String>,
    pub submission_count: i64,
    pub unsubscribed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadList {
    pub leads: Vec<AdminLead>,
    /// More than [`SEARCH_LIMIT`] matched.
    pub truncated: bool,
}

/// One form submission, with the exact consent text agreed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct AdminSubmission {
    pub id: Uuid,
    pub kind: String,
    pub message: Option<String>,
    pub consent_text: String,
    /// RFC 3339, UTC.
    pub consented_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteAuthor {
    pub id: Uuid,
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadNote {
    pub id: Uuid,
    pub body: String,
    /// RFC 3339, UTC.
    pub created_at: String,
    /// `None` once the author's account is removed.
    pub author: Option<NoteAuthor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadDetail {
    pub lead: AdminLead,
    /// Newest first.
    pub submissions: Vec<AdminSubmission>,
    /// Newest first.
    pub notes: Vec<LeadNote>,
}

/// An export: the UTC date it was taken (for the file name) and every
/// matching lead.
#[derive(Debug, Clone)]
pub struct LeadExport {
    pub date: String,
    pub leads: Vec<AdminLead>,
}

// ---------------------------------------------------------------------------
// Queries

/// The `AdminLead` columns over `leads l` and its submission summary.
fn lead_select() -> String {
    format!(
        "SELECT l.id, l.email, l.name, l.company, l.role, l.segment, l.status, \
                to_char(l.created_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS created_at, \
                to_char(l.updated_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS updated_at, \
                to_char(st.last AT TIME ZONE 'UTC', {RFC3339_UTC}) AS last_submitted_at, \
                st.n AS submission_count, \
                to_char(l.unsubscribed_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS unsubscribed_at \
         FROM leads l \
         CROSS JOIN LATERAL (SELECT max(s.consented_at) AS last, count(*) AS n \
                             FROM lead_submissions s WHERE s.lead_id = l.id) st"
    )
}

/// Leads matching `filter`, latest submission first, then id; at most
/// `limit` rows when given.
async fn matching(
    conn: &mut PgConnection,
    filter: &LeadFilter,
    limit: Option<i64>,
) -> Result<Vec<AdminLead>, sqlx::Error> {
    let sql = format!(
        "{} \
         WHERE ($1::text IS NULL \
                OR ($1 = '{SEGMENT_NONE}' AND l.segment IS NULL) OR l.segment = $1) \
           AND ($2::text IS NULL OR l.status = $2) \
           AND (($3::text IS NULL AND $4::text IS NULL) OR EXISTS ( \
                SELECT 1 FROM lead_submissions s WHERE s.lead_id = l.id \
                  AND ($3::text IS NULL \
                       OR s.consented_at >= ($3::text::date)::timestamp AT TIME ZONE 'UTC') \
                  AND ($4::text IS NULL \
                       OR s.consented_at < ($4::text::date + 1)::timestamp AT TIME ZONE 'UTC'))) \
         ORDER BY st.last DESC NULLS LAST, l.id \
         LIMIT $5",
        lead_select()
    );
    sqlx::query_as(&sql)
        .bind(filter.segment.as_deref())
        .bind(filter.status.as_deref())
        .bind(filter.from.as_deref())
        .bind(filter.to.as_deref())
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
}

async fn lead_by_id(conn: &mut PgConnection, id: Uuid) -> Result<Option<AdminLead>, sqlx::Error> {
    sqlx::query_as(&format!("{} WHERE l.id = $1", lead_select()))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
}

/// Lock the lead row; `Some(status)` when it exists.
async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT status FROM leads WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
}

fn lead_ids(leads: &[AdminLead]) -> Vec<Uuid> {
    leads.iter().map(|l| l.id).collect()
}

// ---------------------------------------------------------------------------
// Reads

/// At most [`SEARCH_LIMIT`] leads matching `filter`. Audited as
/// `admin.leads_listed` with the filter and the returned ids.
pub async fn list(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    filter: &LeadFilter,
) -> Result<LeadList, AdminReadError> {
    let rows = matching(conn, filter, Some(SEARCH_LIMIT + 1)).await?;
    let (leads, truncated) = truncate(rows);
    audit_read(
        conn,
        admin,
        "admin.leads_listed",
        "search:leads",
        json!({ "filter": filter, "lead_ids": lead_ids(&leads) }),
    )
    .await?;
    Ok(LeadList { leads, truncated })
}

/// Every lead matching `filter`, uncapped, in the list's order. Audited as
/// `admin.leads_exported` with the filter and every exported id.
pub async fn export_rows(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    filter: &LeadFilter,
) -> Result<LeadExport, AdminReadError> {
    let leads = matching(conn, filter, None).await?;
    let date: String = sqlx::query_scalar("SELECT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD')")
        .fetch_one(&mut *conn)
        .await?;
    audit_read(
        conn,
        admin,
        "admin.leads_exported",
        "search:leads",
        json!({ "filter": filter, "lead_ids": lead_ids(&leads) }),
    )
    .await?;
    Ok(LeadExport { date, leads })
}

/// One lead with its submissions and notes; `None` (no audit row) when
/// there is no such lead. Audited as `admin.lead_viewed`.
pub async fn detail(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
) -> Result<Option<LeadDetail>, AdminReadError> {
    let Some(lead) = lead_by_id(conn, id).await? else {
        return Ok(None);
    };
    let submissions: Vec<AdminSubmission> = sqlx::query_as(&format!(
        "SELECT id, kind, message, consent_text, \
                to_char(consented_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS consented_at \
         FROM lead_submissions WHERE lead_id = $1 \
         ORDER BY lead_submissions.consented_at DESC, id DESC"
    ))
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    type NoteRow = (
        Uuid,
        String,
        String,
        Option<Uuid>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<NoteRow> = sqlx::query_as(&format!(
        "SELECT n.id, n.body, to_char(n.created_at AT TIME ZONE 'UTC', {RFC3339_UTC}), \
                u.id, u.name, u.email \
         FROM lead_notes n LEFT JOIN users u ON u.id = n.author_id \
         WHERE n.lead_id = $1 \
         ORDER BY n.created_at DESC, n.id DESC"
    ))
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let notes = rows
        .into_iter()
        .map(|(id, body, created_at, author_id, name, email)| LeadNote {
            id,
            body,
            created_at,
            author: match (author_id, name, email) {
                (Some(id), Some(name), Some(email)) => Some(NoteAuthor { id, name, email }),
                _ => None,
            },
        })
        .collect();
    audit_read(
        conn,
        admin,
        "admin.lead_viewed",
        &leads::subject(id),
        json!({}),
    )
    .await?;
    Ok(Some(LeadDetail {
        lead,
        submissions,
        notes,
    }))
}

// ---------------------------------------------------------------------------
// Changes

/// Set the lead's status (already [`validate_status`]d). Only a change bumps
/// `updated_at` and is audited as `admin.lead_status_changed {from, to}`;
/// setting the current status again changes nothing but still returns the
/// lead, so it is audited as `admin.lead_viewed`. `None` (no audit row)
/// when there is no such lead.
pub async fn set_status(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
    status: &str,
) -> Result<Option<AdminLead>, AdminReadError> {
    let Some(current) = lock(conn, id).await? else {
        return Ok(None);
    };
    if current != status {
        sqlx::query("UPDATE leads SET status = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(&mut *conn)
            .await?;
        audit_read(
            conn,
            admin,
            "admin.lead_status_changed",
            &leads::subject(id),
            json!({ "from": current, "to": status }),
        )
        .await?;
    } else {
        audit_read(
            conn,
            admin,
            "admin.lead_viewed",
            &leads::subject(id),
            json!({}),
        )
        .await?;
    }
    Ok(lead_by_id(conn, id).await?)
}

/// Add a note (already [`validate_note`]d) by `admin`. Audited as
/// `admin.lead_note_added {note_id}`. `None` (no audit row) when there is
/// no such lead.
pub async fn add_note(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
    body: &str,
) -> Result<Option<LeadNote>, AdminReadError> {
    if lock(conn, id).await?.is_none() {
        return Ok(None);
    }
    let (note_id, created_at): (Uuid, String) = sqlx::query_as(&format!(
        "INSERT INTO lead_notes (lead_id, author_id, body) VALUES ($1, $2, $3) \
         RETURNING id, to_char(created_at AT TIME ZONE 'UTC', {RFC3339_UTC})"
    ))
    .bind(id)
    .bind(admin.user_id())
    .bind(body)
    .fetch_one(&mut *conn)
    .await?;
    let author: Option<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, name, email FROM users WHERE id = $1")
            .bind(admin.user_id())
            .fetch_optional(&mut *conn)
            .await?;
    audit_read(
        conn,
        admin,
        "admin.lead_note_added",
        &leads::subject(id),
        json!({ "note_id": note_id }),
    )
    .await?;
    Ok(Some(LeadNote {
        id: note_id,
        body: body.to_string(),
        created_at,
        author: author.map(|(id, name, email)| NoteAuthor { id, name, email }),
    }))
}

/// What an erasure removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Erased {
    pub submissions: i64,
    pub notes: i64,
}

/// Erase the lead: delete it with its submissions and notes (by cascade),
/// delete the email-send throttle row keyed by its address (it would
/// otherwise stay until some later request purges it, and a purged locked
/// row is audited with its key), and audit `admin.lead_erased {submissions,
/// notes}`. `None` (no audit row) when there is no such lead.
pub async fn erase(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
) -> Result<Option<Erased>, AdminReadError> {
    if lock(conn, id).await?.is_none() {
        return Ok(None);
    }
    let (submissions, notes): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM lead_submissions WHERE lead_id = $1), \
                (SELECT count(*) FROM lead_notes WHERE lead_id = $1)",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    let email: String = sqlx::query_scalar("DELETE FROM leads WHERE id = $1 RETURNING email")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    // The key `charge_mail` uses. A plain delete, not `throttle::clear`: its
    // lock summary is audited with the key, which names the address.
    let key = email_key(&mut *conn, MAIL_TO_PREFIX, &email).await?;
    sqlx::query("DELETE FROM auth_throttle WHERE key = $1")
        .bind(&key)
        .execute(&mut *conn)
        .await?;
    audit_read(
        conn,
        admin,
        "admin.lead_erased",
        &leads::subject(id),
        json!({ "submissions": submissions, "notes": notes }),
    )
    .await?;
    Ok(Some(Erased { submissions, notes }))
}

// ---------------------------------------------------------------------------
// CSV

/// The export's columns, in order: the [`AdminLead`] shape.
pub const CSV_COLUMNS: [&str; 12] = [
    "id",
    "email",
    "name",
    "company",
    "role",
    "segment",
    "status",
    "created_at",
    "updated_at",
    "last_submitted_at",
    "submission_count",
    "unsubscribed_at",
];

/// One RFC 4180 field. A value a spreadsheet would read as a formula
/// (leading `=`, `+`, `-`, `@`, TAB or CR; OWASP CSV Injection) is prefixed
/// with `'`; then a field holding `"`, `,`, CR or LF is quoted with `"`
/// doubled.
pub fn csv_field(value: &str) -> String {
    let mut v = String::with_capacity(value.len() + 2);
    if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        v.push('\'');
    }
    v.push_str(value);
    if v.contains(['"', ',', '\r', '\n']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v
    }
}

/// The UTF-8 byte-order mark the export starts with, so spreadsheet apps
/// (Excel in particular) read it as UTF-8 rather than a legacy code page.
pub const CSV_BOM: &str = "\u{feff}";

/// The export as RFC 4180 CSV after a UTF-8 [`CSV_BOM`]: a header row, then
/// one row per lead, CRLF line ends; absent values are empty fields.
pub fn to_csv(leads: &[AdminLead]) -> String {
    let mut out = String::from(CSV_BOM);
    let mut row = |fields: Vec<String>| {
        let cells: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    };
    row(CSV_COLUMNS.iter().map(|c| c.to_string()).collect());
    for l in leads {
        let opt = |v: &Option<String>| v.clone().unwrap_or_default();
        row(vec![
            l.id.to_string(),
            l.email.clone(),
            l.name.clone(),
            l.company.clone(),
            opt(&l.role),
            opt(&l.segment),
            l.status.clone(),
            l.created_at.clone(),
            l.updated_at.clone(),
            opt(&l.last_submitted_at),
            l.submission_count.to_string(),
            opt(&l.unsubscribed_at),
        ]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(segment: &str, status: &str, from: &str, to: &str) -> FilterParams {
        let o = |s: &str| Some(s.to_string());
        FilterParams {
            segment: o(segment),
            status: o(status),
            from: o(from),
            to: o(to),
        }
    }

    #[test]
    fn empty_and_missing_filters_match_anything() {
        assert_eq!(
            LeadFilter::parse(&FilterParams::default()),
            Ok(LeadFilter::default())
        );
        assert_eq!(
            LeadFilter::parse(&params("", "", "", "")),
            Ok(LeadFilter::default())
        );
    }

    #[test]
    fn good_filters_parse() {
        let f =
            LeadFilter::parse(&params("itad", "contacted", "2026-01-01", "2026-01-01")).unwrap();
        assert_eq!(f.segment.as_deref(), Some("itad"));
        assert_eq!(f.status.as_deref(), Some("contacted"));
        assert_eq!(
            (f.from.as_deref(), f.to.as_deref()),
            (Some("2026-01-01"), Some("2026-01-01"))
        );
        for s in SEGMENTS.iter().chain([&SEGMENT_NONE]) {
            assert!(LeadFilter::parse(&params(s, "", "", "")).is_ok(), "{s}");
        }
        for s in STATUSES {
            assert!(LeadFilter::parse(&params("", s, "", "")).is_ok(), "{s}");
        }
        assert!(LeadFilter::parse(&params("", "", "2024-02-29", "")).is_ok());
        assert!(LeadFilter::parse(&params("", "", "", "2000-02-29")).is_ok());
    }

    #[test]
    fn unknown_filter_keys_and_non_strings_are_refused() {
        for body in [
            r#"{"Segment":"itad"}"#,
            r#"{"q":"x"}"#,
            r#"{"status":1}"#,
            r#"{"from":["2026-01-01"]}"#,
        ] {
            assert!(
                serde_json::from_str::<FilterParams>(body).is_err(),
                "{body}"
            );
        }
        assert!(serde_json::from_str::<FilterParams>(r#"{"status":"new"}"#).is_ok());
    }

    #[test]
    fn bad_filters_are_refused() {
        for p in [
            params("x", "", "", ""),
            params("ITAD", "", "", ""),
            params("", "open", "", ""),
            params("", "New", "", ""),
            params("", "", "2026-13-01", ""),
            params("", "", "", "2026-02-30"),
            params("", "", "2025-02-29", ""),
            params("", "", "1900-02-29", ""),
            params("", "", "0000-01-01", ""),
            params("", "", "2026-1-01", ""),
            params("", "", "2026/01/01", ""),
            params("", "", "2026-01-01T00:00:00Z", ""),
            params("", "", "+026-01-01", ""),
            params("", "", "2026-01-02", "2026-01-01"),
            params("", "", " 2026-01-01", ""),
        ] {
            assert_eq!(LeadFilter::parse(&p), Err(BadFilter), "{p:?}");
        }
    }

    #[test]
    fn statuses_and_notes_validate() {
        assert_eq!(validate_status("qualified"), Ok("qualified"));
        assert_eq!(validate_status("won"), Err(MSG_BAD_STATUS));
        assert_eq!(validate_status(""), Err(MSG_BAD_STATUS));
        assert_eq!(
            validate_note("  called, follow up  ").unwrap(),
            "called, follow up"
        );
        assert_eq!(validate_note(" \n ").unwrap_err(), MSG_BLANK_NOTE);
        assert_eq!(
            validate_note("a\u{7}").unwrap_err(),
            "note must not contain control characters"
        );
        assert!(validate_note(&"x".repeat(5001))
            .unwrap_err()
            .starts_with("note must be at most"));
    }

    #[test]
    fn csv_fields_are_quoted_per_rfc_4180() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field(""), "");
        assert_eq!(csv_field("a,\"b\""), "\"a,\"\"b\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_field("cr\rhere"), "\"cr\rhere\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn formula_cells_are_neutralised() {
        assert_eq!(csv_field("=cmd()"), "'=cmd()");
        assert_eq!(csv_field("+1"), "'+1");
        assert_eq!(csv_field("-1"), "'-1");
        assert_eq!(csv_field("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(csv_field("\tx"), "'\tx");
        assert_eq!(csv_field("\rx"), "\"'\rx\"");
        assert_eq!(
            csv_field("=HYPERLINK(\"a\",\"b\")"),
            "\"'=HYPERLINK(\"\"a\"\",\"\"b\"\")\""
        );
        // Only a leading character counts.
        assert_eq!(csv_field("a=b"), "a=b");
    }

    fn lead(name: &str, company: &str) -> AdminLead {
        AdminLead {
            id: Uuid::nil(),
            email: "ada@example.com".into(),
            name: name.into(),
            company: company.into(),
            role: None,
            segment: Some("itad".into()),
            status: "new".into(),
            created_at: "2026-11-08T10:00:00Z".into(),
            updated_at: "2026-11-08T10:00:00Z".into(),
            last_submitted_at: Some("2026-11-08T10:00:00Z".into()),
            submission_count: 2,
            unsubscribed_at: None,
        }
    }

    #[test]
    fn csv_has_a_header_crlf_lines_and_the_lead_columns() {
        let csv = to_csv(&[lead("=cmd()", "a,\"b\"")]);
        assert_eq!(
            csv,
            "\u{feff}id,email,name,company,role,segment,status,created_at,updated_at,\
             last_submitted_at,submission_count,unsubscribed_at\r\n\
             00000000-0000-0000-0000-000000000000,ada@example.com,'=cmd(),\"a,\"\"b\"\"\",,itad,new,\
             2026-11-08T10:00:00Z,2026-11-08T10:00:00Z,2026-11-08T10:00:00Z,2,\r\n"
        );
        assert_eq!(
            to_csv(&[]),
            format!("{CSV_BOM}{}\r\n", CSV_COLUMNS.join(","))
        );
        assert!(csv.as_bytes().starts_with(&[0xef, 0xbb, 0xbf]));
    }
}
