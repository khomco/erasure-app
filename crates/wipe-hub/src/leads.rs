//! Sales leads (lead-capture story 1, CAP-1, CAP-2, CAP-4): people who
//! submitted the marketing site's early-access or contact form. A *sales
//! lead* is never the elected Lead station (CONTEXT §14).
//!
//! - [`validate`] checks a submission. The consent text must equal the hub's
//!   own text for the form ([`LeadKind::consent_text`]), which is pinned to
//!   `sites/www/src/data/consent.ts`; the client's copy is never the
//!   authority.
//! - [`submit`] keeps one lead per `lower(email)`: a new address inserts a
//!   lead with status `new`; a known one updates email (as typed), name and
//!   company, updates role and segment only when given, and never touches
//!   `status` or `unsubscribed_at`: an unsubscribe is a suppression that only
//!   a future confirmed opt-in may lift, so a resubmission from an
//!   unsubscribed address is stored but marked `suppressed` and gets no
//!   email. Every submission gets its own row with its consent text, time and
//!   the SHA-256 of a fresh unsubscribe token, and is audited as
//!   `lead.submitted`.
//! - [`unsubscribe`] marks the lead of any of its submissions' tokens
//!   unsubscribed, auditing `lead.unsubscribed` only when that changes.
//!
//! Audit rows about a sales lead (`lead.submitted`, `lead.unsubscribed`,
//! subject `lead:<id>`) carry ids and flags only, never a name, email,
//! company, message or peer IP, so erasing a lead later leaves no personal
//! data behind. The per-IP throttle rows (`lead.submit_locked`,
//! `lead.submit_lock_summary`, in `lead_routes`) are not about a lead: their
//! subject is the throttle key `lead-form:ip:<peer>`, they keep the peer IP
//! like every other throttle row, and they are never linked to a lead id.

use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::accounts::{self, Token};
use crate::audit::{self, AuditEntry, AuditError};

/// The segments the early-access form offers.
pub const SEGMENTS: [&str; 4] = ["itad", "enterprise-it", "government", "other"];
/// Longest message accepted, in characters, after trimming.
pub const MESSAGE_MAX_CHARS: usize = 5000;

/// The early-access form's consent statement (`sites/www/src/data/consent.ts`).
pub const EARLY_ACCESS_STATEMENT: &str =
    "I agree that these details are used only to follow up on early access.";
/// The contact form's consent statement (`sites/www/src/data/consent.ts`).
pub const CONTACT_STATEMENT: &str =
    "I agree that these details are used only to reply to this message.";
/// The sentence that follows each statement in the label, linking the notice.
pub const PRIVACY_SENTENCE: &str = "See the privacy notice.";
/// The early-access consent label's whole text: the text stored as consent.
pub const CONSENT_EARLY_ACCESS: &str =
    "I agree that these details are used only to follow up on early access. \
     See the privacy notice.";
/// The contact consent label's whole text: the text stored as consent.
pub const CONSENT_CONTACT: &str =
    "I agree that these details are used only to reply to this message. \
     See the privacy notice.";

pub const MSG_CONSENT_REQUIRED: &str = "consent is required";
pub const MSG_CONSENT_CHANGED: &str = "the consent text has changed; reload the page and try again";
pub const MSG_SUBMIT_LOCKED: &str = "too many submissions; try again later";
pub const MSG_BAD_UNSUBSCRIBE: &str = "this unsubscribe link is not valid";
pub const MSG_BAD_SEGMENT: &str = "segment must be one of itad, enterprise-it, government, other";
pub const MSG_CONTACT_ROLE: &str = "role is only asked on the early-access form";
pub const MSG_CONTACT_SEGMENT: &str = "segment is only asked on the early-access form";

/// Which form a submission came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LeadKind {
    EarlyAccess,
    Contact,
}

impl LeadKind {
    /// The stored `kind` value, as in the request body.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EarlyAccess => "early-access",
            Self::Contact => "contact",
        }
    }

    /// The hub's consent text for this form: the only text accepted.
    pub fn consent_text(self) -> &'static str {
        match self {
            Self::EarlyAccess => CONSENT_EARLY_ACCESS,
            Self::Contact => CONSENT_CONTACT,
        }
    }
}

/// `POST /api/leads`'s body. Missing text fields read as blank, so the
/// error names the field rather than the JSON shape.
#[derive(Debug, Deserialize)]
pub struct LeadRequest {
    pub kind: LeadKind,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub company: String,
    pub role: Option<String>,
    pub segment: Option<String>,
    pub message: Option<String>,
    #[serde(default)]
    pub consent: bool,
    #[serde(default)]
    pub consent_text: String,
    /// The honeypot: hidden from people, filled in by naive bots.
    pub website: Option<String>,
}

