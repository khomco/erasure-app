//! An in-process hub over real HTTP for integration tests: a migrated
//! [`TestDb`], CSRF-aware requests and the sign-up, invitation and audit
//! helpers. Copied from `tests/team.rs` (portal-accounts story 5), which
//! keeps its own copy. Also `wipe-hub admin create` as a subprocess and
//! [`bootstrap_admin`], a signed-in vendor admin.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use reqwest::header::{COOKIE, SET_COOKIE};
use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use uuid::Uuid;
use wipe_hub::config::{DEFAULT_MAIL_FROM, DEFAULT_SMTP_HOST, DEFAULT_SMTP_PORT};
use wipe_hub::mail::{INVITATION_SUBJECT, PASSWORD_RESET_SUBJECT};
use wipe_hub::{AppState, Args, Config};

use super::{unique_address, Mailpit, TestDb};

pub const PASSWORD: &str = "correct horse battery";
pub const NEW_PASSWORD: &str = "a brand new passphrase";
pub const BASE: &str = "http://127.0.0.1:7900";
pub const MAIL_WAIT: Duration = Duration::from_secs(10);
pub const PEER: &str = "ip:127.0.0.1";

pub const OWNER_ONLY: &str = "only an organization owner can manage the team";
pub const BAD_INVITATION: &str = "this invitation is invalid or has expired";
pub const LAST_OWNER: &str = "an organization must keep at least one owner";
pub const LOCKED: &str = "too many failed sign-in attempts; try again later";
pub const MAIL_LOCKED: &str = "too many emails requested; try again later";

pub struct Hub {
    pub addr: SocketAddr,
    pub http: reqwest::Client,
    pub db: TestDb,
    pub csrf: String,
    /// The hub's `--artifact-dir`, removed with the hub.
    pub artifacts: tempfile::TempDir,
}

pub async fn spawn_hub_with(smtp_port: u16) -> Hub {
    spawn_hub_args(|a| a.smtp_port = smtp_port).await
}

/// A hub whose default arguments `edit` changes first.
pub async fn spawn_hub_args(edit: impl FnOnce(&mut Args)) -> Hub {
    spawn_hub_config(edit, |_| {}).await
}

/// A hub whose default arguments `edit` changes, and whose validated
/// configuration `edit_config` then changes (for settings that are not
/// flags, such as `artifact_max_bytes`).
pub async fn spawn_hub_config(
    edit: impl FnOnce(&mut Args),
    edit_config: impl FnOnce(&mut Config),
) -> Hub {
    let db = TestDb::migrated().await;
    let artifacts = tempfile::tempdir().unwrap();
    let mut args = Args {
        addr: "127.0.0.1:0".parse().unwrap(),
        database_url: db.url.clone(),
        smtp_host: DEFAULT_SMTP_HOST.into(),
        smtp_port: DEFAULT_SMTP_PORT,
        mail_from: DEFAULT_MAIL_FROM.into(),
        public_base_url: BASE.into(),
        payment_provider: wipe_hub::PaymentProvider::Fake,
        stripe_webhook_secret: None,
        stripe_secret_key: None,
        stripe_api_base: wipe_hub::config::DEFAULT_STRIPE_API_BASE.into(),
        vendor_root_seed: None,
        portal_dir: None,
        site_origin: wipe_hub::config::DEFAULT_SITE_ORIGIN.into(),
        artifact_dir: artifacts.path().to_path_buf(),
    };
    edit(&mut args);
    let mut config = Config::from_args(args).unwrap();
    config.portal_dir = None;
    edit_config(&mut config);
    let state = AppState::new(config).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(wipe_hub::serve_on(listener, state));
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let resp = http
        .get(format!("http://{addr}/api/auth/csrf"))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let csrf = body["csrf_token"].as_str().unwrap().to_string();
    Hub {
        addr,
        http,
        db,
        csrf,
        artifacts,
    }
}

/// A hub that signs licences with a fresh file-held vendor root (licence-
/// delivery story 1): the hub and that root.
pub async fn spawn_hub_with_root() -> (Hub, wipe_license::VendorRoot) {
    let root = wipe_license::VendorRoot::generate();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vendor-root.seed");
    std::fs::write(&path, root.seed_bytes()).unwrap();
    // The seed is read while the configuration is built, so the directory
    // may go once the hub is up.
    let hub = spawn_hub_args(|a| a.vendor_root_seed = Some(path)).await;
    (hub, root)
}

