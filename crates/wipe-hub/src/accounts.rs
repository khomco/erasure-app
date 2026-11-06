//! Accounts data layer (ADR-0007 §3): organizations, users, emailed tokens
//! and server-side sessions.
//!
//! - Passwords are Argon2id PHC strings, hashed and verified on the blocking
//!   pool ([`hash_password`], [`verify_password`]).
//! - Session and email tokens are 32 bytes from the OS RNG, base64url (no
//!   padding) in transit; only their SHA-256 is stored ([`Token`]).
//! - Emails are unique case-insensitively and looked up by `lower(email)`.
//! - Customer-side reads take the signed-in user's organization id
//!   ([`account`]), so scoping lives here, not only in handlers.
//!
//! Functions take any Postgres executor so callers can put a write and its
//! audit row in one transaction.

use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use argon2::password_hash::rand_core::OsRng as SaltRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Inclusive password length bounds, in characters.
pub const PASSWORD_MIN_CHARS: usize = 12;
pub const PASSWORD_MAX_CHARS: usize = 1024;
/// Upper bound on a user's or organization's name, in characters.
pub const NAME_MAX_CHARS: usize = 200;
/// Upper bound on an email address, in bytes (RFC 5321 path limit, in octets).
pub const EMAIL_MAX_CHARS: usize = 254;

/// How long a session (row and cookie) lasts.
pub const SESSION_TTL: Duration = Duration::from_secs(14 * 24 * 60 * 60);
/// How long an email-verification token is valid.
pub const VERIFY_EMAIL_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Purpose of an `auth_tokens` row (the migration's CHECK list).
pub const PURPOSE_VERIFY_EMAIL: &str = "verify_email";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Member,
    VendorAdmin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Member => "member",
            Role::VendorAdmin => "vendor_admin",
        }
    }
}

impl FromStr for Role {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(Role::Owner),
            "member" => Ok(Role::Member),
            "vendor_admin" => Ok(Role::VendorAdmin),
            other => Err(format!("unknown role {other:?}")),
        }
    }
}

/// A user as the API shows it. Never carries the password hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub role: Role,
    pub email_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
}

/// A user with their organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub user: User,
    pub organization: Organization,
}

// ---------------------------------------------------------------------------
// Validation

/// A validated sign-up: trimmed email, name and organization name.
#[derive(Clone)]
pub struct NewAccount {
    pub email: String,
    pub password: String,
    pub name: String,
    pub organization_name: String,
}

/// Validate sign-up input; the error names the problem.
pub fn validate_sign_up(
    email: &str,
    password: &str,
    name: &str,
    organization_name: &str,
) -> Result<NewAccount, String> {
    let email = validate_email(email)?;
    validate_password(password)?;
    let name = validate_name("name", name)?;
    let organization_name = validate_name("organization_name", organization_name)?;
    Ok(NewAccount {
        email,
        password: password.to_string(),
        name,
        organization_name,
    })
}

/// A trimmed, syntactically valid email address.
pub fn validate_email(email: &str) -> Result<String, String> {
    let email = email.trim();
    if email.is_empty() {
        return Err("email must not be blank".into());
    }
    // RFC 5321 counts octets, not characters.
    if email.len() > EMAIL_MAX_CHARS {
        return Err(format!("email must be at most {EMAIL_MAX_CHARS} bytes"));
    }
    // The address must be one the mailer can send to.
    match lettre::Address::from_str(email) {
        Ok(addr) if addr.domain().contains('.') || addr.domain() == "localhost" => {
            Ok(email.to_string())
        }
        _ => Err("email is not a valid email address".into()),
    }
}

pub fn validate_password(password: &str) -> Result<(), String> {
    let n = password.chars().count();
    if n < PASSWORD_MIN_CHARS {
        return Err(format!(
            "password must be at least {PASSWORD_MIN_CHARS} characters"
        ));
    }
    if n > PASSWORD_MAX_CHARS {
        return Err(format!(
            "password must be at most {PASSWORD_MAX_CHARS} characters"
        ));
    }
    Ok(())
}

fn validate_name(field: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{field} must not be blank"));
    }
    if value.chars().count() > NAME_MAX_CHARS {
        return Err(format!(
            "{field} must be at most {NAME_MAX_CHARS} characters"
        ));
    }
    Ok(value.to_string())
}

// ---------------------------------------------------------------------------
// Passwords

/// Argon2id PHC string for `password`, computed on the blocking pool.
pub async fn hash_password(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || hash_password_blocking(&password)).await?
}

fn hash_password_blocking(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut SaltRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hashing password: {e}"))
}

/// Whether `password` matches `stored`. With no stored hash (unknown email)
/// it still runs one Argon2 verification against a fixed dummy hash and
/// returns `false`, so timing does not reveal whether the email exists.
pub async fn verify_password(password: String, stored: Option<String>) -> bool {
    let joined = tokio::task::spawn_blocking(move || match stored {
        Some(hash) => verify_blocking(&password, &hash),
        None => {
            let _ = verify_blocking(&password, dummy_hash());
            false
        }
    })
    .await;
    joined.unwrap_or(false)
}

