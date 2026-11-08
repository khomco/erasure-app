//! Account, organization and team management over real HTTP against a
//! migrated test database and Mailpit (portal-accounts story 4): profile,
//! password change, organization rename, members, roles, removal and
//! invitations. Needs `docker compose up -d`; fails, never skips, when the
//! compose services are down.

mod support;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use reqwest::header::{COOKIE, RETRY_AFTER, SET_COOKIE};
use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::{unique_address, Mailpit, TestDb};
use tokio::net::TcpListener;
use uuid::Uuid;
use wipe_hub::config::{DEFAULT_MAIL_FROM, DEFAULT_SMTP_HOST, DEFAULT_SMTP_PORT};
use wipe_hub::mail::{INVITATION_SUBJECT, PASSWORD_RESET_SUBJECT};
use wipe_hub::{AppState, Args, Config};

const PASSWORD: &str = "correct horse battery";
const NEW_PASSWORD: &str = "a brand new passphrase";
const BASE: &str = "http://127.0.0.1:7900";
const MAIL_WAIT: Duration = Duration::from_secs(10);
const PEER: &str = "ip:127.0.0.1";

const OWNER_ONLY: &str = "only an organization owner can manage the team";
const BAD_INVITATION: &str = "this invitation is invalid or has expired";
const LAST_OWNER: &str = "an organization must keep at least one owner";
const LOCKED: &str = "too many failed sign-in attempts; try again later";
const MAIL_LOCKED: &str = "too many emails requested; try again later";

struct Hub {
    addr: SocketAddr,
    http: reqwest::Client,
    db: TestDb,
    csrf: String,
}

async fn spawn_hub_with(smtp_port: u16) -> Hub {
    let db = TestDb::migrated().await;
    let args = Args {
        addr: "127.0.0.1:0".parse().unwrap(),
        database_url: db.url.clone(),
        smtp_host: DEFAULT_SMTP_HOST.into(),
        smtp_port,
        mail_from: DEFAULT_MAIL_FROM.into(),
        public_base_url: BASE.into(),
        payment_provider: wipe_hub::PaymentProvider::Fake,
        stripe_webhook_secret: None,
        stripe_secret_key: None,
        stripe_api_base: wipe_hub::config::DEFAULT_STRIPE_API_BASE.into(),
        vendor_root_seed: None,
        portal_dir: None,
        site_origin: wipe_hub::config::DEFAULT_SITE_ORIGIN.into(),
    };
    let mut config = Config::from_args(args).unwrap();
    config.portal_dir = None;
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
    }
}

async fn spawn_hub() -> Hub {
    spawn_hub_with(DEFAULT_SMTP_PORT).await
}

/// A signed-in user: id, email and `Cookie` value.
#[derive(Clone)]
struct Who {
    id: String,
    email: String,
    cookie: String,
}

impl Hub {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn post_req(&self, path: &str, cookie: Option<&str>) -> reqwest::RequestBuilder {
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

