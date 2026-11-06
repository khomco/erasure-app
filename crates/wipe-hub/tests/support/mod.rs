//! Shared integration-test harness for `wipe-hub` (ADR-0007 §2).
//!
//! - [`TestDb`]: a fresh, uniquely named database on the compose Postgres,
//!   dropped (`WITH (FORCE)`) when the guard is dropped, including on panic.
//! - [`Mailpit`]: a client for Mailpit's HTTP API. Mailpit is shared by every
//!   test, so tests isolate by sending to a [`unique_address`] and never delete
//!   messages globally.
//!
//! Everything here panics (never skips) when a compose service is unreachable,
//! and the message says `run docker compose up -d`.
//!
//! Use from a test file with `mod support;`.

#![allow(dead_code)]

use std::time::{Duration, Instant};

use serde::Deserialize;
use sqlx::postgres::{PgConnection, PgPool, PgPoolOptions};
use sqlx::Connection;
use url::Url;
use wipe_hub::config::DEFAULT_DATABASE_URL;
use wipe_hub::db;

/// How long the harness waits for Postgres or Mailpit before failing.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Prefix of every test database name; nothing else may use it.
pub const TEST_DB_PREFIX: &str = "wipehub_test_";

/// The Mailpit HTTP API in `compose.yaml`.
pub const MAILPIT_API: &str = "http://127.0.0.1:8025";

fn compose_hint(what: &str, addr: &str, cause: impl std::fmt::Display) -> String {
    format!(
        "{what} is unreachable at {addr}: {cause}; run docker compose up -d at the repo root \
         (if it is already up, credentials may be stale: docker compose down -v)"
    )
}

/// One admin connection to `admin_url`, bounded by [`CONNECT_TIMEOUT`].
/// Panics with the compose hint on failure.
pub async fn admin_connect(admin_url: &str) -> PgConnection {
    match tokio::time::timeout(CONNECT_TIMEOUT, PgConnection::connect(admin_url)).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(e)) => panic!("{}", compose_hint("Postgres", &db::redacted(admin_url), e)),
        Err(_) => panic!(
            "{}",
            compose_hint(
                "Postgres",
                &db::redacted(admin_url),
                format!("no connection within {}s", CONNECT_TIMEOUT.as_secs())
            )
        ),
    }
}

/// Whether a database called `name` exists on the compose Postgres.
pub async fn database_exists(name: &str) -> bool {
    let mut conn = admin_connect(DEFAULT_DATABASE_URL).await;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(name)
            .fetch_one(&mut conn)
            .await
            .expect("query pg_database");
    let _ = conn.close().await;
    exists
}

/// A disposable database. Dropping the guard drops the database.
pub struct TestDb {
    pub pool: PgPool,
    pub url: String,
    pub name: String,
    admin_url: String,
}

impl TestDb {
    /// A new database with no schema at all.
    pub async fn empty() -> Self {
        Self::empty_on(DEFAULT_DATABASE_URL).await
    }

    /// A new database with every embedded migration applied through
    /// [`wipe_hub::db::migrate_url`], the same function start-up uses.
    pub async fn migrated() -> Self {
        let db = Self::empty().await;
        db::migrate_url(&db.url)
            .await
            .unwrap_or_else(|e| panic!("migrating test database {}: {e:#}", db.name));
        db
    }