fn verify_blocking(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Compute the dummy hash now, so the first unknown-email sign-in does not
/// pay for building it (which would make its timing stand out). Call once at
/// start-up.
pub fn prepare_dummy_hash() {
    let _ = dummy_hash();
}

fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        hash_password_blocking("not a real password: timing equaliser")
            .expect("hashing the dummy password")
    })
}

// ---------------------------------------------------------------------------
// Tokens

/// A fresh secret: `raw` goes to the user (cookie or email), `hash` to the
/// database.
pub struct Token {
    pub raw: String,
    pub hash: [u8; 32],
}

impl Token {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self {
            raw: URL_SAFE_NO_PAD.encode(bytes),
            hash: Sha256::digest(bytes).into(),
        }
    }
}

/// SHA-256 of a raw token, or `None` if it is not 32 base64url bytes.
pub fn hash_token(raw: &str) -> Option<[u8; 32]> {
    let bytes = URL_SAFE_NO_PAD.decode(raw.trim()).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    Some(Sha256::digest(bytes).into())
}

// ---------------------------------------------------------------------------
// Queries

#[derive(Debug, thiserror::Error)]
pub enum CreateAccountError {
    #[error("an account with this email already exists")]
    DuplicateEmail,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Create an organization and its `owner` (unverified). Call inside a
/// transaction: a duplicate email aborts it, leaving no organization.
pub async fn create_account(
    conn: &mut sqlx::PgConnection,
    new: &NewAccount,
    password_hash: &str,
) -> Result<Account, CreateAccountError> {
    let org_id: Uuid =
        sqlx::query_scalar("INSERT INTO organizations (name) VALUES ($1) RETURNING id")
            .bind(&new.organization_name)
            .fetch_one(&mut *conn)
            .await?;
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (organization_id, email, name, password_hash, role) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(org_id)
    .bind(&new.email)
    .bind(&new.name)
    .bind(password_hash)
    .bind(Role::Owner.as_str())
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("users_email_lower_key") => {
            CreateAccountError::DuplicateEmail
        }
        _ => CreateAccountError::Db(e),
    })?;
    Ok(Account {
        user: User {
            id: user_id,
            email: new.email.clone(),
            name: new.name.clone(),
            role: Role::Owner,
            email_verified: false,
        },
        organization: Organization {
            id: org_id,
            name: new.organization_name.clone(),
        },
    })
}

/// What sign-in needs about a user, looked up by email (any case).
pub struct SignInCandidate {
    pub user_id: Uuid,
    pub organization_id: Uuid,
    pub password_hash: String,
}

pub async fn find_sign_in_candidate<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    email: &str,
) -> Result<Option<SignInCandidate>, sqlx::Error> {
    let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT id, organization_id, password_hash FROM users WHERE lower(email) = lower($1)",
    )
    .bind(email.trim())
    .fetch_optional(executor)
    .await?;
    Ok(row.map(
        |(user_id, organization_id, password_hash)| SignInCandidate {
            user_id,
            organization_id,
            password_hash,
        },
    ))
}

/// The signed-in user's own account, scoped to their organization.
pub async fn account<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    user_id: Uuid,
) -> Result<Option<Account>, sqlx::Error> {
    let row: Option<(Uuid, String, String, String, bool, Uuid, String)> = sqlx::query_as(
        "SELECT u.id, u.email, u.name, u.role, u.email_verified_at IS NOT NULL, o.id, o.name \
         FROM users u JOIN organizations o ON o.id = u.organization_id \
         WHERE u.organization_id = $1 AND u.id = $2",
    )
    .bind(organization_id)
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    row.map(
        |(id, email, name, role, email_verified, org_id, org_name)| {
            let role = Role::from_str(&role).map_err(|e| sqlx::Error::Decode(e.into()))?;
            Ok(Account {
                user: User {
                    id,
                    email,
                    name,
                    role,
                    email_verified,
                },
                organization: Organization {
                    id: org_id,
                    name: org_name,
                },
            })
        },
    )
    .transpose()
}

/// Issue an email-verification token for `user_id`; returns the raw token.
pub async fn create_verification_token<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    user_id: Uuid,
) -> Result<String, sqlx::Error> {
    let token = Token::generate();
    sqlx::query(
        "INSERT INTO auth_tokens (token_hash, user_id, purpose, expires_at) \
         VALUES ($1, $2, $3, now() + make_interval(secs => $4))",
    )
    .bind(&token.hash[..])
    .bind(user_id)
    .bind(PURPOSE_VERIFY_EMAIL)
    .bind(VERIFY_EMAIL_TTL.as_secs_f64())
    .execute(executor)
    .await?;
    Ok(token.raw)
}