pub async fn spawn_hub() -> Hub {
    spawn_hub_with(DEFAULT_SMTP_PORT).await
}

/// A signed-in user: id, email and `Cookie` value.
#[derive(Clone)]
pub struct Who {
    pub id: String,
    pub email: String,
    pub cookie: String,
}

impl Hub {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub fn post_req(&self, path: &str, cookie: Option<&str>) -> reqwest::RequestBuilder {
        let csrf = format!("wipehub_csrf={}", self.csrf);
        let cookie = match cookie {
            Some(c) => format!("{c}; {csrf}"),
            None => csrf,
        };
        self.http
            .post(self.url(path))
            .header(COOKIE, cookie)
            .header("x-csrf-token", &self.csrf)
    }

    pub async fn post(&self, path: &str, body: Value, cookie: Option<&str>) -> Response {
        self.post_req(path, cookie)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    pub async fn post_empty(&self, path: &str, cookie: Option<&str>) -> Response {
        self.post_req(path, cookie).send().await.unwrap()
    }

    pub async fn get(&self, path: &str, cookie: Option<&str>) -> Response {
        let mut req = self.http.get(self.url(path));
        if let Some(c) = cookie {
            req = req.header(COOKIE, c);
        }
        req.send().await.unwrap()
    }

    pub async fn me(&self, cookie: &str) -> (StatusCode, Value) {
        let resp = self.get("/api/auth/me", Some(cookie)).await;
        let code = resp.status();
        (code, resp.json().await.unwrap())
    }

    /// Sign up a new organization's owner (unverified).
    pub async fn sign_up(&self, email: &str, org: &str) -> Who {
        let resp = self
            .post(
                "/api/auth/sign-up",
                json!({
                    "email": email,
                    "password": PASSWORD,
                    "name": "Ada Lovelace",
                    "organization_name": org,
                }),
                None,
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let cookie = cookie_pair(&resp);
        let body: Value = resp.json().await.unwrap();
        Who {
            id: body["user"]["id"].as_str().unwrap().to_string(),
            email: email.to_string(),
            cookie,
        }
    }

    /// Sign up an owner and mark their email verified.
    pub async fn verified_owner(&self, org: &str) -> Who {
        let owner = self.sign_up(&unique_address(), org).await;
        self.verify(&owner.email).await;
        owner
    }

    pub async fn verify(&self, email: &str) {
        sqlx::query("UPDATE users SET email_verified_at = now() WHERE lower(email) = lower($1)")
            .bind(email)
            .execute(&self.db.pool)
            .await
            .unwrap();
    }

    pub async fn sign_in(&self, email: &str, password: &str) -> Response {
        self.post(
            "/api/auth/sign-in",
            json!({"email": email, "password": password}),
            None,
        )
        .await
    }

    pub async fn invite(&self, owner: &Who, email: &str, role: &str) -> Response {
        self.post(
            "/api/organization/invitations",
            json!({"email": email, "role": role}),
            Some(&owner.cookie),
        )
        .await
    }

    pub async fn accept(&self, token: &str, password: &str, cookie: Option<&str>) -> Response {
        self.post(
            "/api/invitations/accept",
            json!({"token": token, "name": "Grace Hopper", "password": password}),
            cookie,
        )
        .await
    }

    pub async fn lookup(&self, token: &str) -> Response {
        self.post("/api/invitations/lookup", json!({ "token": token }), None)
            .await
    }

    /// Invite a fresh address as `role` and accept it: the new user.
    pub async fn add(&self, owner: &Who, role: &str) -> Who {
        let email = unique_address();
        let resp = self.invite(owner, &email, role).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let token = invite_tokens(&email, 1).await.remove(0);
        let resp = self.accept(&token, PASSWORD, None).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let cookie = cookie_pair(&resp);
        let body: Value = resp.json().await.unwrap();
        Who {
            id: body["user"]["id"].as_str().unwrap().to_string(),
            email,
            cookie,
        }
    }

    pub async fn members(&self, cookie: &str) -> Vec<Value> {
        let resp = self.get("/api/organization/members", Some(cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = resp.json().await.unwrap();
        body["members"].as_array().unwrap().clone()
    }

    pub async fn invitations(&self, cookie: &str) -> Vec<Value> {
        let resp = self
            .get("/api/organization/invitations", Some(cookie))
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = resp.json().await.unwrap();
        body["invitations"].as_array().unwrap().clone()
    }

    pub async fn set_role(&self, owner: &Who, id: &str, role: &str) -> Response {
        self.post(
            &format!("/api/organization/members/{id}/role"),
            json!({ "role": role }),
            Some(&owner.cookie),
        )
        .await
    }

    pub async fn remove(&self, owner: &Who, id: &str) -> Response {
        self.post_empty(
            &format!("/api/organization/members/{id}/remove"),
            Some(&owner.cookie),
        )
        .await
    }

    pub async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    /// `(actor, subject, detail)` of every audit row for `action`, oldest first.
    pub async fn audited(&self, action: &str) -> Vec<(String, String, Value)> {
        sqlx::query_as("SELECT actor, subject, detail FROM audit_log WHERE action = $1 ORDER BY id")
            .bind(action)
            .fetch_all(&self.db.pool)
            .await
            .unwrap()
    }

    pub async fn actions(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT action FROM audit_log ORDER BY id")
            .fetch_all(&self.db.pool)
            .await
            .unwrap()
    }

    /// A published offering inserted directly (no vendor admin needed):
    /// its id. Annual, USD 1,200.00, one machine seat, unlimited erasures.
    pub async fn published_offering(&self, name: &str) -> String {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO offerings (name, description, price_minor, currency, billing_interval, \
                                    status, scope, seats, quota_erasures, features, term_months, \
                                    published_at) \
             VALUES ($1, 'One station.', 120000, 'USD', 'annual', 'published', 'machine', 1, \
                     NULL, '{hub_sync,pdf_certificates}', 12, now()) RETURNING id",
        )
        .bind(name)
        .fetch_one(&self.db.pool)
        .await
        .unwrap();
        id.to_string()
    }

    /// A published one-off offering inserted directly: its id. USD 1,200.00,
    /// two site seats, 5000 erasures, `term_months` months.
    pub async fn published_one_off_offering(&self, name: &str, term_months: i32) -> String {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO offerings (name, description, price_minor, currency, billing_interval, \
                                    status, scope, seats, quota_erasures, features, term_months, \
                                    published_at) \
             VALUES ($1, 'Pay once.', 120000, 'USD', 'one_off', 'published', 'site', 2, \
                     5000, '{pdf_certificates}', $2, now()) RETURNING id",
        )
        .bind(name)
        .bind(term_months)
        .fetch_one(&self.db.pool)
        .await
        .unwrap();
        id.to_string()
    }

    /// A published free trial offering inserted directly: its id. Price 0,
    /// one-off, one site seat, 25 erasures, one month, `pdf_certificates`.
    pub async fn published_trial_offering(&self, name: &str) -> String {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO offerings (name, description, price_minor, currency, billing_interval, \
                                    status, scope, seats, quota_erasures, features, term_months, \
                                    published_at, trial) \
             VALUES ($1, 'Try it.', 0, 'USD', 'one_off', 'published', 'site', 1, 25, \
                     '{pdf_certificates}', 1, now(), true) RETURNING id",
        )
        .bind(name)
        .fetch_one(&self.db.pool)
        .await
        .unwrap();
        id.to_string()
    }

    pub async fn org_name(&self, owner: &Who) -> String {
        self.me(&owner.cookie).await.1["organization"]["name"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Row counts that a refused request must leave unchanged.
    pub async fn counts(&self) -> Vec<i64> {
        let mut out = Vec::new();
        for t in [
            "audit_log",
            "users",
            "sessions",
            "auth_throttle",
            "auth_tokens",
            "invitations",
            "organizations",
        ] {
            out.push(self.count(t).await);
        }
        out
    }

    /// Every user's (id, role, name) and the organizations' names: a
    /// snapshot that cross-organization attempts must leave alone.
    pub async fn snapshot(&self) -> (Vec<(Uuid, String, String)>, Vec<String>, Vec<(Uuid, bool)>) {
        let users = sqlx::query_as("SELECT id, role, name FROM users ORDER BY id")
            .fetch_all(&self.db.pool)
            .await
            .unwrap();
        let orgs = sqlx::query_scalar("SELECT name FROM organizations ORDER BY id")
            .fetch_all(&self.db.pool)
            .await
            .unwrap();
        let invs = sqlx::query_as(
            "SELECT id, revoked_at IS NULL AND accepted_at IS NULL FROM invitations ORDER BY id",
        )
        .fetch_all(&self.db.pool)
        .await
        .unwrap();
        (users, orgs, invs)
    }
}

pub fn cookie_pair(resp: &Response) -> String {
    resp.headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .find(|v| v.starts_with("wipehub_session="))
        .expect("a wipehub_session Set-Cookie header")
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

pub async fn error_of(resp: Response) -> (StatusCode, String) {
    let code = resp.status();
    let body: Value = resp.json().await.unwrap();
    let obj = body.as_object().expect("error body is an object");
    assert_eq!(obj.len(), 1, "{body}");
    (code, obj["error"].as_str().unwrap().to_string())
}

pub fn token_in(text: &str, page: &str) -> String {
    let prefix = format!("{BASE}/{page}?token=");
    let start = text
        .find(&prefix)
        .unwrap_or_else(|| panic!("no {page} link in {text:?}"))
        + prefix.len();
    text[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// Wait for `n` messages to `addr` with `subject`; their texts, newest first.
pub async fn mail_texts(addr: &str, subject: &str, n: usize) -> Vec<String> {
    let mailpit = Mailpit::new();
    let deadline = Instant::now() + MAIL_WAIT;
    loop {
        let found: Vec<_> = mailpit
            .messages_to(addr)
            .await
            .into_iter()
            .filter(|m| m.subject == subject)
            .collect();
        if found.len() >= n {
            let mut texts = Vec::new();
            for m in found {
                texts.push(mailpit.message(&m.id).await.text);
            }
            return texts;
        }
        if Instant::now() >= deadline {
            panic!("{n} {subject:?} messages to {addr} did not arrive");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The tokens of `n` invitation emails to `addr`, newest first.
pub async fn invite_tokens(addr: &str, n: usize) -> Vec<String> {
    mail_texts(addr, INVITATION_SUBJECT, n)
        .await
        .iter()
        .map(|t| token_in(t, "accept-invite"))
        .collect()
}

pub async fn mail_count(addr: &str) -> usize {
    Mailpit::new().messages_to(addr).await.len()
}

pub async fn assert_error(resp: Response, code: StatusCode, message: &str) {
    assert_eq!(error_of(resp).await, (code, message.to_string()));
}

// ---------------------------------------------------------------------------
// Profile and password

// ---------------------------------------------------------------------------
// `wipe-hub admin create` and a signed-in vendor admin (moved from
// `tests/admin.rs` for lead-capture story 2)

/// `wipe-hub admin create` against `db_url`, with every `WIPE_HUB_*` var
/// removed so a developer's environment cannot change the outcome.
pub fn admin_create(db_url: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wipe-hub"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("WIPE_HUB_") {
            cmd.env_remove(&key);
        }
    }
    cmd.env("RUST_LOG", "warn")
        .args(["admin", "create", "--database-url", db_url])
        .args(["--public-base-url", BASE])
        .args(args);
    cmd
}

pub fn run(db_url: &str, args: &[&str]) -> Output {
    admin_create(db_url, args).output().unwrap()
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub fn assert_ok(out: &Output) {
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        stdout(out),
        stderr(out)
    );
}

pub fn assert_failed_with(out: &Output, message: &str) {
    assert!(!out.status.success(), "stdout: {}", stdout(out));
    assert!(stderr(out).contains(message), "stderr: {}", stderr(out));
}

pub fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// The emailed set-password token for `email`.
pub async fn reset_token(email: &str) -> String {
    let texts = mail_texts(email, PASSWORD_RESET_SUBJECT, 1).await;
    token_in(&texts[0], "reset-password")
}

/// Create a vendor admin through the CLI, follow the emailed link to set a
/// password, and sign in.
pub async fn bootstrap_admin(hub: &Hub) -> Who {
    let email = unique_address();
    assert_ok(&run(
        &hub.db.url,
        &["--email", &email, "--name", "Vera Vendor"],
    ));
    let token = reset_token(&email).await;
    let resp = hub
        .post(
            "/api/auth/password-reset/complete",
            json!({"token": token, "password": NEW_PASSWORD}),
            None,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = hub.sign_in(&email, NEW_PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = cookie_pair(&resp);
    let (_, me) = hub.me(&cookie).await;
    Who {
        id: me["user"]["id"].as_str().unwrap().to_string(),
        email,
        cookie,
    }
}