    /// [`TestDb::empty`] against a given admin URL (used to test the
    /// unreachable-Postgres message).
    pub async fn empty_on(admin_url: &str) -> Self {
        let name = format!("{TEST_DB_PREFIX}{}", uuid::Uuid::new_v4().simple());
        let mut admin = admin_connect(admin_url).await;
        // `name` is generated here (prefix + hex), so quoting is safe.
        sqlx::query(&format!(r#"CREATE DATABASE "{name}""#))
            .execute(&mut admin)
            .await
            .unwrap_or_else(|e| panic!("CREATE DATABASE {name}: {e}"));
        let _ = admin.close().await;

        let mut url = Url::parse(admin_url).expect("admin database URL");
        url.set_path(&format!("/{name}"));
        let url = url.to_string();

        // The pool is lazy, so the guard exists before the first real
        // connection: if `acquire()` below fails, dropping the guard still
        // drops the database.
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(CONNECT_TIMEOUT)
            .connect_lazy(&url)
            .expect("test database URL");
        let db = Self {
            pool,
            url,
            name,
            admin_url: admin_url.to_string(),
        };
        db.pool
            .acquire()
            .await
            .unwrap_or_else(|e| panic!("{}", compose_hint("Postgres", &db::redacted(&db.url), e)));
        db
    }
}

impl Drop for TestDb {
    /// Drops the database from a separate thread with its own runtime, since
    /// `Drop` cannot await and may run inside (or outside) the test's runtime.
    /// Errors go to stderr: panicking here would abort a test that is already
    /// unwinding.
    fn drop(&mut self) {
        let name = self.name.clone();
        let admin_url = self.admin_url.clone();
        let joined = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            rt.block_on(async {
                let mut admin =
                    tokio::time::timeout(CONNECT_TIMEOUT, PgConnection::connect(&admin_url))
                        .await
                        .map_err(|_| "admin connect timed out".to_string())?
                        .map_err(|e| e.to_string())?;
                sqlx::query(&format!(r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#))
                    .execute(&mut admin)
                    .await
                    .map_err(|e| e.to_string())?;
                let _ = admin.close().await;
                Ok::<_, String>(())
            })
        })
        .join();
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("TestDb: could not drop database {}: {e}", self.name),
            Err(_) => eprintln!("TestDb: drop thread for {} panicked", self.name),
        }
    }
}

/// A recipient address no other test uses.
pub fn unique_address() -> String {
    format!("t-{}@wipehub.test", uuid::Uuid::new_v4().simple())
}

/// One entry from Mailpit's search results.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageSummary {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Subject")]
    pub subject: String,
}

/// A full message from Mailpit.
#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Subject")]
    pub subject: String,
    #[serde(rename = "Text")]
    pub text: String,
    #[serde(rename = "From")]
    pub from: MailAddress,
}

/// A sender or recipient in a Mailpit message.
#[derive(Debug, Clone, Deserialize)]
pub struct MailAddress {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Address")]
    pub address: String,
}

#[derive(Deserialize)]
struct SearchResult {
    messages: Vec<MessageSummary>,
}

/// Client for Mailpit's HTTP API.
pub struct Mailpit {
    base: String,
    http: reqwest::Client,
}

impl Mailpit {
    pub fn new() -> Self {
        Self::with_base(MAILPIT_API)
    }

    pub fn with_base(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            http: reqwest::Client::builder()
                .timeout(CONNECT_TIMEOUT)
                .build()
                .expect("reqwest client"),
        }
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str, query: &[(&str, &str)]) -> T {
        let url = format!("{}{path}", self.base);
        let resp = self
            .http
            .get(&url)
            .query(query)
            .send()
            .await
            .unwrap_or_else(|e| panic!("{}", compose_hint("Mailpit API", &self.base, e)));
        let status = resp.status();
        if !status.is_success() {
            panic!("Mailpit GET {url} returned {status}");
        }
        resp.json()
            .await
            .unwrap_or_else(|e| panic!("Mailpit GET {url}: unexpected body: {e}"))
    }

    /// Every captured message addressed to `addr` (newest first).
    pub async fn messages_to(&self, addr: &str) -> Vec<MessageSummary> {
        let query = format!("to:\"{addr}\"");
        let r: SearchResult = self
            .get("/api/v1/search", &[("query", query.as_str())])
            .await;
        r.messages
    }

    /// One message by Mailpit id.
    pub async fn message(&self, id: &str) -> Message {
        self.get(&format!("/api/v1/message/{id}"), &[]).await
    }

    /// Poll until a message to `addr` arrives, then return it. Panics naming
    /// `addr` if none arrives within `timeout`.
    pub async fn wait_for_message_to(&self, addr: &str, timeout: Duration) -> Message {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(first) = self.messages_to(addr).await.into_iter().next() {
                return self.message(&first.id).await;
            }
            if Instant::now() >= deadline {
                panic!("no message to {addr} reached Mailpit within {timeout:?}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Default for Mailpit {
    fn default() -> Self {
        Self::new()
    }
}
