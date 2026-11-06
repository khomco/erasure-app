//! Schema versioning (ADR-0007 §2): sqlx migrations embedded at compile time
//! from `crates/wipe-hub/migrations` and applied before the hub listens.
//!
//! Start-up (`serve`) and the integration-test harness (`TestDb::migrated`)
//! both go through [`migrate_url`], so tests run against exactly the schema
//! the binary ships, applied the same way.

use std::time::Duration;

use anyhow::anyhow;
use sqlx::migrate::{Migrate, MigrateError, Migrator};
use sqlx::postgres::{PgConnection, PgPool};
use sqlx::Connection;
use tracing::info;
use url::Url;

/// Every migration in `migrations/`, embedded in the binary.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// How long start-up waits for its single migration connection.
pub const MIGRATE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Apply every pending embedded migration through `pool`.
pub async fn migrate(pool: &PgPool) -> Result<(), MigrateError> {
    MIGRATOR.run(pool).await
}

/// Number of embedded (up) migrations.
pub fn embedded_count() -> usize {
    MIGRATOR
        .iter()
        .filter(|m| m.migration_type.is_up_migration())
        .count()
}

/// Start-up migration: one connection attempt bounded by
/// [`MIGRATE_CONNECT_TIMEOUT`], separate from the lazy request pool, so an
/// unreachable Postgres fails start-up in seconds. Errors name the redacted
/// address and never the password.
pub async fn migrate_url(url: &str) -> anyhow::Result<()> {
    let fail = |cause: String| {
        anyhow!(
            "cannot apply migrations to {}: {cause}; is Postgres running? run docker compose up -d",
            redacted(url)
        )
    };
    let mut conn =
        match tokio::time::timeout(MIGRATE_CONNECT_TIMEOUT, PgConnection::connect(url)).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => return Err(fail(e.to_string())),
            Err(_) => {
                return Err(fail(format!(
                    "no connection within {}s",
                    MIGRATE_CONNECT_TIMEOUT.as_secs()
                )))
            }
        };

    let result = async {
        conn.ensure_migrations_table().await?;
        let before = conn.list_applied_migrations().await?.len();
        MIGRATOR.run_direct(&mut conn).await?;
        let after = conn.list_applied_migrations().await?.len();
        Ok::<_, MigrateError>((before, after))
    }
    .await;
    let _ = conn.close().await;

    let (before, after) = result.map_err(|e| fail(e.to_string()))?;
    info!(
        database = %redacted(url),
        applied = after.saturating_sub(before),
        present = after,
        embedded = embedded_count(),
        "database migrations up to date"
    );
    Ok(())
}

/// `url` with its password and query string removed, safe to log. An
/// unparseable URL is never echoed, since it may still hold a password.
pub fn redacted(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut u) => {
            let _ = u.set_password(None);
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => "<unparseable database URL>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_drops_the_password() {
        let r =
            redacted("postgres://wipehub:s3cret-Pa55@db.example:5432/wipehub?password=s3cret-Pa55");
        assert!(!r.contains("s3cret-Pa55"), "{r}");
        assert!(r.contains("db.example:5432/wipehub"), "{r}");
    }

    #[test]
    fn redacted_never_echoes_an_unparseable_url() {
        let r = redacted("not a url s3cret");
        assert!(!r.contains("s3cret"), "{r}");
    }

    #[test]
    fn at_least_one_migration_is_embedded() {
        assert!(embedded_count() >= 1);
    }
}