impl LeadRequest {
    /// Whether the honeypot field was filled in.
    pub fn is_honeypot(&self) -> bool {
        self.website
            .as_deref()
            .is_some_and(|w| !w.trim().is_empty())
    }
}

/// A validated submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLead {
    pub kind: LeadKind,
    pub name: String,
    pub email: String,
    pub company: String,
    /// Early access only.
    pub role: Option<String>,
    /// Early access only.
    pub segment: Option<String>,
    pub message: Option<String>,
}

fn non_blank(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// A trimmed message of at most [`MESSAGE_MAX_CHARS`] characters with no
/// control characters other than `\n`, `\r` and `\t`; `None` when blank.
pub fn validate_message(message: Option<&str>) -> Result<Option<String>, String> {
    let Some(m) = message.map(str::trim).filter(|m| !m.is_empty()) else {
        return Ok(None);
    };
    if m.chars().count() > MESSAGE_MAX_CHARS {
        return Err(format!(
            "message must be at most {MESSAGE_MAX_CHARS} characters"
        ));
    }
    if m.chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err("message must not contain control characters".into());
    }
    Ok(Some(m.to_string()))
}

/// Validate a submission; the error is the 400 message.
pub fn validate(req: &LeadRequest) -> Result<NewLead, String> {
    let name = accounts::validate_name("name", &req.name)?;
    let email = accounts::validate_email(&req.email)?;
    let company = accounts::validate_name("company", &req.company)?;
    let (role, segment) = match req.kind {
        LeadKind::EarlyAccess => {
            let role = accounts::validate_name("role", req.role.as_deref().unwrap_or(""))?;
            let segment = non_blank(&req.segment)
                .filter(|s| SEGMENTS.contains(s))
                .ok_or(MSG_BAD_SEGMENT)?;
            (Some(role), Some(segment.to_string()))
        }
        LeadKind::Contact => {
            if non_blank(&req.role).is_some() {
                return Err(MSG_CONTACT_ROLE.into());
            }
            if non_blank(&req.segment).is_some() {
                return Err(MSG_CONTACT_SEGMENT.into());
            }
            (None, None)
        }
    };
    let message = validate_message(req.message.as_deref())?;
    if req.kind == LeadKind::Contact && message.is_none() {
        return Err("message must not be blank".into());
    }
    if !req.consent {
        return Err(MSG_CONSENT_REQUIRED.into());
    }
    if req.consent_text != req.kind.consent_text() {
        return Err(MSG_CONSENT_CHANGED.into());
    }
    Ok(NewLead {
        kind: req.kind,
        name,
        email,
        company,
        role,
        segment,
        message,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum LeadStoreError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

/// What [`submit`] stored.
#[derive(Debug)]
pub struct Submitted {
    pub lead_id: Uuid,
    /// The address was new.
    pub created: bool,
    /// The lead is unsubscribed, so no confirmation email may be sent. The
    /// submission is stored; the unsubscribe stays.
    pub suppressed: bool,
    /// The raw unsubscribe token for this submission's email; only its
    /// hash is stored.
    pub unsubscribe_token: String,
}

/// The `subject` of a lead's audit rows.
pub fn subject(lead_id: Uuid) -> String {
    format!("lead:{lead_id}")
}

/// Upsert the lead, insert the submission and audit `lead.submitted`. Call
/// inside a transaction.
pub async fn submit(
    conn: &mut sqlx::PgConnection,
    new: &NewLead,
) -> Result<Submitted, LeadStoreError> {
    let (lead_id, created, suppressed) = upsert(conn, new).await?;
    let token = Token::generate();
    sqlx::query(
        "INSERT INTO lead_submissions \
             (lead_id, kind, message, consent_text, unsubscribe_token_hash) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(lead_id)
    .bind(new.kind.as_str())
    .bind(new.message.as_deref())
    .bind(new.kind.consent_text())
    .bind(&token.hash[..])
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: "anonymous",
            action: "lead.submitted",
            subject: &subject(lead_id),
            detail: json!({
                "kind": new.kind.as_str(),
                "created": created,
                "suppressed": suppressed,
            }),
        },
    )
    .await?;
    Ok(Submitted {
        lead_id,
        created,
        suppressed,
        unsubscribe_token: token.raw,
    })
}

/// `(id, created, suppressed)`. The existing row is locked before it is
/// read, so `suppressed` is exact under a concurrent unsubscribe; an insert
/// that loses a race to a concurrent one retries as an update. Never
/// changes `status` or `unsubscribed_at`.
async fn upsert(
    conn: &mut sqlx::PgConnection,
    new: &NewLead,
) -> Result<(Uuid, bool, bool), sqlx::Error> {
    for _ in 0..3 {
        let existing: Option<(Uuid, bool)> = sqlx::query_as(
            "SELECT id, unsubscribed_at IS NOT NULL FROM leads \
             WHERE lower(email) = lower($1) FOR UPDATE",
        )
        .bind(&new.email)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((id, unsubscribed)) = existing {
            sqlx::query(
                "UPDATE leads SET email = $2, name = $3, company = $4, \
                     role = coalesce($5, role), segment = coalesce($6, segment), \
                     updated_at = now() \
                 WHERE id = $1",
            )
            .bind(id)
            .bind(&new.email)
            .bind(&new.name)
            .bind(&new.company)
            .bind(new.role.as_deref())
            .bind(new.segment.as_deref())
            .execute(&mut *conn)
            .await?;
            return Ok((id, false, unsubscribed));
        }
        let inserted: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO leads (email, name, company, role, segment) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT ((lower(email))) DO NOTHING RETURNING id",
        )
        .bind(&new.email)
        .bind(&new.name)
        .bind(&new.company)
        .bind(new.role.as_deref())
        .bind(new.segment.as_deref())
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(id) = inserted {
            return Ok((id, true, false));
        }
    }
    Err(sqlx::Error::Protocol(
        "lead upsert kept racing; giving up after 3 tries".into(),
    ))
}

/// Unsubscribe the lead whose submission carries `token_hash`. `None` when
/// no submission does; otherwise the lead's id. Sets `unsubscribed_at` and
/// audits `lead.unsubscribed` only if it was not already set. Call inside a
/// transaction.
pub async fn unsubscribe(
    conn: &mut sqlx::PgConnection,
    token_hash: &[u8; 32],
) -> Result<Option<Uuid>, LeadStoreError> {
    let lead_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT lead_id FROM lead_submissions WHERE unsubscribe_token_hash = $1",
    )
    .bind(&token_hash[..])
    .fetch_optional(&mut *conn)
    .await?;
    let Some(lead_id) = lead_id else {
        return Ok(None);
    };
    // Concurrent unsubscribes serialize on the row lock, and the loser
    // re-checks `unsubscribed_at`, so the change is audited once.
    let changed = sqlx::query(
        "UPDATE leads SET unsubscribed_at = now(), updated_at = now() \
         WHERE id = $1 AND unsubscribed_at IS NULL",
    )
    .bind(lead_id)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;
    if changed {
        audit::record(
            &mut *conn,
            &AuditEntry {
                actor: "anonymous",
                action: "lead.unsubscribed",
                subject: &subject(lead_id),
                detail: json!({}),
            },
        )
        .await?;
    }
    Ok(Some(lead_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The marketing site's consent wording; the hub must accept exactly it.
    const SITE_CONSENT: &str = include_str!("../../../sites/www/src/data/consent.ts");

    #[test]
    fn consent_text_is_pinned_to_the_site() {
        assert_eq!(
            CONSENT_EARLY_ACCESS,
            format!("{EARLY_ACCESS_STATEMENT} {PRIVACY_SENTENCE}")
        );
        assert_eq!(
            CONSENT_CONTACT,
            format!("{CONTACT_STATEMENT} {PRIVACY_SENTENCE}")
        );
        for pinned in [
            format!("\"early-access\": \"{EARLY_ACCESS_STATEMENT}\","),
            format!("contact: \"{CONTACT_STATEMENT}\","),
            format!("export const PRIVACY_SENTENCE = \"{PRIVACY_SENTENCE}\";"),
            "return `${CONSENT_STATEMENTS[kind]} ${PRIVACY_SENTENCE}`;".to_string(),
        ] {
            assert!(
                SITE_CONSENT.contains(&pinned),
                "sites/www/src/data/consent.ts must contain {pinned}"
            );
        }
    }

    fn early_access() -> LeadRequest {
        LeadRequest {
            kind: LeadKind::EarlyAccess,
            name: " Ada Lovelace ".into(),
            email: " ada@example.com ".into(),
            company: "Engines Ltd".into(),
            role: Some("Ops lead".into()),
            segment: Some("itad".into()),
            message: None,
            consent: true,
            consent_text: CONSENT_EARLY_ACCESS.into(),
            website: None,
        }
    }

    fn contact() -> LeadRequest {
        LeadRequest {
            kind: LeadKind::Contact,
            role: None,
            segment: None,
            message: Some("  Hello there.\n\tThanks  ".into()),
            consent_text: CONSENT_CONTACT.into(),
            ..early_access()
        }
    }

    #[test]
    fn valid_submissions_are_trimmed() {
        let new = validate(&early_access()).unwrap();
        assert_eq!(new.name, "Ada Lovelace");
        assert_eq!(new.email, "ada@example.com");
        assert_eq!(new.role.as_deref(), Some("Ops lead"));
        assert_eq!(new.segment.as_deref(), Some("itad"));
        assert_eq!(new.message, None);
        let new = validate(&contact()).unwrap();
        assert_eq!(new.message.as_deref(), Some("Hello there.\n\tThanks"));
        assert_eq!((new.role, new.segment), (None, None));
        // Blank role and segment on a contact form are fine.
        let mut r = contact();
        r.role = Some("  ".into());
        r.segment = Some(String::new());
        assert!(validate(&r).is_ok());
    }

    #[test]
    fn every_rule_names_its_problem() {
        type Base = fn() -> LeadRequest;
        type Edit = fn(&mut LeadRequest);
        let cases: [(Base, Edit, &str); 14] = [
            (
                early_access,
                |r| r.name = " ".into(),
                "name must not be blank",
            ),
            (
                early_access,
                |r| r.email = "nope".into(),
                "email is not a valid",
            ),
            (
                early_access,
                |r| r.email = String::new(),
                "email must not be blank",
            ),
            (
                early_access,
                |r| r.company = String::new(),
                "company must not be blank",
            ),
            (early_access, |r| r.role = None, "role must not be blank"),
            (early_access, |r| r.segment = None, MSG_BAD_SEGMENT),
            (
                early_access,
                |r| r.segment = Some("ITAD".into()),
                MSG_BAD_SEGMENT,
            ),
            (early_access, |r| r.consent = false, MSG_CONSENT_REQUIRED),
            (
                early_access,
                |r| r.consent_text = CONSENT_CONTACT.into(),
                MSG_CONSENT_CHANGED,
            ),
            (
                early_access,
                |r| r.consent_text = String::new(),
                MSG_CONSENT_CHANGED,
            ),
            (contact, |r| r.message = None, "message must not be blank"),
            (contact, |r| r.role = Some("CTO".into()), MSG_CONTACT_ROLE),
            (
                contact,
                |r| r.segment = Some("itad".into()),
                MSG_CONTACT_SEGMENT,
            ),
            (
                contact,
                |r| r.message = Some("a\u{7}b".into()),
                "message must not contain control characters",
            ),
        ];
        for (base, edit, want) in cases {
            let mut r = base();
            edit(&mut r);
            let err = validate(&r).unwrap_err();
            assert!(err.contains(want), "{err:?} should contain {want:?}");
        }
    }

    #[test]
    fn message_length_is_bounded_in_characters() {
        let at = "é".repeat(MESSAGE_MAX_CHARS);
        assert_eq!(validate_message(Some(&at)).unwrap(), Some(at.clone()));
        let over = format!("{at}é");
        assert!(validate_message(Some(&over))
            .unwrap_err()
            .contains("at most 5000"));
        assert_eq!(validate_message(Some(" \n ")).unwrap(), None);
        assert_eq!(validate_message(None).unwrap(), None);
        assert!(validate_message(Some("a\u{0}b")).is_err());
        assert!(validate_message(Some("a\u{1b}[31m")).is_err());
        assert!(validate_message(Some("line\r\nline\tx")).is_ok());
    }

    #[test]
    fn honeypot() {
        let mut r = early_access();
        assert!(!r.is_honeypot());
        r.website = Some("  ".into());
        assert!(!r.is_honeypot());
        r.website = Some("https://spam.example".into());
        assert!(r.is_honeypot());
    }

    #[test]
    fn kinds_parse_from_the_form_names() {
        let k: LeadKind = serde_json::from_str("\"early-access\"").unwrap();
        assert_eq!(k, LeadKind::EarlyAccess);
        let k: LeadKind = serde_json::from_str("\"contact\"").unwrap();
        assert_eq!(k.as_str(), "contact");
        assert!(serde_json::from_str::<LeadKind>("\"early_access\"").is_err());
    }
}