/// Consume an unused, unexpired verification token and mark its user's email
/// verified. Returns the user's id, or `None` for an unknown, used or expired
/// token (nothing changes then). Call inside a transaction.
pub async fn consume_verification_token(
    conn: &mut sqlx::PgConnection,
    raw: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let Some(hash) = hash_token(raw) else {
        return Ok(None);
    };
    let user_id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE auth_tokens SET used_at = now() \
         WHERE token_hash = $1 AND purpose = $2 AND used_at IS NULL AND expires_at > now() \
         RETURNING user_id",
    )
    .bind(&hash[..])
    .bind(PURPOSE_VERIFY_EMAIL)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(id) = user_id {
        sqlx::query(
            "UPDATE users SET email_verified_at = coalesce(email_verified_at, now()) \
             WHERE id = $1",
        )
        .bind(id)
        .execute(&mut *conn)
        .await?;
        // One verification per user: retire any other outstanding links
        // (e.g. from a resend) so they cannot verify or audit again.
        sqlx::query(
            "UPDATE auth_tokens SET used_at = now() \
             WHERE user_id = $1 AND purpose = $2 AND used_at IS NULL",
        )
        .bind(id)
        .bind(PURPOSE_VERIFY_EMAIL)
        .execute(&mut *conn)
        .await?;
    }
    Ok(user_id)
}

/// Start a session for `user_id`; returns the raw token for the cookie.
pub async fn create_session<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    user_id: Uuid,
) -> Result<String, sqlx::Error> {
    let token = Token::generate();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, expires_at) \
         VALUES ($1, $2, now() + make_interval(secs => $3))",
    )
    .bind(&token.hash[..])
    .bind(user_id)
    .bind(SESSION_TTL.as_secs_f64())
    .execute(executor)
    .await?;
    Ok(token.raw)
}

/// Who a live session belongs to: `(user_id, organization_id)`.
pub async fn session_user<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    raw: &str,
) -> Result<Option<(Uuid, Uuid)>, sqlx::Error> {
    let Some(hash) = hash_token(raw) else {
        return Ok(None);
    };
    sqlx::query_as(
        "SELECT u.id, u.organization_id FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.token_hash = $1 AND s.expires_at > now()",
    )
    .bind(&hash[..])
    .fetch_optional(executor)
    .await
}

/// End a session; returns its user's id if it existed.
pub async fn delete_session<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    raw: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let Some(hash) = hash_token(raw) else {
        return Ok(None);
    };
    sqlx::query_scalar("DELETE FROM sessions WHERE token_hash = $1 RETURNING user_id")
        .bind(&hash[..])
        .fetch_optional(executor)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_32_bytes_base64url_and_hash_round_trips() {
        let t = Token::generate();
        assert_eq!(t.raw.len(), 43);
        assert!(t
            .raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_eq!(hash_token(&t.raw), Some(t.hash));
        assert_ne!(Token::generate().raw, t.raw);
    }

    #[test]
    fn malformed_tokens_have_no_hash() {
        for raw in ["", "garbage!", "abc", &URL_SAFE_NO_PAD.encode([0u8; 16])] {
            assert_eq!(hash_token(raw), None, "{raw}");
        }
    }

    #[test]
    fn password_bounds() {
        assert!(validate_password(&"a".repeat(11)).is_err());
        assert!(validate_password(&"a".repeat(12)).is_ok());
        assert!(validate_password(&"é".repeat(12)).is_ok());
        assert!(validate_password(&"a".repeat(1024)).is_ok());
        assert!(validate_password(&"a".repeat(1025)).is_err());
    }

    #[test]
    fn emails() {
        // The limit is in bytes: 127 two-byte characters pass by count but not by length.
        let wide = format!("{}@example.com", "é".repeat(127));
        assert!(wide.chars().count() <= EMAIL_MAX_CHARS && wide.len() > EMAIL_MAX_CHARS);
        assert!(validate_email(&wide).is_err());
        assert_eq!(
            validate_email("  Alice@Example.com ").unwrap(),
            "Alice@Example.com"
        );
        for bad in [
            "",
            "  ",
            "alice",
            "alice@",
            "@example.com",
            "a b@example.com",
            "a@nodot",
        ] {
            assert!(validate_email(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn names_are_trimmed_and_non_blank() {
        assert_eq!(validate_name("name", "  Ada ").unwrap(), "Ada");
        let err = validate_name("organization_name", " \t").unwrap_err();
        assert!(err.contains("organization_name"), "{err}");
    }

    #[test]
    fn hashes_are_argon2id_and_verify() {
        let h = hash_password_blocking("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$"), "{h}");
        assert!(verify_blocking("correct horse battery", &h));
        assert!(!verify_blocking("wrong horse battery", &h));
        assert!(!verify_blocking("anything", "not a phc string"));
    }
}
