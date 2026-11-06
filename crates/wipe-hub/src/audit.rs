//! The append-only audit log (ADR-0007 §6, ROADMAP A1.4).
//!
//! Callers must record every vendor-admin action, licence issuance,
//! revocation, order state change and download (ADR-0007 §6) in the
//! `audit_log` table with an actor, a time and a subject. [`record`] never
//! supplies a time, so `occurred_at` is the database's `now()`, in UTC.
//!
//! `now()` is the start time of the enclosing transaction, so rows written in
//! one transaction share it. Ids order inserts, but they can have gaps (from
//! rolled-back transactions) and need not match commit order.
//!
//! Append-only is enforced in Postgres, not here: triggers on `audit_log`
//! reject every `UPDATE`, `DELETE` and `TRUNCATE` with `audit_log is
//! append-only`, for every role including the table owner. The owner (the
//! hub's own role) or a superuser can still disable or drop the triggers; a
//! separate unprivileged runtime role is a deployment decision (ROADMAP
//! §0.5). This module adds no mutating path of its own: [`record`] is its
//! only write.
//!
//! [`record`] accepts any Postgres executor (`&PgPool`, `&mut PgConnection`,
//! or `&mut *tx`), so an action and its audit row can commit or roll back
//! together in one transaction.

use serde_json::Value;

/// One audit entry to append.
#[derive(Debug, Clone)]
pub struct AuditEntry<'a> {
    /// Who acted, e.g. `vendor_admin:alice`. Free-form until accounts exist.
    pub actor: &'a str,
    /// What happened, e.g. `licence.issue`.
    pub action: &'a str,
    /// What it happened to, e.g. `licence:42`.
    pub subject: &'a str,
    /// Further structured detail; must be a JSON object.
    pub detail: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    /// The named field is empty or whitespace only.
    #[error("audit entry field `{0}` must not be blank")]
    Blank(&'static str),
    /// `detail` is not a JSON object.
    #[error("audit entry detail must be a JSON object")]
    DetailNotObject,
    /// The database rejected or failed the insert.
    #[error("audit log write failed: {0}")]
    Db(#[from] sqlx::Error),
}

impl AuditEntry<'_> {
    /// The checks the database CHECK constraints also enforce, made here so
    /// the caller gets a typed error naming the field.
    fn validate(&self) -> Result<(), AuditError> {
        for (name, value) in [
            ("actor", self.actor),
            ("action", self.action),
            ("subject", self.subject),
        ] {
            if value.trim().is_empty() {
                return Err(AuditError::Blank(name));
            }
        }
        if !self.detail.is_object() {
            return Err(AuditError::DetailNotObject);
        }
        Ok(())
    }
}

/// Append `entry` to the audit log and return the new row's id.
///
/// Validates first, so an invalid entry never reaches the database.
pub async fn record<'e, E>(executor: E, entry: &AuditEntry<'_>) -> Result<i64, AuditError>
where
    E: sqlx::PgExecutor<'e>,
{
    entry.validate()?;
    let id = sqlx::query_scalar(
        "INSERT INTO audit_log (actor, action, subject, detail) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(entry.actor)
    .bind(entry.action)
    .bind(entry.subject)
    .bind(&entry.detail)
    .fetch_one(executor)
    .await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry<'a>(
        actor: &'a str,
        action: &'a str,
        subject: &'a str,
        detail: Value,
    ) -> AuditEntry<'a> {
        AuditEntry {
            actor,
            action,
            subject,
            detail,
        }
    }

    #[test]
    fn a_complete_entry_is_valid() {
        entry(
            "vendor_admin:alice",
            "licence.issue",
            "licence:42",
            json!({}),
        )
        .validate()
        .unwrap();
    }

    #[test]
    fn blank_fields_are_named() {
        for (e, field) in [
            (entry("", "a", "s", json!({})), "actor"),
            (entry(" \t", "a", "s", json!({})), "actor"),
            (entry("x", "", "s", json!({})), "action"),
            (entry("x", "a", "  ", json!({})), "subject"),
        ] {
            match e.validate() {
                Err(AuditError::Blank(f)) => assert_eq!(f, field),
                other => panic!("expected Blank({field}), got {other:?}"),
            }
        }
    }

    #[test]
    fn detail_must_be_an_object() {
        for d in [json!([1]), json!("x"), json!(null), json!(3)] {
            assert!(matches!(
                entry("x", "a", "s", d).validate(),
                Err(AuditError::DetailNotObject)
            ));
        }
    }
}