    async fn post(&self, path: &str, body: Value, cookie: Option<&str>) -> Response {
        self.post_req(path, cookie)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn post_empty(&self, path: &str, cookie: Option<&str>) -> Response {
        self.post_req(path, cookie).send().await.unwrap()
    }

    async fn get(&self, path: &str, cookie: Option<&str>) -> Response {
        let mut req = self.http.get(self.url(path));
        if let Some(c) = cookie {
            req = req.header(COOKIE, c);
        }
        req.send().await.unwrap()
    }

    async fn me(&self, cookie: &str) -> (StatusCode, Value) {
        let resp = self.get("/api/auth/me", Some(cookie)).await;
        let code = resp.status();
        (code, resp.json().await.unwrap())
    }

    /// Sign up a new organization's owner (unverified).
    async fn sign_up(&self, email: &str, org: &str) -> Who {
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
    async fn verified_owner(&self, org: &str) -> Who {
        let owner = self.sign_up(&unique_address(), org).await;
        self.verify(&owner.email).await;
        owner
    }

    async fn verify(&self, email: &str) {
        sqlx::query("UPDATE users SET email_verified_at = now() WHERE lower(email) = lower($1)")
            .bind(email)
            .execute(&self.db.pool)
            .await
            .unwrap();
    }

    async fn sign_in(&self, email: &str, password: &str) -> Response {
        self.post(
            "/api/auth/sign-in",
            json!({"email": email, "password": password}),
            None,
        )
        .await
    }

    async fn invite(&self, owner: &Who, email: &str, role: &str) -> Response {
        self.post(
            "/api/organization/invitations",
            json!({"email": email, "role": role}),
            Some(&owner.cookie),
        )
        .await
    }

    async fn accept(&self, token: &str, password: &str, cookie: Option<&str>) -> Response {
        self.post(
            "/api/invitations/accept",
            json!({"token": token, "name": "Grace Hopper", "password": password}),
            cookie,
        )
        .await
    }

    async fn lookup(&self, token: &str) -> Response {
        self.post("/api/invitations/lookup", json!({ "token": token }), None)
            .await
    }

    /// Invite a fresh address as `role` and accept it: the new user.
    async fn add(&self, owner: &Who, role: &str) -> Who {
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

    async fn members(&self, cookie: &str) -> Vec<Value> {
        let resp = self.get("/api/organization/members", Some(cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = resp.json().await.unwrap();
        body["members"].as_array().unwrap().clone()
    }

    async fn invitations(&self, cookie: &str) -> Vec<Value> {
        let resp = self
            .get("/api/organization/invitations", Some(cookie))
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = resp.json().await.unwrap();
        body["invitations"].as_array().unwrap().clone()
    }

    async fn set_role(&self, owner: &Who, id: &str, role: &str) -> Response {
        self.post(
            &format!("/api/organization/members/{id}/role"),
            json!({ "role": role }),
            Some(&owner.cookie),
        )
        .await
    }

    async fn remove(&self, owner: &Who, id: &str) -> Response {
        self.post_empty(
            &format!("/api/organization/members/{id}/remove"),
            Some(&owner.cookie),
        )
        .await
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    /// `(actor, subject, detail)` of every audit row for `action`, oldest first.
    async fn audited(&self, action: &str) -> Vec<(String, String, Value)> {
        sqlx::query_as("SELECT actor, subject, detail FROM audit_log WHERE action = $1 ORDER BY id")
            .bind(action)
            .fetch_all(&self.db.pool)
            .await
            .unwrap()
    }

    async fn actions(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT action FROM audit_log ORDER BY id")
            .fetch_all(&self.db.pool)
            .await
            .unwrap()
    }

    async fn org_name(&self, owner: &Who) -> String {
        self.me(&owner.cookie).await.1["organization"]["name"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Row counts that a refused request must leave unchanged.
    async fn counts(&self) -> Vec<i64> {
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
    async fn snapshot(&self) -> (Vec<(Uuid, String, String)>, Vec<String>, Vec<(Uuid, bool)>) {
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

fn cookie_pair(resp: &Response) -> String {
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

async fn error_of(resp: Response) -> (StatusCode, String) {
    let code = resp.status();
    let body: Value = resp.json().await.unwrap();
    let obj = body.as_object().expect("error body is an object");
    assert_eq!(obj.len(), 1, "{body}");
    (code, obj["error"].as_str().unwrap().to_string())
}

fn token_in(text: &str, page: &str) -> String {
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
async fn mail_texts(addr: &str, subject: &str, n: usize) -> Vec<String> {
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
async fn invite_tokens(addr: &str, n: usize) -> Vec<String> {
    mail_texts(addr, INVITATION_SUBJECT, n)
        .await
        .iter()
        .map(|t| token_in(t, "accept-invite"))
        .collect()
}

async fn mail_count(addr: &str) -> usize {
    Mailpit::new().messages_to(addr).await.len()
}

async fn assert_error(resp: Response, code: StatusCode, message: &str) {
    assert_eq!(error_of(resp).await, (code, message.to_string()));
}

// ---------------------------------------------------------------------------
// Profile and password

#[tokio::test]
async fn profile_name_is_trimmed_audited_and_validated() {
    let hub = spawn_hub().await;
    let owner = hub.sign_up(&unique_address(), "Engines").await;
    let resp = hub
        .post(
            "/api/account/profile",
            json!({"name": " Grace "}),
            Some(&owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["user"]["name"], "Grace");
    assert_eq!(body["organization"]["name"], "Engines");
    assert_eq!(hub.me(&owner.cookie).await.1["user"]["name"], "Grace");
    let rows = hub.audited("account.profile_updated").await;
    let who = format!("user:{}", owner.id);
    assert_eq!(rows, vec![(who.clone(), who, json!({}))]);

    for (bad, msg) in [
        ("  ", "name must not be blank"),
        (
            "Eve\nBcc: x@evil.test",
            "name must not contain control characters",
        ),
    ] {
        let resp = hub
            .post(
                "/api/account/profile",
                json!({ "name": bad }),
                Some(&owner.cookie),
            )
            .await;
        assert_error(resp, StatusCode::BAD_REQUEST, msg).await;
    }
    assert_eq!(hub.me(&owner.cookie).await.1["user"]["name"], "Grace");
    assert_eq!(hub.audited("account.profile_updated").await.len(), 1);
    let resp = hub
        .post("/api/account/profile", json!({"name": "X"}), None)
        .await;
    assert_error(resp, StatusCode::UNAUTHORIZED, "not signed in").await;
}

#[tokio::test]
async fn password_change_signs_out_everywhere_else_and_keeps_this_browser() {
    let hub = spawn_hub().await;
    let owner = hub.sign_up(&unique_address(), "Engines").await;
    let resp = hub.sign_in(&owner.email, PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let b = cookie_pair(&resp);

    // A too-short new password is refused before anything is charged.
    let throttle_before = hub.count("auth_throttle").await;
    let resp = hub
        .post(
            "/api/auth/password",
            json!({"current_password": PASSWORD, "new_password": "a".repeat(11)}),
            Some(&owner.cookie),
        )
        .await;
    assert_error(
        resp,
        StatusCode::BAD_REQUEST,
        "password must be at least 12 characters",
    )
    .await;
    assert_eq!(hub.count("auth_throttle").await, throttle_before);

    let resp = hub
        .post(
            "/api/auth/password",
            json!({"current_password": PASSWORD, "new_password": NEW_PASSWORD}),
            Some(&owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let c = cookie_pair(&resp);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["user"]["id"], json!(owner.id));

    assert_eq!(hub.me(&owner.cookie).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(hub.me(&b).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(hub.me(&c).await.0, StatusCode::OK);
    assert_eq!(
        hub.sign_in(&owner.email, PASSWORD).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        hub.sign_in(&owner.email, NEW_PASSWORD).await.status(),
        StatusCode::OK
    );
    let who = format!("user:{}", owner.id);
    assert_eq!(
        hub.audited("auth.password_changed").await,
        vec![(who.clone(), who, json!({"sessions_ended": 2, "peer": PEER}))]
    );
}

#[tokio::test]
async fn wrong_current_passwords_lock_the_email_like_sign_in() {
    let hub = spawn_hub().await;
    let owner = hub.sign_up(&unique_address(), "Engines").await;
    for _ in 0..5 {
        let resp = hub
            .post(
                "/api/auth/password",
                json!({"current_password": "not my password", "new_password": NEW_PASSWORD}),
                Some(&owner.cookie),
            )
            .await;
        assert_error(
            resp,
            StatusCode::BAD_REQUEST,
            "current password is incorrect",
        )
        .await;
    }
    let failed = hub.audited("auth.password_change_failed").await;
    assert_eq!(failed.len(), 5);
    assert_eq!(
        failed[0].2,
        json!({"reason": "wrong_password", "peer": PEER})
    );
    let locked = hub.audited("auth.sign_in_locked").await;
    assert_eq!(locked.len(), 1, "{locked:?}");
    assert_eq!(locked[0].1, format!("email:{}", owner.email.to_lowercase()));

    // Even the right password is now refused, and so is sign-in.
    let resp = hub
        .post(
            "/api/auth/password",
            json!({"current_password": PASSWORD, "new_password": NEW_PASSWORD}),
            Some(&owner.cookie),
        )
        .await;
    assert!(resp.headers().get(RETRY_AFTER).is_some());
    assert_error(resp, StatusCode::TOO_MANY_REQUESTS, LOCKED).await;
    let resp = hub.sign_in(&owner.email, PASSWORD).await;
    assert_error(resp, StatusCode::TOO_MANY_REQUESTS, LOCKED).await;
    // The session itself is untouched.
    assert_eq!(hub.me(&owner.cookie).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_password_change_retires_outstanding_reset_links() {
    let hub = spawn_hub().await;
    let owner = hub.sign_up(&unique_address(), "Engines").await;
    let resp = hub
        .post(
            "/api/auth/password-reset",
            json!({"email": owner.email}),
            None,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let reset = token_in(
        &mail_texts(&owner.email, PASSWORD_RESET_SUBJECT, 1).await[0],
        "reset-password",
    );
    let resp = hub
        .post(
            "/api/auth/password",
            json!({"current_password": PASSWORD, "new_password": NEW_PASSWORD}),
            Some(&owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = hub
        .post(
            "/api/auth/password-reset/complete",
            json!({"token": reset, "password": "yet another passphrase"}),
            None,
        )
        .await;
    assert_error(
        resp,
        StatusCode::BAD_REQUEST,
        "this password reset link is invalid or has expired",
    )
    .await;
    assert_eq!(
        hub.sign_in(&owner.email, NEW_PASSWORD).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_magic_link_account_without_a_password_cannot_change_it() {
    let hub = spawn_hub().await;
    let owner = hub.sign_up(&unique_address(), "Engines").await;
    sqlx::query("UPDATE users SET password_hash = '!unusable' WHERE id = $1::uuid")
        .bind(&owner.id)
        .execute(&hub.db.pool)
        .await
        .unwrap();
    let resp = hub
        .post(
            "/api/auth/password",
            json!({"current_password": "!unusable", "new_password": NEW_PASSWORD}),
            Some(&owner.cookie),
        )
        .await;
    assert_error(
        resp,
        StatusCode::BAD_REQUEST,
        "current password is incorrect",
    )
    .await;
}

// ---------------------------------------------------------------------------
// Organization and members

#[tokio::test]
async fn owners_rename_the_organization_and_members_cannot() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let member = hub.add(&owner, "member").await;

    let resp = hub
        .post(
            "/api/organization",
            json!({"name": " Difference Engines "}),
            Some(&owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "Difference Engines");
    let rows = hub.audited("organization.renamed").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, format!("user:{}", owner.id));
    assert!(rows[0].1.starts_with("organization:"), "{}", rows[0].1);
    assert_eq!(
        rows[0].2,
        json!({"from": "Engines", "to": "Difference Engines"})
    );

    let resp = hub
        .post(
            "/api/organization",
            json!({"name": "Hijacked"}),
            Some(&member.cookie),
        )
        .await;
    assert_error(resp, StatusCode::FORBIDDEN, OWNER_ONLY).await;
    assert_eq!(hub.org_name(&owner).await, "Difference Engines");

    // The name goes into invitation emails: no control characters.
    let resp = hub
        .post(
            "/api/organization",
            json!({"name": "Engines\r\nBcc: x@evil.test"}),
            Some(&owner.cookie),
        )
        .await;
    assert_error(
        resp,
        StatusCode::BAD_REQUEST,
        "name must not contain control characters",
    )
    .await;
    assert_eq!(hub.org_name(&owner).await, "Difference Engines");
    assert_eq!(hub.audited("organization.renamed").await.len(), 1);
}

#[tokio::test]
async fn the_member_list_is_the_organizations_own() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("A").await;
    let member = hub.add(&owner, "member").await;
    let _other = hub.verified_owner("B").await;

    for cookie in [&owner.cookie, &member.cookie] {
        let members = hub.members(cookie).await;
        let mut ids: Vec<&str> = members.iter().map(|m| m["id"].as_str().unwrap()).collect();
        ids.sort();
        let mut want = vec![owner.id.as_str(), member.id.as_str()];
        want.sort();
        assert_eq!(ids, want);
        // Ordered by name: "Ada Lovelace" before "Grace Hopper".
        assert_eq!(members[0]["name"], "Ada Lovelace");
        assert_eq!(members[1]["role"], "member");
        assert!(members[0].get("password_hash").is_none());
    }
    assert_error(
        hub.get("/api/organization/members", None).await,
        StatusCode::UNAUTHORIZED,
        "not signed in",
    )
    .await;
}

// ---------------------------------------------------------------------------
// Invitations

#[tokio::test]
async fn a_verified_owner_invites_by_email() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let email = unique_address();
    let resp = hub.invite(&owner, &format!("  {email} "), "member").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["invitation_email_sent"], true);
    let inv = &body["invitation"];
    assert_eq!(inv["email"], json!(email));
    assert_eq!(inv["role"], "member");
    let mut keys: Vec<_> = inv.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["created_at", "email", "expires_at", "id", "role"]);
    let created = inv["created_at"].as_str().unwrap();
    assert!(created.ends_with('Z') && created.len() == 20, "{created}");
    let lifetime: f64 =
        sqlx::query_scalar("SELECT extract(epoch FROM $2::timestamptz - $1::timestamptz)::float8")
            .bind(created)
            .bind(inv["expires_at"].as_str().unwrap())
            .fetch_one(&hub.db.pool)
            .await
            .unwrap();
    assert_eq!(lifetime, 604800.0);

    let texts = mail_texts(&email, INVITATION_SUBJECT, 1).await;
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("Ada Lovelace"), "{}", texts[0]);
    assert!(texts[0].contains("Engines"), "{}", texts[0]);
    let token = token_in(&texts[0], "accept-invite");
    assert_eq!(token.len(), 43);

    let pending = hub.invitations(&owner.cookie).await;
    assert_eq!(pending, vec![inv.clone()]);
    let rows = hub.audited("organization.member_invited").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, format!("user:{}", owner.id));
    assert_eq!(
        rows[0].2,
        json!({"invitation_id": inv["id"], "email": email, "role": "member", "peer": PEER})
    );
    // The raw token is nowhere in the audit log.
    let all: Vec<String> = sqlx::query_scalar("SELECT detail::text FROM audit_log")
        .fetch_all(&hub.db.pool)
        .await
        .unwrap();
    assert!(all.iter().all(|d| !d.contains(&token)));
}

#[tokio::test]
async fn members_get_403_on_every_owner_route_and_nothing_changes() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let member = hub.add(&owner, "member").await;
    let other = hub.add(&owner, "member").await;
    let target = unique_address();
    hub.invite(&owner, &unique_address(), "member").await;
    let inv_id = hub.invitations(&owner.cookie).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let before = hub.counts().await;
    let snapshot = hub.snapshot().await;

    let posts: Vec<(String, Value)> = vec![
        ("/api/organization".into(), json!({"name": "Hijacked"})),
        (
            "/api/organization/invitations".into(),
            json!({"email": target, "role": "owner"}),
        ),
        (
            format!("/api/organization/invitations/{inv_id}/revoke"),
            json!({}),
        ),
        (
            format!("/api/organization/members/{}/role", other.id),
            json!({"role": "owner"}),
        ),
        (
            format!("/api/organization/members/{}/role", member.id),
            json!({"role": "owner"}),
        ),
        (
            format!("/api/organization/members/{}/remove", owner.id),
            json!({}),
        ),
        // Even a malformed body is refused for the role first.
        ("/api/organization".into(), json!({"nope": 1})),
    ];
    for (path, body) in &posts {
        let resp = hub.post(path, body.clone(), Some(&member.cookie)).await;
        assert_eq!(
            error_of(resp).await,
            (StatusCode::FORBIDDEN, OWNER_ONLY.into()),
            "{path}"
        );
    }
    assert_error(
        hub.get("/api/organization/invitations", Some(&member.cookie))
            .await,
        StatusCode::FORBIDDEN,
        OWNER_ONLY,
    )
    .await;
    assert_eq!(hub.counts().await, before);
    assert_eq!(hub.snapshot().await, snapshot);
    assert_eq!(mail_count(&target).await, 0);
}

#[tokio::test]
async fn an_unverified_owner_cannot_invite() {
    let hub = spawn_hub().await;
    let owner = hub.sign_up(&unique_address(), "Engines").await;
    let email = unique_address();
    let resp = hub.invite(&owner, &email, "member").await;
    assert_error(
        resp,
        StatusCode::FORBIDDEN,
        "verify your email address before inviting members",
    )
    .await;
    assert_eq!(hub.count("invitations").await, 0);
    assert_eq!(mail_count(&email).await, 0);
}

#[tokio::test]
async fn inviting_an_existing_member_or_a_bad_role_is_refused() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let member = hub.add(&owner, "member").await;
    for email in [owner.email.to_uppercase(), member.email.to_uppercase()] {
        let resp = hub.invite(&owner, &email, "member").await;
        assert_error(
            resp,
            StatusCode::CONFLICT,
            "this person is already a member of your organization",
        )
        .await;
    }
    // One invitation email to the member, from `add`; none since.
    assert_eq!(mail_count(&member.email).await, 1);
    // Each 409 kept its email-send charge: the owner's sign-up verification
    // or the member's invitation, plus the refused invite.
    for email in [&owner.email, &member.email] {
        let key = format!("mail:to:{}", email.to_lowercase());
        assert_eq!(throttle_failures(&hub, &key).await, Some(2), "{key}");
    }
    // A 409 keeps its email-send charge (decided under the lock); a 400 is
    // refused before anything is charged.
    let throttle = hub.count("auth_throttle").await;
    for role in ["vendor_admin", "admin"] {
        let resp = hub.invite(&owner, &unique_address(), role).await;
        assert_error(
            resp,
            StatusCode::BAD_REQUEST,
            "role must be owner or member",
        )
        .await;
    }
    let resp = hub.invite(&owner, "not an address", "member").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(hub.count("auth_throttle").await, throttle);
    assert_eq!(hub.count("invitations").await, 1);
}

#[tokio::test]
async fn inviting_again_replaces_the_pending_invitation() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let email = unique_address();
    assert_eq!(
        hub.invite(&owner, &email, "member").await.status(),
        StatusCode::CREATED
    );
    let first = invite_tokens(&email, 1).await.remove(0);
    assert_eq!(
        hub.invite(&owner, &email.to_uppercase(), "owner")
            .await
            .status(),
        StatusCode::CREATED
    );
    let tokens = invite_tokens(&email, 2).await;
    let second = tokens.into_iter().find(|t| *t != first).unwrap();
    let pending = hub.invitations(&owner.cookie).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["role"], "owner");

    assert_error(
        hub.accept(&first, PASSWORD, None).await,
        StatusCode::BAD_REQUEST,
        BAD_INVITATION,
    )
    .await;
    let resp = hub.accept(&second, PASSWORD, None).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["user"]["role"], "owner");
}

#[tokio::test]
async fn invitations_share_the_per_address_email_limit() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let email = unique_address();
    for _ in 0..3 {
        assert_eq!(
            hub.invite(&owner, &email, "member").await.status(),
            StatusCode::CREATED
        );
    }
    for _ in 0..2 {
        let resp = hub
            .post("/api/auth/magic-link", json!({"email": email}), None)
            .await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
    }
    let resp = hub.invite(&owner, &email, "member").await;
    assert!(resp.headers().get(RETRY_AFTER).is_some());
    assert_error(resp, StatusCode::TOO_MANY_REQUESTS, MAIL_LOCKED).await;
    assert_eq!(hub.audited("organization.member_invited").await.len(), 3);
}

#[tokio::test]
async fn an_invitation_is_created_even_when_smtp_is_down() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let hub = spawn_hub_with(port).await;
    // Sign-up's own verification email fails too; that is fine here.
    let owner = hub.verified_owner("Engines").await;
    let resp = hub.invite(&owner, &unique_address(), "member").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["invitation_email_sent"], false);
    assert_eq!(hub.invitations(&owner.cookie).await.len(), 1);
}

#[tokio::test]
async fn an_invitee_looks_up_then_accepts_and_joins() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let email = unique_address();
    hub.invite(&owner, &email, "member").await;
    let token = invite_tokens(&email, 1).await.remove(0);

    let resp = hub.lookup(&token).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body,
        json!({"organization_name": "Engines", "email": email, "role": "member"})
    );
    // Looking up consumes nothing.
    assert_eq!(hub.lookup(&token).await.status(), StatusCode::OK);

    // Someone else's session in this browser is rotated away.
    let resp = hub.accept(&token, PASSWORD, Some(&owner.cookie)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookie = cookie_pair(&resp);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["user"]["email"], json!(email));
    assert_eq!(body["user"]["name"], "Grace Hopper");
    assert_eq!(body["user"]["role"], "member");
    assert_eq!(body["user"]["email_verified"], true);
    assert_eq!(body["organization"]["name"], "Engines");
    let (code, me) = hub.me(&cookie).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(me, body);
    assert_eq!(hub.me(&owner.cookie).await.0, StatusCode::UNAUTHORIZED);

    let owner = Who {
        cookie: cookie_pair(&hub.sign_in(&owner.email, PASSWORD).await),
        ..owner
    };
    let members = hub.members(&owner.cookie).await;
    assert!(members.iter().any(|m| m["email"] == json!(email)));
    assert!(hub.invitations(&owner.cookie).await.is_empty());
    assert_eq!(hub.sign_in(&email, PASSWORD).await.status(), StatusCode::OK);

    let id = body["user"]["id"].as_str().unwrap();
    let rows = hub.audited("organization.invitation_accepted").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, format!("user:{id}"));
    assert_eq!(rows[0].1, format!("user:{id}"));
    assert_eq!(rows[0].2["role"], "member");
    assert_eq!(rows[0].2["rotated"], true);
    assert_eq!(rows[0].2["peer"], PEER);
    assert_eq!(rows[0].2["organization_id"], body["organization"]["id"]);
}

#[tokio::test]
async fn used_revoked_and_expired_invitations_are_refused() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let (used, revoked, expired) = (unique_address(), unique_address(), unique_address());
    for e in [&used, &revoked, &expired] {
        hub.invite(&owner, e, "member").await;
    }
    let used_t = invite_tokens(&used, 1).await.remove(0);
    let revoked_t = invite_tokens(&revoked, 1).await.remove(0);
    let expired_t = invite_tokens(&expired, 1).await.remove(0);

    assert_eq!(
        hub.accept(&used_t, PASSWORD, None).await.status(),
        StatusCode::CREATED
    );
    let pending = hub.invitations(&owner.cookie).await;
    let id_of = |email: &str| {
        pending.iter().find(|i| i["email"] == json!(email)).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let revoked_id = id_of(&revoked);
    let resp = hub
        .post_empty(
            &format!("/api/organization/invitations/{revoked_id}/revoke"),
            Some(&owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let rows = hub.audited("organization.invitation_revoked").await;
    assert_eq!(
        rows[0].2,
        json!({"invitation_id": revoked_id, "email": revoked, "reason": "revoked"})
    );
    // Revoking again: no longer pending.
    let resp = hub
        .post_empty(
            &format!("/api/organization/invitations/{revoked_id}/revoke"),
            Some(&owner.cookie),
        )
        .await;
    assert_error(resp, StatusCode::NOT_FOUND, "no such pending invitation").await;

    sqlx::query("UPDATE invitations SET expires_at = now() - interval '1 second' WHERE email = $1")
        .bind(&expired)
        .execute(&hub.db.pool)
        .await
        .unwrap();

    for token in [
        &used_t,
        &revoked_t,
        &expired_t,
        &"A".repeat(43),
        &"garbage".to_string(),
    ] {
        assert_error(
            hub.lookup(token).await,
            StatusCode::BAD_REQUEST,
            BAD_INVITATION,
        )
        .await;
        assert_error(
            hub.accept(token, PASSWORD, None).await,
            StatusCode::BAD_REQUEST,
            BAD_INVITATION,
        )
        .await;
    }
    assert!(hub.invitations(&owner.cookie).await.is_empty());

    // The next invite purges the expired row; accepted and revoked rows stay.
    hub.invite(&owner, &unique_address(), "member").await;
    let left: Vec<String> = sqlx::query_scalar("SELECT email FROM invitations ORDER BY email")
        .fetch_all(&hub.db.pool)
        .await
        .unwrap();
    assert!(!left.contains(&expired), "{left:?}");
    assert!(left.contains(&used), "{left:?}");
    assert!(left.contains(&revoked), "{left:?}");
}

#[tokio::test]
async fn an_invitation_accepted_concurrently_joins_once() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let email = unique_address();
    hub.invite(&owner, &email, "member").await;
    let token = invite_tokens(&email, 1).await.remove(0);
    let users = hub.count("users").await;

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..5 {
        let req = hub
            .post_req("/api/invitations/accept", None)
            .json(&json!({"token": token, "name": "Grace", "password": PASSWORD}));
        tasks.spawn(async move { req.send().await.unwrap().status() });
    }
    let mut codes = Vec::new();
    while let Some(code) = tasks.join_next().await {
        codes.push(code.unwrap());
    }
    codes.sort();
    assert_eq!(
        codes,
        [
            StatusCode::CREATED,
            StatusCode::BAD_REQUEST,
            StatusCode::BAD_REQUEST,
            StatusCode::BAD_REQUEST,
            StatusCode::BAD_REQUEST,
        ]
    );
    assert_eq!(hub.count("users").await, users + 1);
}

#[tokio::test]
async fn invalid_accept_input_never_burns_the_token() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let email = unique_address();
    hub.invite(&owner, &email, "member").await;
    let token = invite_tokens(&email, 1).await.remove(0);
    assert_error(
        hub.accept(&token, &"a".repeat(11), None).await,
        StatusCode::BAD_REQUEST,
        "password must be at least 12 characters",
    )
    .await;
    let resp = hub
        .post(
            "/api/invitations/accept",
            json!({"token": token, "name": "\u{7}", "password": PASSWORD}),
            None,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        hub.accept(&token, PASSWORD, None).await.status(),
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn accepting_with_an_address_taken_elsewhere_is_409_and_keeps_the_invitation() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let email = unique_address();
    hub.invite(&owner, &email, "member").await;
    let token = invite_tokens(&email, 1).await.remove(0);
    hub.sign_up(&email.to_uppercase(), "Elsewhere").await;
    let users = hub.count("users").await;
    let sessions = hub.count("sessions").await;

    assert_error(
        hub.accept(&token, PASSWORD, None).await,
        StatusCode::CONFLICT,
        "an account with this email already exists",
    )
    .await;
    assert_eq!(hub.count("users").await, users);
    assert_eq!(hub.count("sessions").await, sessions);
    assert_eq!(hub.invitations(&owner.cookie).await.len(), 1);
    assert_eq!(hub.lookup(&token).await.status(), StatusCode::OK);
    assert!(hub
        .audited("organization.invitation_accepted")
        .await
        .is_empty());
}

// ---------------------------------------------------------------------------
// Roles and removal

#[tokio::test]
async fn owners_promote_and_demote_members() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let member = hub.add(&owner, "member").await;

    let resp = hub.set_role(&owner, &member.id, "owner").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["role"], "owner");
    assert_eq!(body["id"], json!(member.id));
    assert_eq!(hub.me(&member.cookie).await.1["user"]["role"], "owner");

    let resp = hub.set_role(&owner, &member.id, "member").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(hub.me(&member.cookie).await.1["user"]["role"], "member");

    // Same role: 200, no write, no audit.
    let resp = hub.set_role(&owner, &member.id, "member").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let rows = hub.audited("organization.member_role_changed").await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, format!("user:{}", owner.id));
    assert_eq!(rows[0].1, format!("user:{}", member.id));
    assert_eq!(rows[0].2["from"], "member");
    assert_eq!(rows[0].2["to"], "owner");
    assert_eq!(rows[1].2["from"], "owner");
    assert_eq!(rows[1].2["to"], "member");

    assert_error(
        hub.set_role(&owner, &member.id, "vendor_admin").await,
        StatusCode::BAD_REQUEST,
        "role must be owner or member",
    )
    .await;
}

#[tokio::test]
async fn a_demotion_applies_to_the_next_request() {
    let hub = spawn_hub().await;
    let a = hub.verified_owner("Engines").await;
    let b = hub.add(&a, "owner").await;
    assert_eq!(
        hub.get("/api/organization/invitations", Some(&b.cookie))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        hub.set_role(&a, &b.id, "member").await.status(),
        StatusCode::OK
    );
    assert_error(
        hub.get("/api/organization/invitations", Some(&b.cookie))
            .await,
        StatusCode::FORBIDDEN,
        OWNER_ONLY,
    )
    .await;
    assert_error(
        hub.set_role(&b, &a.id, "member").await,
        StatusCode::FORBIDDEN,
        OWNER_ONLY,
    )
    .await;
}

#[tokio::test]
async fn the_last_owner_cannot_demote_or_remove_themselves() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let audit = hub.count("audit_log").await;
    assert_error(
        hub.set_role(&owner, &owner.id, "member").await,
        StatusCode::CONFLICT,
        LAST_OWNER,
    )
    .await;
    assert_error(
        hub.remove(&owner, &owner.id).await,
        StatusCode::CONFLICT,
        "you cannot remove yourself from the organization",
    )
    .await;
    assert_eq!(hub.me(&owner.cookie).await.1["user"]["role"], "owner");
    assert_eq!(hub.count("audit_log").await, audit);

    // With a second owner, demoting yourself is allowed.
    let other = hub.add(&owner, "owner").await;
    assert_eq!(
        hub.set_role(&owner, &owner.id, "member").await.status(),
        StatusCode::OK
    );
    assert_error(
        hub.set_role(&other, &other.id, "member").await,
        StatusCode::CONFLICT,
        LAST_OWNER,
    )
    .await;
}

#[tokio::test]
async fn two_owners_demoting_each_other_at_once_leave_one_owner() {
    for _ in 0..3 {
        let hub = spawn_hub().await;
        let a = hub.verified_owner("Engines").await;
        let b = hub.add(&a, "owner").await;
        let ra = hub
            .post_req(
                &format!("/api/organization/members/{}/role", b.id),
                Some(&a.cookie),
            )
            .json(&json!({"role": "member"}));
        let rb = hub
            .post_req(
                &format!("/api/organization/members/{}/role", a.id),
                Some(&b.cookie),
            )
            .json(&json!({"role": "member"}));
        let (x, y) = tokio::join!(ra.send(), rb.send());
        let mut codes = [x.unwrap().status(), y.unwrap().status()];
        codes.sort();
        assert_eq!(codes, [StatusCode::OK, StatusCode::CONFLICT]);
        let owners: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'owner'")
            .fetch_one(&hub.db.pool)
            .await
            .unwrap();
        assert_eq!(owners, 1);
    }
}

#[tokio::test]
async fn removing_a_member_signs_them_out_and_frees_the_address() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let member = hub.add(&owner, "member").await;
    assert_eq!(hub.me(&member.cookie).await.0, StatusCode::OK);

    let resp = hub.remove(&owner, &member.id).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(hub.me(&member.cookie).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(hub.members(&owner.cookie).await.len(), 1);
    let rows = hub.audited("organization.member_removed").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, format!("user:{}", owner.id));
    assert_eq!(rows[0].1, format!("user:{}", member.id));
    assert_eq!(rows[0].2["email"], json!(member.email));
    assert_eq!(rows[0].2["role"], "member");
    assert_eq!(rows[0].2["sessions_ended"], 1);

    assert_error(
        hub.remove(&owner, &member.id).await,
        StatusCode::NOT_FOUND,
        "no such member",
    )
    .await;
    // The address can start its own organization.
    hub.sign_up(&member.email, "Fresh Start").await;
}

#[tokio::test]
async fn another_organizations_ids_are_404_and_change_nothing() {
    let hub = spawn_hub().await;
    let a = hub.verified_owner("A").await;
    let b = hub.verified_owner("B").await;
    let b_member = hub.add(&b, "member").await;
    hub.invite(&b, &unique_address(), "member").await;
    let b_inv = hub.invitations(&b.cookie).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let before = hub.counts().await;
    let snapshot = hub.snapshot().await;

    for id in [&b.id, &b_member.id] {
        assert_error(
            hub.set_role(&a, id, "member").await,
            StatusCode::NOT_FOUND,
            "no such member",
        )
        .await;
        assert_error(
            hub.set_role(&a, id, "owner").await,
            StatusCode::NOT_FOUND,
            "no such member",
        )
        .await;
        assert_error(
            hub.remove(&a, id).await,
            StatusCode::NOT_FOUND,
            "no such member",
        )
        .await;
    }
    let resp = hub
        .post_empty(
            &format!("/api/organization/invitations/{b_inv}/revoke"),
            Some(&a.cookie),
        )
        .await;
    assert_error(resp, StatusCode::NOT_FOUND, "no such pending invitation").await;
    // Ids that are not UUIDs are the same 404s.
    assert_error(
        hub.remove(&a, "not-a-uuid").await,
        StatusCode::NOT_FOUND,
        "no such member",
    )
    .await;
    let resp = hub
        .post_empty("/api/organization/invitations/123/revoke", Some(&a.cookie))
        .await;
    assert_error(resp, StatusCode::NOT_FOUND, "no such pending invitation").await;

    assert_eq!(hub.counts().await, before);
    assert_eq!(hub.snapshot().await, snapshot);
    assert_eq!(hub.invitations(&b.cookie).await.len(), 1);
    assert_eq!(hub.members(&b.cookie).await.len(), 2);
}

#[tokio::test]
async fn vendor_admins_are_never_targets_and_cannot_manage() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let org: Uuid = sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1::uuid")
        .bind(&owner.id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    let hash = wipe_hub::accounts::hash_password(PASSWORD.into())
        .await
        .unwrap();
    let admin_email = unique_address();
    let admin: Uuid = sqlx::query_scalar(
        "INSERT INTO users (organization_id, email, name, password_hash, role, email_verified_at) \
         VALUES ($1, $2, 'Vendor', $3, 'vendor_admin', now()) RETURNING id",
    )
    .bind(org)
    .bind(&admin_email)
    .bind(&hash)
    .fetch_one(&hub.db.pool)
    .await
    .unwrap();
    let id = admin.to_string();
    assert_error(
        hub.set_role(&owner, &id, "member").await,
        StatusCode::NOT_FOUND,
        "no such member",
    )
    .await;
    assert_error(
        hub.remove(&owner, &id).await,
        StatusCode::NOT_FOUND,
        "no such member",
    )
    .await;

    let resp = hub.sign_in(&admin_email, PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let admin_who = Who {
        id: id.clone(),
        email: admin_email,
        cookie: cookie_pair(&resp),
    };
    assert_error(
        hub.invite(&admin_who, &unique_address(), "member").await,
        StatusCode::FORBIDDEN,
        OWNER_ONLY,
    )
    .await;
    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE id = $1")
        .bind(admin)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    assert_eq!(role, "vendor_admin");
}

#[tokio::test]
async fn the_whole_team_lifecycle_is_visible_and_audited_in_order() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let invitee = hub.add(&owner, "member").await;
    let listed = |members: Vec<Value>| {
        members
            .into_iter()
            .find(|m| m["id"] == json!(invitee.id))
            .map(|m| m["role"].as_str().unwrap().to_string())
    };
    assert_eq!(
        listed(hub.members(&owner.cookie).await).as_deref(),
        Some("member")
    );
    assert_eq!(hub.me(&invitee.cookie).await.1["user"]["role"], "member");

    hub.set_role(&owner, &invitee.id, "owner").await;
    assert_eq!(
        listed(hub.members(&owner.cookie).await).as_deref(),
        Some("owner")
    );
    assert_eq!(hub.me(&invitee.cookie).await.1["user"]["role"], "owner");

    hub.set_role(&owner, &invitee.id, "member").await;
    assert_eq!(
        listed(hub.members(&owner.cookie).await).as_deref(),
        Some("member")
    );

    assert_eq!(
        hub.remove(&owner, &invitee.id).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(listed(hub.members(&owner.cookie).await), None);
    assert_eq!(hub.me(&invitee.cookie).await.0, StatusCode::UNAUTHORIZED);

    let team: Vec<String> = hub
        .actions()
        .await
        .into_iter()
        .filter(|a| a.starts_with("organization."))
        .collect();
    assert_eq!(
        team,
        [
            "organization.member_invited",
            "organization.invitation_accepted",
            "organization.member_role_changed",
            "organization.member_role_changed",
            "organization.member_removed",
        ]
    );
}

// ---------------------------------------------------------------------------
// Review pass 1 rows: limits, concurrency, revocation reasons

/// `failures` on a throttle row, `None` when the key has no row.
async fn throttle_failures(hub: &Hub, key: &str) -> Option<i32> {
    sqlx::query_scalar("SELECT failures FROM auth_throttle WHERE key = $1")
        .bind(key)
        .fetch_optional(&hub.db.pool)
        .await
        .unwrap()
}

/// The id of the organization's pending invitation for `email`.
async fn pending_id(hub: &Hub, owner: &Who, email: &str) -> String {
    hub.invitations(&owner.cookie)
        .await
        .into_iter()
        .find(|i| i["email"] == json!(email))
        .unwrap_or_else(|| panic!("no pending invitation for {email}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_successful_password_change_clears_the_lock_count() {
    let hub = spawn_hub().await;
    let owner = hub.sign_up(&unique_address(), "Engines").await;
    for _ in 0..4 {
        let resp = hub
            .post(
                "/api/auth/password",
                json!({"current_password": "not my password", "new_password": NEW_PASSWORD}),
                Some(&owner.cookie),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
    let email_key = format!("email:{}", owner.email.to_lowercase());
    assert_eq!(throttle_failures(&hub, &email_key).await, Some(4));
    let resp = hub
        .post(
            "/api/auth/password",
            json!({"current_password": PASSWORD, "new_password": NEW_PASSWORD}),
            Some(&owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(throttle_failures(&hub, &email_key).await, None);
    // Four failures kept, the success's own charge refunded.
    assert_eq!(throttle_failures(&hub, PEER).await, Some(4));
    assert_error(
        hub.sign_in(&owner.email, "still not my password").await,
        StatusCode::UNAUTHORIZED,
        "invalid email or password",
    )
    .await;
}

#[tokio::test]
async fn an_organization_is_limited_to_twenty_invitations_an_hour() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let org_id = hub.me(&owner.cookie).await.1["organization"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let org_key = format!("mail:org:{org_id}");
    // Keep the per-IP limit (also 20, all from 127.0.0.1) out of the way.
    let reset_ip = || async {
        sqlx::query("DELETE FROM auth_throttle WHERE key LIKE 'mail:ip:%'")
            .execute(&hub.db.pool)
            .await
            .unwrap();
    };
    for _ in 0..20 {
        reset_ip().await;
        let resp = hub.invite(&owner, &unique_address(), "member").await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }
    reset_ip().await;
    let last = unique_address();
    let resp = hub.invite(&owner, &last, "member").await;
    assert!(resp.headers().get(RETRY_AFTER).is_some());
    assert_error(resp, StatusCode::TOO_MANY_REQUESTS, MAIL_LOCKED).await;

    assert_eq!(hub.count("invitations").await, 20);
    assert_eq!(mail_count(&last).await, 0);
    let locked: Vec<_> = hub
        .audited("auth.mail_locked")
        .await
        .into_iter()
        .filter(|(_, subject, _)| *subject == org_key)
        .collect();
    assert_eq!(locked.len(), 1, "{locked:?}");
    // The refused request's IP and address charges were given back.
    assert_eq!(throttle_failures(&hub, "mail:ip:127.0.0.1").await, Some(0));
    assert_eq!(
        throttle_failures(&hub, &format!("mail:to:{}", last.to_lowercase())).await,
        Some(0)
    );
}

#[tokio::test]
async fn concurrent_invites_for_one_address_leave_one_pending() {
    for _ in 0..3 {
        let hub = spawn_hub().await;
        let owner = hub.verified_owner("Engines").await;
        let email = unique_address();
        let a = hub
            .post_req("/api/organization/invitations", Some(&owner.cookie))
            .json(&json!({"email": email, "role": "member"}));
        let b = hub
            .post_req("/api/organization/invitations", Some(&owner.cookie))
            .json(&json!({"email": email, "role": "owner"}));
        let (x, y) = tokio::join!(a.send(), b.send());
        assert_eq!(x.unwrap().status(), StatusCode::CREATED);
        assert_eq!(y.unwrap().status(), StatusCode::CREATED);
        assert_eq!(hub.invitations(&owner.cookie).await.len(), 1);
        let revoked = hub.audited("organization.invitation_revoked").await;
        assert_eq!(revoked.len(), 1, "{revoked:?}");
        assert_eq!(revoked[0].2["reason"], "replaced");
    }
}

#[tokio::test]
async fn a_re_invite_audits_the_replaced_invitation() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let email = unique_address();
    hub.invite(&owner, &email, "member").await;
    let first = pending_id(&hub, &owner, &email).await;
    hub.invite(&owner, &email, "member").await;
    let second = pending_id(&hub, &owner, &email).await;
    assert_ne!(first, second);
    let rows = hub.audited("organization.invitation_revoked").await;
    assert_eq!(
        rows,
        vec![(
            format!("user:{}", owner.id),
            rows[0].1.clone(),
            json!({"invitation_id": first, "email": email, "reason": "replaced"}),
        )]
    );
    assert!(rows[0].1.starts_with("organization:"), "{}", rows[0].1);
}

#[tokio::test]
async fn removing_an_owner_revokes_the_invitations_they_sent() {
    let hub = spawn_hub().await;
    let a = hub.verified_owner("Engines").await;
    let b = hub.add(&a, "owner").await;
    let (x, y) = (unique_address(), unique_address());
    assert_eq!(
        hub.invite(&b, &x, "owner").await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        hub.invite(&b, &y, "member").await.status(),
        StatusCode::CREATED
    );
    let x_token = invite_tokens(&x, 1).await.remove(0);
    let y_token = invite_tokens(&y, 1).await.remove(0);
    assert_eq!(
        hub.accept(&y_token, PASSWORD, None).await.status(),
        StatusCode::CREATED
    );
    let x_id = pending_id(&hub, &a, &x).await;

    assert_eq!(hub.remove(&a, &b.id).await.status(), StatusCode::NO_CONTENT);
    assert_error(
        hub.accept(&x_token, PASSWORD, None).await,
        StatusCode::BAD_REQUEST,
        BAD_INVITATION,
    )
    .await;
    assert!(hub.invitations(&a.cookie).await.is_empty());
    let rows = hub.audited("organization.invitation_revoked").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, format!("user:{}", a.id));
    assert_eq!(
        rows[0].2,
        json!({"invitation_id": x_id, "email": x, "reason": "sender_removed"})
    );
    // The accepted invitation survives, its sender gone.
    let senders: Vec<Option<Uuid>> =
        sqlx::query_scalar("SELECT invited_by FROM invitations WHERE email = $1")
            .bind(&y)
            .fetch_all(&hub.db.pool)
            .await
            .unwrap();
    assert_eq!(senders, vec![None]);
    // Revocation came before the removal, in one transaction.
    let team: Vec<String> = hub
        .actions()
        .await
        .into_iter()
        .filter(|a| a == "organization.invitation_revoked" || a == "organization.member_removed")
        .collect();
    assert_eq!(
        team,
        [
            "organization.invitation_revoked",
            "organization.member_removed"
        ]
    );
}

#[tokio::test]
async fn demoting_an_owner_revokes_the_invitations_they_sent() {
    let hub = spawn_hub().await;
    let a = hub.verified_owner("Engines").await;
    let b = hub.add(&a, "owner").await;
    let x = unique_address();
    assert_eq!(
        hub.invite(&b, &x, "owner").await.status(),
        StatusCode::CREATED
    );
    let x_token = invite_tokens(&x, 1).await.remove(0);
    let x_id = pending_id(&hub, &a, &x).await;
    // A's own invitation is untouched.
    let z = unique_address();
    hub.invite(&a, &z, "member").await;

    assert_eq!(
        hub.set_role(&a, &b.id, "member").await.status(),
        StatusCode::OK
    );
    assert_error(
        hub.accept(&x_token, PASSWORD, None).await,
        StatusCode::BAD_REQUEST,
        BAD_INVITATION,
    )
    .await;
    let pending = hub.invitations(&a.cookie).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["email"], json!(z));
    let rows = hub.audited("organization.invitation_revoked").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0].2,
        json!({"invitation_id": x_id, "email": x, "reason": "sender_demoted"})
    );
    // Promoting back does not revive it, and a member→owner change revokes
    // nothing.
    assert_eq!(
        hub.set_role(&a, &b.id, "owner").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        hub.audited("organization.invitation_revoked").await.len(),
        1
    );
}

#[tokio::test]
async fn accepting_while_someone_else_is_signed_in_rotates_their_session() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Engines").await;
    let c = hub.sign_up(&unique_address(), "Elsewhere").await;
    let email = unique_address();
    hub.invite(&owner, &email, "member").await;
    let token = invite_tokens(&email, 1).await.remove(0);

    let resp = hub.accept(&token, PASSWORD, Some(&c.cookie)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookie = cookie_pair(&resp);
    assert_eq!(hub.me(&c.cookie).await.0, StatusCode::UNAUTHORIZED);
    let (code, me) = hub.me(&cookie).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(me["user"]["email"], json!(email));
    let rows = hub.audited("organization.invitation_accepted").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].2["rotated"], true);
    // C's account itself is untouched.
    assert_eq!(
        hub.sign_in(&c.email, PASSWORD).await.status(),
        StatusCode::OK
    );
}
