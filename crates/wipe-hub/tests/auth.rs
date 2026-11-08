//! `/api/auth/*` over real HTTP against a migrated test database and
//! Mailpit (portal-accounts stories 1, 2 and 3). Needs `docker compose up -d`;
//! fails, never skips, when the compose services are down.
//!
//! `Hub::post`/`post_empty` send a valid CSRF token (fetched once per hub);
//! the CSRF tests build raw requests to send none or a bad one.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::header::{CACHE_CONTROL, COOKIE, RETRY_AFTER, SET_COOKIE};
use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::{unique_address, Mailpit, TestDb};
use tokio::net::TcpListener;
use wipe_hub::config::{DEFAULT_MAIL_FROM, DEFAULT_SMTP_HOST, DEFAULT_SMTP_PORT};
use wipe_hub::{AppState, Args, Config};

const PASSWORD: &str = "correct horse battery";
const MAIL_WAIT: Duration = Duration::from_secs(10);

struct Hub {
    addr: SocketAddr,
    http: reqwest::Client,
    db: TestDb,
    /// A token from `GET /api/auth/csrf`, sent as cookie and header.
    csrf: String,
}

async fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

async fn spawn_hub_with(smtp_port: u16, public_base_url: &str) -> Hub {
    spawn_hub_from(smtp_port, public_base_url, DEFAULT_MAIL_FROM).await
}

fn hub_state(db: &TestDb, smtp_port: u16, public_base_url: &str, mail_from: &str) -> AppState {
    let args = Args {
        addr: "127.0.0.1:0".parse().unwrap(),
        database_url: db.url.clone(),
        smtp_host: DEFAULT_SMTP_HOST.into(),
        smtp_port,
        mail_from: mail_from.into(),
        public_base_url: public_base_url.into(),
        payment_provider: wipe_hub::PaymentProvider::Fake,
        stripe_webhook_secret: None,
        vendor_root_seed: None,
        portal_dir: None,
    };
    let mut config = Config::from_args(args).unwrap();
    config.portal_dir = None;
    AppState::new(config).unwrap()
}

async fn spawn_hub_from(smtp_port: u16, public_base_url: &str, mail_from: &str) -> Hub {
    let db = TestDb::migrated().await;
    let state = hub_state(&db, smtp_port, public_base_url, mail_from);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(wipe_hub::serve_on(listener, state));
    // No cookie store: every test passes cookies explicitly.
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let csrf = fetch_csrf(&http, addr, None).await.0;
    Hub {
        addr,
        http,
        db,
        csrf,
    }
}

/// `GET /api/auth/csrf`: the token and the raw `wipehub_csrf` `Set-Cookie`,
/// if one was set.
async fn fetch_csrf(
    http: &reqwest::Client,
    addr: SocketAddr,
    cookie: Option<&str>,
) -> (String, Option<String>) {
    let mut req = http.get(format!("http://{addr}/api/auth/csrf"));
    if let Some(c) = cookie {
        req = req.header(COOKIE, c);
    }
    let resp = req.send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(CACHE_CONTROL).unwrap().to_str().unwrap(),
        "no-store"
    );
    let set = resp
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .find(|v| v.starts_with("wipehub_csrf="));
    let body: Value = resp.json().await.unwrap();
    let obj = body.as_object().unwrap();
    assert_eq!(obj.len(), 1, "{body}");
    (obj["csrf_token"].as_str().unwrap().to_string(), set)
}

async fn spawn_hub() -> Hub {
    spawn_hub_with(DEFAULT_SMTP_PORT, "http://127.0.0.1:7900").await
}

impl Hub {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// A POST to `path` carrying `cookie` plus a valid CSRF cookie and header.
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

    async fn me(&self, cookie: Option<&str>) -> (StatusCode, Value) {
        let mut req = self.http.get(self.url("/api/auth/me"));
        if let Some(c) = cookie {
            req = req.header(COOKIE, c);
        }
        let resp = req.send().await.unwrap();
        let code = resp.status();
        (code, resp.json().await.unwrap())
    }

    async fn sign_up(&self, email: &str) -> (Value, String) {
        let resp = self
            .post(
                "/api/auth/sign-up",
                json!({
                    "email": email,
                    "password": PASSWORD,
                    "name": "Ada Lovelace",
                    "organization_name": "Analytical Engines Ltd",
                }),
                None,
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let cookie = cookie_pair(&resp);
        (resp.json().await.unwrap(), cookie)
    }

    async fn sign_in(&self, email: &str, password: &str) -> Response {
        self.post(
            "/api/auth/sign-in",
            json!({"email": email, "password": password}),
            None,
        )
        .await
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    /// Audit rows as (actor, action, subject), oldest first.
    async fn audit(&self) -> Vec<(String, String, String)> {
        sqlx::query_as("SELECT actor, action, subject FROM audit_log ORDER BY id")
            .fetch_all(&self.db.pool)
            .await
            .unwrap()
    }

    async fn audited(&self, action: &str) -> Vec<(String, String)> {
        self.audit()
            .await
            .into_iter()
            .filter(|(_, a, _)| a == action)
            .map(|(actor, _, subject)| (actor, subject))
            .collect()
    }
}

/// The raw `Set-Cookie` header for the session cookie.
fn set_cookie(resp: &Response) -> String {
    resp.headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .find(|v| v.starts_with("wipehub_session="))
        .expect("a wipehub_session Set-Cookie header")
}

/// `name=value` from the session `Set-Cookie`, ready for a `Cookie` header.
fn cookie_pair(resp: &Response) -> String {
    set_cookie(resp).split(';').next().unwrap().to_string()
}

async fn error_of(resp: Response) -> (StatusCode, String) {
    let code = resp.status();
    let body: Value = resp.json().await.unwrap();
    let obj = body.as_object().expect("error body is an object");
    assert_eq!(obj.len(), 1, "{body}");
    (code, obj["error"].as_str().unwrap().to_string())
}

/// The `token` query parameter of the verification link in `text`.
fn token_from_mail(text: &str, base: &str) -> String {
    token_in(text, base, "verify-email")
}

/// The `token` query parameter of the `{base}/{page}?token=` link in `text`.
fn token_in(text: &str, base: &str, page: &str) -> String {
    let prefix = format!("{base}/{page}?token=");
    let start = text
        .find(&prefix)
        .unwrap_or_else(|| panic!("no {page} link in {text:?}"))
        + prefix.len();
    text[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

async fn verification_token(addr: &str) -> String {
    let msg = Mailpit::new().wait_for_message_to(addr, MAIL_WAIT).await;
    token_from_mail(&msg.text, "http://127.0.0.1:7900")
}

#[tokio::test]
async fn sign_up_creates_owner_org_session_mail_and_audit() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (body, cookie) = hub.sign_up(&email).await;

    assert_eq!(body["verification_email_sent"], json!(true));
    assert_eq!(body["user"]["email"], json!(email));
    assert_eq!(body["user"]["name"], json!("Ada Lovelace"));
    assert_eq!(body["user"]["role"], json!("owner"));
    assert_eq!(body["user"]["email_verified"], json!(false));
    assert_eq!(
        body["organization"]["name"],
        json!("Analytical Engines Ltd")
    );
    assert!(body["user"].get("password_hash").is_none());
    assert!(!body.to_string().contains(PASSWORD));

    assert_eq!(hub.count("organizations").await, 1);
    assert_eq!(hub.count("users").await, 1);
    let user_id = body["user"]["id"].as_str().unwrap().to_string();
    let (hash, org_id): (String, uuid::Uuid) =
        sqlx::query_as("SELECT password_hash, organization_id FROM users")
            .fetch_one(&hub.db.pool)
            .await
            .unwrap();
    assert!(hash.starts_with("$argon2id$"), "{hash}");
    assert_eq!(body["organization"]["id"], json!(org_id.to_string()));
    let who = format!("user:{user_id}");
    assert_eq!(hub.audited("auth.sign_up").await, vec![(who.clone(), who)]);

    // Signed in at once, unverified.
    let (code, me) = hub.me(Some(&cookie)).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(me["user"]["id"], json!(user_id));
    assert_eq!(me["user"]["email_verified"], json!(false));
    assert_eq!(me["organization"]["id"], body["organization"]["id"]);

    // Mail landed with a link under the public base URL; only hashes stored.
    let msg = Mailpit::new().wait_for_message_to(&email, MAIL_WAIT).await;
    let token = token_from_mail(&msg.text, "http://127.0.0.1:7900");
    assert_eq!(token.len(), 43, "{}", msg.text);
    let session = cookie.strip_prefix("wipehub_session=").unwrap();
    for (table, raw) in [("auth_tokens", token.as_str()), ("sessions", session)] {
        let bytes = URL_SAFE_NO_PAD.decode(raw).unwrap();
        assert_eq!(bytes.len(), 32, "{table}");
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let count = |value: Vec<u8>| {
            let pool = hub.db.pool.clone();
            let sql = format!("SELECT count(*) FROM {table} WHERE token_hash = $1");
            async move {
                sqlx::query_scalar::<_, i64>(&sql)
                    .bind(value)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        assert_eq!(count(hash.to_vec()).await, 1, "{table}: hash stored once");
        assert_eq!(count(bytes).await, 0, "{table}: raw token stored");
        assert_eq!(hub.count(table).await, 1, "{table}");
    }
}

#[tokio::test]
async fn verification_email_uses_mail_from_and_subject() {
    let hub = spawn_hub_from(
        DEFAULT_SMTP_PORT,
        "http://127.0.0.1:7900",
        "Hub Tests <hub-tests@wipehub.test>",
    )
    .await;
    let email = unique_address();
    hub.sign_up(&email).await;
    let msg = Mailpit::new().wait_for_message_to(&email, MAIL_WAIT).await;
    assert_eq!(msg.from.address, "hub-tests@wipehub.test");
    assert_eq!(msg.from.name, "Hub Tests");
    assert_eq!(msg.subject, wipe_hub::mail::VERIFICATION_SUBJECT);
}

#[tokio::test]
async fn non_json_body_is_415_with_a_json_error() {
    let hub = spawn_hub().await;
    let resp = hub
        .post_req("/api/auth/sign-up", None)
        .header("content-type", "text/plain")
        .body("email=a@b.co")
        .send()
        .await
        .unwrap();
    let (code, msg) = error_of(resp).await;
    assert_eq!(code, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(!msg.is_empty());
}

#[tokio::test]
async fn oversized_sign_in_is_the_generic_401_with_a_bounded_audit_subject() {
    let hub = spawn_hub().await;
    let long_email = format!("{}@example.com", "a".repeat(5000));
    for (email, password) in [
        (long_email.clone(), PASSWORD.to_string()),
        (unique_address(), "p".repeat(5000)),
    ] {
        let resp = hub.sign_in(&email, &password).await;
        assert_eq!(
            error_of(resp).await,
            (StatusCode::UNAUTHORIZED, "invalid email or password".into())
        );
    }
    let failed = hub.audited("auth.sign_in_failed").await;
    assert_eq!(failed.len(), 2);
    for (actor, subject) in &failed {
        assert_eq!(actor, "anonymous");
        assert!(subject.chars().count() <= "email:".len() + 254, "{subject}");
    }
}

#[tokio::test]
async fn session_cookie_attributes() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let resp = hub
        .post(
            "/api/auth/sign-up",
            json!({"email": email, "password": PASSWORD, "name": "A", "organization_name": "O"}),
            None,
        )
        .await;
    let sign_up = set_cookie(&resp);
    let resp = hub.sign_in(&email, PASSWORD).await;
    let sign_in = set_cookie(&resp);
    for c in [&sign_up, &sign_in] {
        let attrs: Vec<&str> = c.split(';').map(str::trim).collect();
        assert!(attrs.contains(&"HttpOnly"), "{c}");
        assert!(attrs.contains(&"SameSite=Lax"), "{c}");
        assert!(attrs.contains(&"Path=/"), "{c}");
        assert!(attrs.contains(&"Max-Age=1209600"), "{c}");
        assert!(!attrs.contains(&"Secure"), "{c}");
    }
    let session_ttl_days: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM max(expires_at - created_at))::float8 / 86400 FROM sessions",
    )
    .fetch_one(&hub.db.pool)
    .await
    .unwrap();
    assert!((session_ttl_days - 14.0).abs() < 1e-6, "{session_ttl_days}");
}

#[tokio::test]
async fn https_public_base_url_sets_secure() {
    let hub = spawn_hub_with(DEFAULT_SMTP_PORT, "https://portal.example.com").await;
    let email = unique_address();
    let resp = hub
        .post(
            "/api/auth/sign-up",
            json!({"email": email, "password": PASSWORD, "name": "A", "organization_name": "O"}),
            None,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let c = set_cookie(&resp);
    assert!(c.split(';').map(str::trim).any(|a| a == "Secure"), "{c}");
    let msg = Mailpit::new().wait_for_message_to(&email, MAIL_WAIT).await;
    assert!(
        msg.text
            .contains("https://portal.example.com/verify-email?token="),
        "{}",
        msg.text
    );
}

#[tokio::test]
async fn duplicate_email_in_any_case_is_409_and_writes_nothing() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    let resp = hub
        .post(
            "/api/auth/sign-up",
            json!({
                "email": email.to_uppercase(),
                "password": PASSWORD,
                "name": "Other",
                "organization_name": "Other Org",
            }),
            None,
        )
        .await;
    assert_eq!(
        error_of(resp).await,
        (
            StatusCode::CONFLICT,
            "an account with this email already exists".into()
        )
    );
    assert_eq!(hub.count("organizations").await, 1);
    assert_eq!(hub.count("users").await, 1);
    assert_eq!(hub.count("sessions").await, 1);
    assert_eq!(hub.audited("auth.sign_up").await.len(), 1);
}

#[tokio::test]
async fn invalid_sign_up_is_400_naming_the_problem_and_writes_nothing() {
    let hub = spawn_hub().await;
    let good = || {
        json!({
            "email": unique_address(),
            "password": PASSWORD,
            "name": "Ada",
            "organization_name": "Org",
        })
    };
    let with = |field: &str, value: Value| {
        let mut b = good();
        b[field] = value;
        b
    };
    let without = |field: &str| {
        let mut b = good();
        b.as_object_mut().unwrap().remove(field);
        b
    };
    let cases = [
        (with("password", json!("elevenchars")), "password"),
        (with("password", json!("x".repeat(1025))), "password"),
        (with("name", json!("   ")), "name"),
        (with("organization_name", json!("")), "organization_name"),
        (with("email", json!("not-an-email")), "email"),
        (with("email", json!("a@")), "email"),
        (without("password"), "password"),
        (without("email"), "email"),
        (without("organization_name"), "organization_name"),
    ];
    for (body, field) in cases {
        let resp = hub.post("/api/auth/sign-up", body.clone(), None).await;
        let (code, msg) = error_of(resp).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}: {msg}");
        assert!(msg.contains(field), "{body}: {msg}");
    }
    // Malformed JSON is a JSON 400 too.
    let resp = hub
        .post_req("/api/auth/sign-up", None)
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(error_of(resp).await.0, StatusCode::BAD_REQUEST);

    for table in [
        "organizations",
        "users",
        "sessions",
        "auth_tokens",
        "audit_log",
    ] {
        assert_eq!(hub.count(table).await, 0, "{table}");
    }
}

#[tokio::test]
async fn mail_down_at_sign_up_still_creates_the_account() {
    let hub = spawn_hub_with(closed_port().await, "http://127.0.0.1:7900").await;
    let resp = hub
        .post(
            "/api/auth/sign-up",
            json!({
                "email": unique_address(),
                "password": PASSWORD,
                "name": "Ada",
                "organization_name": "Org",
            }),
            None,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookie = cookie_pair(&resp);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["verification_email_sent"], json!(false));
    assert_eq!(hub.count("users").await, 1);
    assert_eq!(hub.me(Some(&cookie)).await.0, StatusCode::OK);

    // Resending while mail is down is a 502.
    let resp = hub
        .post_empty("/api/auth/verification-email", Some(&cookie))
        .await;
    assert_eq!(error_of(resp).await.0, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn verify_email_with_the_mailed_token() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (body, cookie) = hub.sign_up(&email).await;
    let token = verification_token(&email).await;

    let resp = hub
        .post("/api/auth/verify-email", json!({"token": token}), None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.json::<Value>().await.unwrap(),
        json!({"email_verified": true})
    );
    let (_, me) = hub.me(Some(&cookie)).await;
    assert_eq!(me["user"]["email_verified"], json!(true));
    let who = format!("user:{}", body["user"]["id"].as_str().unwrap());
    assert_eq!(
        hub.audited("auth.email_verified").await,
        vec![(who.clone(), who)]
    );

    // Reuse is rejected.
    let resp = hub
        .post("/api/auth/verify-email", json!({"token": token}), None)
        .await;
    assert_eq!(
        error_of(resp).await,
        (
            StatusCode::BAD_REQUEST,
            "this verification link is invalid or has expired".into()
        )
    );
    assert_eq!(hub.audited("auth.email_verified").await.len(), 1);

    // Resend once verified is a 409.
    let resp = hub
        .post_empty("/api/auth/verification-email", Some(&cookie))
        .await;
    assert_eq!(error_of(resp).await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn expired_and_unknown_tokens_are_rejected_and_leave_the_user_unverified() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (_, cookie) = hub.sign_up(&email).await;
    let token = verification_token(&email).await;
    sqlx::query("UPDATE auth_tokens SET expires_at = now() - interval '1 second'")
        .execute(&hub.db.pool)
        .await
        .unwrap();

    let garbage_43 = "A".repeat(43);
    for t in [token.as_str(), "garbage", "", garbage_43.as_str()] {
        let resp = hub
            .post("/api/auth/verify-email", json!({"token": t}), None)
            .await;
        assert_eq!(
            error_of(resp).await,
            (
                StatusCode::BAD_REQUEST,
                "this verification link is invalid or has expired".into()
            ),
            "{t}"
        );
    }
    let (_, me) = hub.me(Some(&cookie)).await;
    assert_eq!(me["user"]["email_verified"], json!(false));
    assert!(hub.audited("auth.email_verified").await.is_empty());
}

#[tokio::test]
async fn verification_tokens_expire_after_24_hours() {
    let hub = spawn_hub().await;
    hub.sign_up(&unique_address()).await;
    let hours: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM expires_at - created_at)::float8 / 3600 FROM auth_tokens",
    )
    .fetch_one(&hub.db.pool)
    .await
    .unwrap();
    assert!((hours - 24.0).abs() < 1e-6, "{hours}");
}

#[tokio::test]
async fn resend_sends_a_new_working_link() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (_, cookie) = hub.sign_up(&email).await;
    let first = verification_token(&email).await;

    let resp = hub
        .post_empty("/api/auth/verification-email", Some(&cookie))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Wait for the second message, then use its token.
    let mailpit = Mailpit::new();
    let deadline = std::time::Instant::now() + MAIL_WAIT;
    let second = loop {
        let msgs = mailpit.messages_to(&email).await;
        if msgs.len() >= 2 {
            break mailpit.message(&msgs[0].id).await;
        }
        assert!(std::time::Instant::now() < deadline, "no second email");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let second = token_from_mail(&second.text, "http://127.0.0.1:7900");
    assert_ne!(first, second);
    let resp = hub
        .post("/api/auth/verify-email", json!({"token": second}), None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // The first link was retired when the second one verified.
    let resp = hub
        .post("/api/auth/verify-email", json!({"token": first}), None)
        .await;
    assert_eq!(error_of(resp).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(hub.audited("auth.email_verified").await.len(), 1);

    // Not signed in: 401.
    let resp = hub.post_empty("/api/auth/verification-email", None).await;
    assert_eq!(
        error_of(resp).await,
        (StatusCode::UNAUTHORIZED, "not signed in".into())
    );
}

#[tokio::test]
async fn sign_in_issues_a_new_session_and_both_work_until_signed_out() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (body, sign_up_cookie) = hub.sign_up(&email).await;

    let resp = hub.sign_in(&email.to_uppercase(), PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let sign_in_cookie = cookie_pair(&resp);
    let signed_in: Value = resp.json().await.unwrap();
    assert_eq!(signed_in["user"], body["user"]);
    assert_eq!(signed_in["organization"], body["organization"]);
    assert_ne!(sign_in_cookie, sign_up_cookie);

    assert_eq!(hub.me(Some(&sign_up_cookie)).await.0, StatusCode::OK);
    assert_eq!(hub.me(Some(&sign_in_cookie)).await.0, StatusCode::OK);
    let who = format!("user:{}", body["user"]["id"].as_str().unwrap());
    assert_eq!(
        hub.audited("auth.sign_in").await,
        vec![(who.clone(), who.clone())]
    );

    // Signing out one session leaves the other working.
    let resp = hub
        .post_empty("/api/auth/sign-out", Some(&sign_in_cookie))
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let cleared = set_cookie(&resp);
    assert!(cleared.starts_with("wipehub_session=;"), "{cleared}");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    assert_eq!(
        hub.me(Some(&sign_in_cookie)).await,
        (StatusCode::UNAUTHORIZED, json!({"error": "not signed in"}))
    );
    assert_eq!(hub.me(Some(&sign_up_cookie)).await.0, StatusCode::OK);
    assert_eq!(hub.audited("auth.sign_out").await, vec![(who.clone(), who)]);
}

#[tokio::test]
async fn bad_sign_in_is_one_generic_401_and_is_audited() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    let unknown = unique_address();

    let wrong = hub.sign_in(&email.to_uppercase(), "wrong password!!").await;
    assert!(wrong.headers().get(SET_COOKIE).is_none());
    let wrong = (wrong.status(), wrong.text().await.unwrap());
    let missing = hub.sign_in(&unknown, PASSWORD).await;
    let missing = (missing.status(), missing.text().await.unwrap());
    assert_eq!(wrong.0, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong, missing);
    assert_eq!(
        serde_json::from_str::<Value>(&wrong.1).unwrap(),
        json!({"error": "invalid email or password"})
    );

    assert_eq!(
        hub.audited("auth.sign_in_failed").await,
        vec![
            (
                "anonymous".into(),
                format!("email:{}", email.to_lowercase())
            ),
            ("anonymous".into(), format!("email:{unknown}")),
        ]
    );
    assert_eq!(hub.count("sessions").await, 1);
}

#[tokio::test]
async fn me_and_sign_out_without_a_session() {
    let hub = spawn_hub().await;
    let not_signed_in = (StatusCode::UNAUTHORIZED, json!({"error": "not signed in"}));
    assert_eq!(hub.me(None).await, not_signed_in);
    assert_eq!(hub.me(Some("wipehub_session=garbage")).await, not_signed_in);
    let forged = format!("wipehub_session={}", "A".repeat(43));
    assert_eq!(hub.me(Some(&forged)).await, not_signed_in);

    let resp = hub.post_empty("/api/auth/sign-out", None).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = hub.post_empty("/api/auth/sign-out", Some(&forged)).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(hub.audited("auth.sign_out").await.is_empty());
}

#[tokio::test]
async fn expired_session_is_not_signed_in() {
    let hub = spawn_hub().await;
    let (_, cookie) = hub.sign_up(&unique_address()).await;
    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 second'")
        .execute(&hub.db.pool)
        .await
        .unwrap();
    assert_eq!(hub.me(Some(&cookie)).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_shows_only_the_signed_in_users_own_organization() {
    let hub = spawn_hub().await;
    let (a, a_cookie) = hub.sign_up(&unique_address()).await;
    let (b, b_cookie) = hub.sign_up(&unique_address()).await;
    assert_ne!(a["organization"]["id"], b["organization"]["id"]);
    let (_, me_a) = hub.me(Some(&a_cookie)).await;
    let (_, me_b) = hub.me(Some(&b_cookie)).await;
    assert_eq!(me_a["user"]["id"], a["user"]["id"]);
    assert_eq!(me_a["organization"], a["organization"]);
    assert_eq!(me_b["organization"], b["organization"]);

    // The data layer itself refuses a user outside the given organization.
    let a_user: uuid::Uuid = a["user"]["id"].as_str().unwrap().parse().unwrap();
    let b_org: uuid::Uuid = b["organization"]["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        wipe_hub::accounts::account(&hub.db.pool, b_org, a_user)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn unknown_auth_paths_stay_json_404() {
    let hub = spawn_hub().await;
    let resp = hub
        .http
        .get(hub.url("/api/auth/nope"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        error_of(resp).await,
        (StatusCode::NOT_FOUND, "not found".into())
    );
}

// ---------------------------------------------------------------------------
// Story 2: CSRF, rotation, throttles, sign-out everywhere, cookie policy

const BAD_CSRF: &str = "missing or invalid CSRF token";
const LOCKED: &str = "too many failed sign-in attempts; try again later";

impl Hub {
    /// A request with exactly the given `Cookie` and `X-CSRF-Token` headers.
    async fn raw(
        &self,
        method: reqwest::Method,
        path: &str,
        cookie: Option<&str>,
        header: Option<&str>,
        body: Option<&Value>,
    ) -> Response {
        let mut req = self.http.request(method, self.url(path));
        if let Some(c) = cookie {
            req = req.header(COOKIE, c);
        }
        if let Some(h) = header {
            req = req.header("x-csrf-token", h);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        req.send().await.unwrap()
    }

    /// `(subject, detail)` of every audit row for `action`, oldest first.
    async fn audit_details(&self, action: &str) -> Vec<(String, Value)> {
        sqlx::query_as("SELECT subject, detail FROM audit_log WHERE action = $1 ORDER BY id")
            .bind(action)
            .fetch_all(&self.db.pool)
            .await
            .unwrap()
    }

    async fn counts(&self) -> Vec<i64> {
        let mut out = Vec::new();
        for t in [
            "audit_log",
            "users",
            "sessions",
            "auth_throttle",
            "auth_tokens",
        ] {
            out.push(self.count(t).await);
        }
        out
    }

    async fn throttle_refused(&self, key: &str) -> Option<i32> {
        sqlx::query_scalar("SELECT refused FROM auth_throttle WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.db.pool)
            .await
            .unwrap()
    }

    async fn throttle_failures(&self, key: &str) -> Option<i32> {
        sqlx::query_scalar("SELECT failures FROM auth_throttle WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.db.pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn csrf_route_mints_once_then_reuses_the_cookie() {
    let hub = spawn_hub().await;
    let (token, set) = fetch_csrf(&hub.http, hub.addr, None).await;
    assert_eq!(token.len(), 43);
    assert_eq!(URL_SAFE_NO_PAD.decode(&token).unwrap().len(), 32);
    assert_eq!(
        set.as_deref(),
        Some(
            format!("wipehub_csrf={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age=1209600")
                .as_str()
        )
    );

    // A well-formed cookie is reused and not set again.
    let cookie = format!("wipehub_csrf={token}");
    let (again, set) = fetch_csrf(&hub.http, hub.addr, Some(&cookie)).await;
    assert_eq!(again, token);
    assert_eq!(set, None);

    // A malformed cookie gets a new token.
    for bad in ["wipehub_csrf=abc", "wipehub_csrf=", "other=1"] {
        let (fresh, set) = fetch_csrf(&hub.http, hub.addr, Some(bad)).await;
        assert_ne!(fresh, token, "{bad}");
        assert!(set.unwrap().starts_with(&format!("wipehub_csrf={fresh};")));
    }
    assert_eq!(hub.count("audit_log").await, 0);
}

#[tokio::test]
async fn mutating_api_requests_without_a_valid_csrf_token_are_403_and_write_nothing() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (_, session) = hub.sign_up(&email).await;
    let before = hub.counts().await;

    let good = hub.csrf.clone();
    let other = fetch_csrf(&hub.http, hub.addr, None).await.0;
    let malformed_43 = format!("{}B", "A".repeat(42));
    // (csrf cookie, header)
    let variants: Vec<(Option<String>, Option<String>)> = vec![
        (None, None),
        (Some(good.clone()), None),
        (None, Some(good.clone())),
        (Some(good.clone()), Some(other.clone())),
        (Some("abc".into()), Some("abc".into())),
        (Some(malformed_43.clone()), Some(malformed_43.clone())),
        (Some(format!("{good}x")), Some(format!("{good}x"))),
    ];
    let routes: Vec<(&str, Option<Value>)> = vec![
        (
            "/api/auth/sign-up",
            Some(json!({
                "email": unique_address(),
                "password": PASSWORD,
                "name": "Eve",
                "organization_name": "Evil Ltd",
            })),
        ),
        (
            "/api/auth/sign-in",
            Some(json!({"email": email, "password": "wrong password!!"})),
        ),
        ("/api/auth/sign-out", None),
        (
            "/api/auth/verify-email",
            Some(json!({"token": "A".repeat(43)})),
        ),
        ("/api/auth/verification-email", None),
        ("/api/auth/sign-out-everywhere", None),
        ("/api/auth/magic-link", Some(json!({"email": email}))),
        (
            "/api/auth/magic-link/sign-in",
            Some(json!({"token": "A".repeat(43)})),
        ),
        ("/api/auth/password-reset", Some(json!({"email": email}))),
        (
            "/api/auth/password-reset/complete",
            Some(json!({"token": "A".repeat(43), "password": PASSWORD})),
        ),
        (
            "/api/auth/password",
            Some(json!({"current_password": PASSWORD, "new_password": "a new passphrase"})),
        ),
        ("/api/account/profile", Some(json!({"name": "Eve"}))),
        ("/api/organization", Some(json!({"name": "Evil Ltd"}))),
        (
            "/api/organization/invitations",
            Some(json!({"email": unique_address(), "role": "member"})),
        ),
        (
            "/api/organization/invitations/00000000-0000-0000-0000-000000000000/revoke",
            None,
        ),
        (
            "/api/organization/members/00000000-0000-0000-0000-000000000000/role",
            Some(json!({"role": "owner"})),
        ),
        (
            "/api/organization/members/00000000-0000-0000-0000-000000000000/remove",
            None,
        ),
        (
            "/api/invitations/lookup",
            Some(json!({"token": "A".repeat(43)})),
        ),
        (
            "/api/invitations/accept",
            Some(json!({"token": "A".repeat(43), "name": "Eve", "password": PASSWORD})),
        ),
        ("/api/nope", None),
        ("/api", None),
        ("/api/health", None),
    ];
    for (path, body) in &routes {
        for (c, h) in &variants {
            let cookie = match c {
                Some(t) => format!("{session}; wipehub_csrf={t}"),
                None => session.clone(),
            };
            let resp = hub
                .raw(
                    reqwest::Method::POST,
                    path,
                    Some(&cookie),
                    h.as_deref(),
                    body.as_ref(),
                )
                .await;
            assert_eq!(
                error_of(resp).await,
                (StatusCode::FORBIDDEN, BAD_CSRF.into()),
                "{path} cookie={c:?} header={h:?}"
            );
        }
    }
    // Exactly one header value: the valid token sent twice, or valid plus
    // invalid, is refused.
    for second in [good.clone(), other.clone()] {
        let resp = hub
            .http
            .post(hub.url("/api/auth/sign-out"))
            .header(COOKIE, format!("{session}; wipehub_csrf={good}"))
            .header("x-csrf-token", &good)
            .header("x-csrf-token", &second)
            .send()
            .await
            .unwrap();
        assert_eq!(
            error_of(resp).await,
            (StatusCode::FORBIDDEN, BAD_CSRF.into()),
            "second header {second}"
        );
    }
    // Other mutating methods are covered too.
    for method in [
        reqwest::Method::PUT,
        reqwest::Method::PATCH,
        reqwest::Method::DELETE,
    ] {
        let resp = hub
            .raw(method.clone(), "/api/auth/me", Some(&session), None, None)
            .await;
        assert_eq!(
            error_of(resp).await,
            (StatusCode::FORBIDDEN, BAD_CSRF.into()),
            "{method}"
        );
    }
    assert_eq!(hub.counts().await, before);
    // The session survived every attempt, and safe methods need no token.
    assert_eq!(hub.me(Some(&session)).await.0, StatusCode::OK);

    // The same request with a valid token proceeds.
    let resp = hub
        .raw(
            reqwest::Method::POST,
            "/api/nope",
            Some(&format!("wipehub_csrf={good}")),
            Some(&good),
            None,
        )
        .await;
    assert_eq!(
        error_of(resp).await,
        (StatusCode::NOT_FOUND, "not found".into())
    );
}

#[tokio::test]
async fn sign_in_rotates_the_incoming_session() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (_, a) = hub.sign_up(&email).await;

    let resp = hub
        .post(
            "/api/auth/sign-in",
            json!({"email": email, "password": PASSWORD}),
            Some(&a),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let b = cookie_pair(&resp);
    assert_ne!(a, b);
    assert_eq!(hub.me(Some(&a)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(hub.me(Some(&b)).await.0, StatusCode::OK);
    assert_eq!(hub.count("sessions").await, 1);

    // No cookie, a garbage cookie and an already-deleted cookie: not rotated.
    hub.sign_in(&email, PASSWORD).await;
    for cookie in ["wipehub_session=garbage", a.as_str()] {
        let resp = hub
            .post(
                "/api/auth/sign-in",
                json!({"email": email, "password": PASSWORD}),
                Some(cookie),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let rotated: Vec<Value> = hub
        .audit_details("auth.sign_in")
        .await
        .into_iter()
        .map(|(_, d)| d)
        .collect();
    assert_eq!(
        rotated,
        vec![
            json!({"rotated": true, "peer": "ip:127.0.0.1"}),
            json!({"rotated": false, "peer": "ip:127.0.0.1"}),
            json!({"rotated": false, "peer": "ip:127.0.0.1"}),
            json!({"rotated": false, "peer": "ip:127.0.0.1"}),
        ]
    );
    assert_eq!(hub.me(Some(&b)).await.0, StatusCode::OK);
}

#[tokio::test]
async fn sign_up_deletes_the_incoming_session() {
    let hub = spawn_hub().await;
    let (_, a) = hub.sign_up(&unique_address()).await;
    let resp = hub
        .post(
            "/api/auth/sign-up",
            json!({
                "email": unique_address(),
                "password": PASSWORD,
                "name": "B",
                "organization_name": "B Org",
            }),
            Some(&a),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let b = cookie_pair(&resp);
    assert_eq!(hub.me(Some(&a)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(hub.me(Some(&b)).await.0, StatusCode::OK);
    assert_eq!(hub.count("sessions").await, 1);
}

fn retry_after(resp: &Response) -> u64 {
    resp.headers()
        .get(RETRY_AFTER)
        .expect("Retry-After")
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn assert_locked(resp: Response) {
    let retry = retry_after(&resp);
    assert!((1..=900).contains(&retry), "{retry}");
    assert!(resp.headers().get(SET_COOKIE).is_none());
    assert_eq!(
        error_of(resp).await,
        (StatusCode::TOO_MANY_REQUESTS, LOCKED.into())
    );
}

#[tokio::test]
async fn five_wrong_passwords_lock_the_email_until_the_lock_elapses() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    let key = format!("email:{}", email.to_lowercase());

    for i in 1..=5 {
        // Every spelling that reaches the account shares one key.
        let spelling = if i % 2 == 0 {
            format!("  {}  ", email.to_uppercase())
        } else {
            email.clone()
        };
        let resp = hub.sign_in(&spelling, "wrong password!!").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    assert_eq!(hub.throttle_failures(&key).await, Some(5));
    // Locked: even the right password is refused, and the lock is not extended.
    let until = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT locked_until::text FROM auth_throttle WHERE key = $1",
        )
        .bind(&key)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
    };
    let first_until = until().await;
    let resp = hub.sign_in(&email, PASSWORD).await;
    let retry = retry_after(&resp);
    assert!((890..=900).contains(&retry), "{retry}");
    assert_locked(resp).await;
    assert_locked(hub.sign_in(&email, "wrong password!!").await).await;
    assert_eq!(until().await, first_until);
    assert_eq!(hub.throttle_failures(&key).await, Some(5));
    assert_eq!(hub.throttle_refused(&key).await, Some(2));

    let locked = hub.audit_details("auth.sign_in_locked").await;
    assert_eq!(locked.len(), 1, "{locked:?}");
    assert_eq!(locked[0].0, key);
    assert!(locked[0].1["until"].as_str().unwrap().ends_with('Z'));
    assert_eq!(locked[0].1["peer"], "ip:127.0.0.1");
    assert_eq!(
        hub.audited("auth.sign_in_locked").await[0].0,
        "anonymous".to_string()
    );
    // Refused attempts are not audited one by one.
    let failed = hub.audit_details("auth.sign_in_failed").await;
    assert_eq!(failed.len(), 5);
    for (_, d) in &failed {
        assert_eq!(
            d,
            &json!({"reason": "wrong_password", "peer": "ip:127.0.0.1"})
        );
    }
    assert!(hub.audited("auth.sign_in").await.is_empty());
    assert_eq!(hub.count("sessions").await, 1);

    // Lock elapsed: the right password signs in and the email key is gone.
    sqlx::query(
        "UPDATE auth_throttle SET locked_until = now() - interval '1 second' WHERE key = $1",
    )
    .bind(&key)
    .execute(&hub.db.pool)
    .await
    .unwrap();
    let resp = hub.sign_in(&email, PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(hub.throttle_failures(&key).await, None);
    // The IP key is not cleared by a success, only refunded its own charge.
    assert_eq!(hub.throttle_failures("ip:127.0.0.1").await, Some(5));
    // The restart that ended the lock summarised its two refusals.
    let summary = hub.audit_details("auth.sign_in_lock_summary").await;
    assert_eq!(summary.len(), 1, "{summary:?}");
    assert_eq!(summary[0].0, key);
    assert_eq!(summary[0].1["refused"], 2);
    // The test moved the lock into the past; the summary reports it as found.
    assert!(summary[0].1["until"].as_str().unwrap().ends_with('Z'));
    assert_eq!(summary[0].1["peer"], "ip:127.0.0.1");
}

#[tokio::test]
async fn unknown_emails_lock_like_known_ones() {
    let hub = spawn_hub().await;
    let email = unique_address();
    for _ in 0..5 {
        let resp = hub.sign_in(&email, PASSWORD).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
    assert_locked(hub.sign_in(&email, PASSWORD).await).await;
    assert_eq!(
        hub.audit_details("auth.sign_in_locked").await[0].0,
        format!("email:{email}")
    );
}

#[tokio::test]
async fn a_success_resets_the_email_count() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    for _ in 0..4 {
        let resp = hub.sign_in(&email, "wrong password!!").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(hub.sign_in(&email, PASSWORD).await.status(), StatusCode::OK);
    for _ in 0..4 {
        let resp = hub.sign_in(&email, "wrong password!!").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        hub.throttle_failures(&format!("email:{}", email.to_lowercase()))
            .await,
        Some(4)
    );
    assert!(hub.audited("auth.sign_in_locked").await.is_empty());
}

#[tokio::test]
async fn an_elapsed_window_restarts_the_count() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let key = format!("email:{email}");
    for _ in 0..4 {
        hub.sign_in(&email, PASSWORD).await;
    }
    sqlx::query("UPDATE auth_throttle SET window_started_at = now() - interval '16 minutes'")
        .execute(&hub.db.pool)
        .await
        .unwrap();
    let resp = hub.sign_in(&email, PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(hub.throttle_failures(&key).await, Some(1));
    assert!(hub.audited("auth.sign_in_locked").await.is_empty());
}

#[tokio::test]
async fn twenty_failures_from_one_ip_lock_it_for_every_email() {
    let hub = spawn_hub().await;
    for i in 1..=20 {
        let resp = hub.sign_in(&unique_address(), PASSWORD).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let fresh = unique_address();
    assert_locked(hub.sign_in(&fresh, PASSWORD).await).await;
    let locked = hub.audit_details("auth.sign_in_locked").await;
    assert_eq!(locked.len(), 1, "{locked:?}");
    assert_eq!(locked[0].0, "ip:127.0.0.1");
    // The IP key refused first, so the email key was never touched, and
    // the attempt counted as a refusal on the IP key only.
    assert_eq!(hub.throttle_failures(&format!("email:{fresh}")).await, None);
    assert_eq!(hub.throttle_failures("ip:127.0.0.1").await, Some(20));
    assert_eq!(hub.throttle_refused("ip:127.0.0.1").await, Some(1));
}

#[tokio::test]
async fn sign_out_everywhere_ends_every_session_of_that_user_only() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (body, first) = hub.sign_up(&email).await;
    let second = cookie_pair(&hub.sign_in(&email, PASSWORD).await);
    let third = cookie_pair(&hub.sign_in(&email, PASSWORD).await);
    let (_, other) = hub.sign_up(&unique_address()).await;

    let resp = hub
        .post_empty("/api/auth/sign-out-everywhere", Some(&second))
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let cleared = set_cookie(&resp);
    assert!(cleared.starts_with("wipehub_session=;"), "{cleared}");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    for c in [&first, &second, &third] {
        assert_eq!(hub.me(Some(c)).await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(hub.me(Some(&other)).await.0, StatusCode::OK);
    let who = format!("user:{}", body["user"]["id"].as_str().unwrap());
    assert_eq!(
        hub.audited("auth.sign_out_everywhere").await,
        vec![(who.clone(), who.clone())]
    );
    assert_eq!(
        hub.audit_details("auth.sign_out_everywhere").await[0].1,
        json!({"sessions": 3})
    );

    // Not signed in (or no longer): 401, nothing audited.
    for cookie in [None, Some(second.as_str())] {
        let resp = hub
            .post_empty("/api/auth/sign-out-everywhere", cookie)
            .await;
        assert_eq!(
            error_of(resp).await,
            (StatusCode::UNAUTHORIZED, "not signed in".into())
        );
    }
    assert_eq!(hub.audited("auth.sign_out_everywhere").await.len(), 1);
}

#[tokio::test]
async fn secure_cookie_policy_follows_the_public_base_url_host() {
    for (base, secure) in [
        ("http://127.0.0.1", false),
        ("http://localhost", false),
        ("http://hub.example.com", true),
        ("https://localhost", true),
    ] {
        let hub = spawn_hub_with(DEFAULT_SMTP_PORT, base).await;
        let (_, csrf_set) = fetch_csrf(&hub.http, hub.addr, None).await;
        let resp = hub
            .post(
                "/api/auth/sign-up",
                json!({
                    "email": unique_address(),
                    "password": PASSWORD,
                    "name": "A",
                    "organization_name": "O",
                }),
                None,
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED, "{base}");
        let session_set = set_cookie(&resp);
        for c in [csrf_set.unwrap(), session_set] {
            let has = c.split(';').map(str::trim).any(|a| a == "Secure");
            assert_eq!(has, secure, "{base}: {c}");
        }
    }
}

#[tokio::test]
async fn concurrent_guesses_get_exactly_five_password_checks() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let req = hub
            .post_req("/api/auth/sign-in", None)
            .json(&json!({"email": email, "password": "wrong password!!"}));
        set.spawn(async move { req.send().await.unwrap() });
    }
    let (mut unauthorized, mut refused) = (0, 0);
    while let Some(resp) = set.join_next().await {
        let resp = resp.unwrap();
        match resp.status() {
            StatusCode::UNAUTHORIZED => unauthorized += 1,
            StatusCode::TOO_MANY_REQUESTS => {
                assert_locked(resp).await;
                refused += 1;
            }
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!((unauthorized, refused), (5, 15));
    assert_eq!(hub.audited("auth.sign_in_locked").await.len(), 1);
    assert_eq!(hub.audited("auth.sign_in_failed").await.len(), 5);
    let key = format!("email:{}", email.to_lowercase());
    assert_eq!(hub.throttle_failures(&key).await, Some(5));
    assert_eq!(hub.throttle_refused(&key).await, Some(15));
    // The 15 refused attempts had their IP charge refunded: only the five
    // checked attempts count.
    assert_eq!(hub.throttle_failures("ip:127.0.0.1").await, Some(5));
}

#[tokio::test]
async fn right_password_on_the_fifth_try_signs_in_without_a_lock() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    for _ in 0..4 {
        let resp = hub.sign_in(&email, "wrong password!!").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(hub.sign_in(&email, PASSWORD).await.status(), StatusCode::OK);
    let key = format!("email:{}", email.to_lowercase());
    assert_eq!(hub.throttle_failures(&key).await, None);
    assert!(hub.audited("auth.sign_in_locked").await.is_empty());
    assert_eq!(hub.throttle_failures("ip:127.0.0.1").await, Some(4));
}

#[tokio::test]
async fn hammering_a_locked_email_is_counted_not_audited_then_summarised_once() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    let key = format!("email:{}", email.to_lowercase());
    for _ in 0..5 {
        hub.sign_in(&email, "wrong password!!").await;
    }
    let audit_rows = hub.count("audit_log").await;
    for _ in 0..100 {
        assert_locked(hub.sign_in(&email, "wrong password!!").await).await;
    }
    assert_eq!(hub.count("audit_log").await, audit_rows);
    assert_eq!(hub.throttle_refused(&key).await, Some(100));
    assert_eq!(hub.throttle_failures(&key).await, Some(5));

    sqlx::query("UPDATE auth_throttle SET locked_until = now() - interval '1 second'")
        .execute(&hub.db.pool)
        .await
        .unwrap();
    assert_eq!(hub.sign_in(&email, PASSWORD).await.status(), StatusCode::OK);
    let summary = hub.audit_details("auth.sign_in_lock_summary").await;
    assert_eq!(summary.len(), 1, "{summary:?}");
    assert_eq!(summary[0].0, key);
    assert_eq!(summary[0].1["refused"], 100);
    assert!(summary[0].1["until"].as_str().unwrap().ends_with('Z'));
    assert_eq!(
        hub.audited("auth.sign_in_lock_summary").await[0].0,
        "anonymous"
    );
    // The lock cost exactly two audit rows: locked and its summary.
    assert_eq!(hub.audited("auth.sign_in_locked").await.len(), 1);
}

#[tokio::test]
async fn ipv6_peers_are_throttled_by_their_slash_64() {
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use tower::ServiceExt;

    let db = TestDb::migrated().await;
    let app = wipe_hub::router(hub_state(
        &db,
        DEFAULT_SMTP_PORT,
        "http://127.0.0.1:7900",
        DEFAULT_MAIL_FROM,
    ));
    let csrf = "A".repeat(43);
    let attempt = |peer: &str| {
        let peer: SocketAddr = peer.parse().unwrap();
        let body = json!({"email": unique_address(), "password": PASSWORD}).to_string();
        let mut req = axum::http::Request::post("/api/auth/sign-in")
            .header("content-type", "application/json")
            .header("cookie", format!("wipehub_csrf={csrf}"))
            .header("x-csrf-token", &csrf)
            .body(Body::from(body))
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(peer));
        app.clone().oneshot(req)
    };
    for i in 1..=20 {
        let resp = attempt(&format!("[2001:db8:1:2::{i:x}]:5000"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let resp = attempt("[2001:db8:1:2:ffff:1:2:3]:5000").await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(resp.headers().get(RETRY_AFTER).is_some());
    // Another /64 is unaffected.
    let resp = attempt("[2001:db8:1:3::1]:5000").await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let failures: i32 =
        sqlx::query_scalar("SELECT failures FROM auth_throttle WHERE key = 'ip:2001:db8:1:2::/64'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(failures, 20);
    let peers: Vec<Value> = sqlx::query_scalar(
        "SELECT detail->'peer' FROM audit_log WHERE action = 'auth.sign_in_failed' ORDER BY id",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(peers.len(), 21);
    assert!(peers[..20].iter().all(|p| p == "ip:2001:db8:1:2::/64"));
    assert_eq!(peers[20], "ip:2001:db8:1:3::/64");
}

#[tokio::test]
async fn sign_in_purges_that_users_expired_sessions() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (body, _) = hub.sign_up(&email).await;
    let (_, other) = hub.sign_up(&unique_address()).await;
    // Both users' existing sessions expire.
    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 second'")
        .execute(&hub.db.pool)
        .await
        .unwrap();
    assert_eq!(hub.sign_in(&email, PASSWORD).await.status(), StatusCode::OK);
    let user_id = uuid::Uuid::parse_str(body["user"]["id"].as_str().unwrap()).unwrap();
    let mine: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    assert_eq!(mine, 1, "only the new session remains");
    // The other user's expired session is not this sign-in's business.
    assert_eq!(hub.count("sessions").await, 2);
    assert_eq!(hub.me(Some(&other)).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn stale_throttle_rows_are_purged_on_the_next_failure() {
    let hub = spawn_hub().await;
    sqlx::query(
        "INSERT INTO auth_throttle (key, failures, refused, window_started_at, locked_until) VALUES \
         ('email:stale@example.com', 3, 0, now() - interval '2 days', NULL), \
         ('email:stale-refused@example.com', 5, 7, now() - interval '2 days', \
          now() - interval '1 day'), \
         ('email:still-locked@example.com', 5, 0, now() - interval '2 days', \
          now() + interval '1 hour'), \
         ('email:recent@example.com', 2, 0, now() - interval '1 hour', NULL)",
    )
    .execute(&hub.db.pool)
    .await
    .unwrap();
    let resp = hub.sign_in(&unique_address(), PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    for (key, kept) in [
        ("email:stale@example.com", false),
        ("email:stale-refused@example.com", false),
        ("email:still-locked@example.com", true),
        ("email:recent@example.com", true),
    ] {
        assert_eq!(hub.throttle_failures(key).await.is_some(), kept, "{key}");
    }
    let summary = hub.audit_details("auth.sign_in_lock_summary").await;
    assert_eq!(summary.len(), 1, "{summary:?}");
    assert_eq!(summary[0].0, "email:stale-refused@example.com");
    assert_eq!(summary[0].1["refused"], 7);
    // A purged row belongs to another key: no peer of this request.
    assert!(summary[0].1.get("peer").is_none(), "{:?}", summary[0].1);
}

#[tokio::test]
async fn a_success_summarises_refusals_on_the_email_key_it_clears() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    let key = format!("email:{}", email.to_lowercase());
    sqlx::query(
        "INSERT INTO auth_throttle (key, failures, refused, window_started_at, locked_until) \
         VALUES ($1, 4, 3, now(), NULL)",
    )
    .bind(&key)
    .execute(&hub.db.pool)
    .await
    .unwrap();
    assert_eq!(hub.sign_in(&email, PASSWORD).await.status(), StatusCode::OK);
    assert_eq!(hub.throttle_failures(&key).await, None);
    let summary = hub.audit_details("auth.sign_in_lock_summary").await;
    assert_eq!(summary.len(), 1, "{summary:?}");
    assert_eq!(summary[0].0, key);
    assert_eq!(
        summary[0].1,
        json!({"until": null, "refused": 3, "peer": "ip:127.0.0.1"})
    );
}

#[tokio::test]
async fn a_full_but_unlocked_key_answers_retry_after_one_second() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    let key = format!("email:{}", email.to_lowercase());
    // Five checks in flight: full, not (yet) locked.
    sqlx::query(
        "INSERT INTO auth_throttle (key, failures, refused, window_started_at, locked_until) \
         VALUES ($1, 5, 0, now(), NULL)",
    )
    .bind(&key)
    .execute(&hub.db.pool)
    .await
    .unwrap();
    let resp = hub.sign_in(&email, PASSWORD).await;
    assert_eq!(retry_after(&resp), 1);
    assert_locked(resp).await;
    assert_eq!(hub.throttle_refused(&key).await, Some(1));
    // The IP charge was refunded.
    assert_eq!(hub.throttle_failures("ip:127.0.0.1").await, Some(0));
}

// ---------------------------------------------------------------------------
// Story 3: magic links, password reset and the email-send throttle

const BASE: &str = "http://127.0.0.1:7900";
const MAGIC: &str = "/api/auth/magic-link";
const MAGIC_SIGN_IN: &str = "/api/auth/magic-link/sign-in";
const RESET: &str = "/api/auth/password-reset";
const RESET_COMPLETE: &str = "/api/auth/password-reset/complete";
const MAIL_LOCKED: &str = "too many emails requested; try again later";
const BAD_MAGIC: &str = "this sign-in link is invalid or has expired";
const BAD_RESET: &str = "this password reset link is invalid or has expired";
const NEW_PASSWORD: &str = "a brand new passphrase";

impl Hub {
    /// `POST` `{email}` to one of the two request routes.
    async fn request(&self, path: &str, email: &str) -> Response {
        self.post(path, json!({ "email": email }), None).await
    }

    async fn magic_sign_in(&self, token: &str, cookie: Option<&str>) -> Response {
        self.post(MAGIC_SIGN_IN, json!({ "token": token }), cookie)
            .await
    }

    async fn complete_reset(&self, token: &str, password: &str) -> Response {
        self.post(
            RESET_COMPLETE,
            json!({ "token": token, "password": password }),
            None,
        )
        .await
    }

    async fn password_hash(&self, user_id: &str) -> String {
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1::uuid")
            .bind(user_id)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn throttle_until(&self, key: &str) -> Option<String> {
        sqlx::query_scalar("SELECT locked_until::text FROM auth_throttle WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.db.pool)
            .await
            .unwrap()
            .flatten()
    }

    /// Wait until `table` has `n` rows: the request routes write in a
    /// background task after answering.
    async fn wait_for_count(&self, table: &str, n: i64) {
        let deadline = std::time::Instant::now() + MAIL_WAIT;
        loop {
            let got = self.count(table).await;
            if got == n {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{table} has {got} rows, want {n}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Move every throttle lock into the past.
    async fn expire_locks(&self) {
        sqlx::query("UPDATE auth_throttle SET locked_until = now() - interval '1 second'")
            .execute(&self.db.pool)
            .await
            .unwrap();
    }
}

/// The messages to `addr` with `subject`, newest first.
async fn messages_with_subject(addr: &str, subject: &str) -> Vec<support::MessageSummary> {
    Mailpit::new()
        .messages_to(addr)
        .await
        .into_iter()
        .filter(|m| m.subject == subject)
        .collect()
}

/// Wait until at least `n` messages to `addr` have `subject`, then return the
/// tokens of their `page` links, oldest first.
async fn mailed_tokens(addr: &str, subject: &str, page: &str, n: usize) -> Vec<String> {
    let mailpit = Mailpit::new();
    let deadline = std::time::Instant::now() + MAIL_WAIT;
    loop {
        let msgs = messages_with_subject(addr, subject).await;
        if msgs.len() >= n {
            let mut out = Vec::new();
            for m in msgs.iter().rev() {
                out.push(token_in(&mailpit.message(&m.id).await.text, BASE, page));
            }
            return out;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "only {} of {n} {subject:?} messages to {addr}",
            msgs.len()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn magic_token(addr: &str) -> String {
    mailed_tokens(addr, wipe_hub::mail::MAGIC_LINK_SUBJECT, "magic-link", 1)
        .await
        .pop()
        .unwrap()
}

async fn reset_token(addr: &str) -> String {
    mailed_tokens(
        addr,
        wipe_hub::mail::PASSWORD_RESET_SUBJECT,
        "reset-password",
        1,
    )
    .await
    .pop()
    .unwrap()
}

/// Assert `resp` is exactly the 202 `{}` both request routes answer.
async fn assert_accepted(resp: Response) {
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert!(resp.headers().get(SET_COOKIE).is_none());
    assert_eq!(resp.text().await.unwrap(), "{}");
}

async fn assert_mail_locked(resp: Response) -> u64 {
    let retry = retry_after(&resp);
    assert!((1..=3600).contains(&retry), "{retry}");
    assert!(resp.headers().get(SET_COOKIE).is_none());
    assert_eq!(
        error_of(resp).await,
        (StatusCode::TOO_MANY_REQUESTS, MAIL_LOCKED.into())
    );
    retry
}

fn user_of(body: &Value) -> String {
    format!("user:{}", body["user"]["id"].as_str().unwrap())
}

#[tokio::test]
async fn magic_link_signs_in_once() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (body, _) = hub.sign_up(&email).await;
    let who = user_of(&body);

    // Any spelling of the address reaches the account.
    assert_accepted(
        hub.request(MAGIC, &format!(" {} ", email.to_uppercase()))
            .await,
    )
    .await;
    let token = magic_token(&email).await;
    assert_eq!(token.len(), 43);

    let resp = hub.magic_sign_in(&token, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = cookie_pair(&resp);
    let account: Value = resp.json().await.unwrap();
    assert_eq!(account["user"]["id"], body["user"]["id"]);
    assert_eq!(account["organization"], body["organization"]);
    let (code, me) = hub.me(Some(&cookie)).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(me["user"]["id"], body["user"]["id"]);

    assert_eq!(
        hub.audited("auth.magic_link_requested").await,
        vec![("anonymous".to_string(), who.clone())]
    );
    assert_eq!(
        hub.audit_details("auth.magic_link_requested").await[0].1,
        json!({"peer": "ip:127.0.0.1"})
    );
    assert_eq!(
        hub.audited("auth.magic_link_sign_in").await,
        vec![(who.clone(), who)]
    );

    // Reuse is rejected and signs nobody in.
    let sessions = hub.count("sessions").await;
    let resp = hub.magic_sign_in(&token, None).await;
    assert!(resp.headers().get(SET_COOKIE).is_none());
    assert_eq!(
        error_of(resp).await,
        (StatusCode::BAD_REQUEST, BAD_MAGIC.into())
    );
    assert_eq!(hub.count("sessions").await, sessions);
    let garbage_43 = "A".repeat(43);
    for garbage in ["garbage", "", garbage_43.as_str()] {
        let resp = hub.magic_sign_in(garbage, None).await;
        assert_eq!(
            error_of(resp).await,
            (StatusCode::BAD_REQUEST, BAD_MAGIC.into())
        );
    }
    assert_eq!(hub.audited("auth.magic_link_sign_in").await.len(), 1);

    // 15 minutes, and the raw token is in no audit row.
    let minutes: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM expires_at - created_at)::float8 / 60 FROM auth_tokens \
         WHERE purpose = 'magic_link'",
    )
    .fetch_one(&hub.db.pool)
    .await
    .unwrap();
    assert!((minutes - 15.0).abs() < 1e-6, "{minutes}");
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE strpos(detail::text || subject, $1) > 0",
    )
    .bind(&token)
    .fetch_one(&hub.db.pool)
    .await
    .unwrap();
    assert_eq!(leaked, 0);
}

/// Headers other than `Date`, as sorted `(name, value)` pairs.
fn comparable_headers(resp: &Response) -> Vec<(String, String)> {
    let mut h: Vec<(String, String)> = resp
        .headers()
        .iter()
        .filter(|(n, _)| *n != reqwest::header::DATE)
        .map(|(n, v)| (n.to_string(), v.to_str().unwrap().to_string()))
        .collect();
    h.sort();
    h
}

#[tokio::test]
async fn request_routes_answer_known_and_unknown_emails_identically() {
    let hub = spawn_hub().await;
    let known = unique_address();
    let (body, _) = hub.sign_up(&known).await;
    let before = hub.counts().await;

    for path in [MAGIC, RESET] {
        let unknown = unique_address();
        let u = hub.request(path, &unknown).await;
        let k = hub.request(path, &known).await;
        let (u_parts, k_parts) = (
            (u.status(), comparable_headers(&u)),
            (k.status(), comparable_headers(&k)),
        );
        assert_eq!(u_parts, k_parts, "{path}");
        assert_eq!(u_parts.0, StatusCode::ACCEPTED);
        assert_eq!(u.bytes().await.unwrap(), k.bytes().await.unwrap(), "{path}");

        // The known address's mail arrives; the unknown one gets none.
        let (subject, page) = if path == MAGIC {
            (wipe_hub::mail::MAGIC_LINK_SUBJECT, "magic-link")
        } else {
            (wipe_hub::mail::PASSWORD_RESET_SUBJECT, "reset-password")
        };
        mailed_tokens(&known, subject, page, 1).await;
        assert!(Mailpit::new().messages_to(&unknown).await.is_empty());
    }
    // Only the known address got tokens and audit rows.
    let after = hub.counts().await;
    assert_eq!(after[0], before[0] + 2, "audit_log");
    assert_eq!(after[4], before[4] + 2, "auth_tokens");
    let who = user_of(&body);
    assert_eq!(
        hub.audited("auth.password_reset_requested").await,
        vec![("anonymous".to_string(), who)]
    );

    // A syntactically invalid email is 400 before any throttle charge.
    let throttle_rows = hub.count("auth_throttle").await;
    for path in [MAGIC, RESET] {
        let resp = hub.request(path, "not-an-email").await;
        assert_eq!(
            error_of(resp).await,
            (
                StatusCode::BAD_REQUEST,
                "email is not a valid email address".into()
            )
        );
    }
    assert_eq!(hub.count("auth_throttle").await, throttle_rows);
}

#[tokio::test]
async fn request_routes_answer_202_when_smtp_is_down() {
    let hub = spawn_hub_with(closed_port().await, BASE).await;
    let email = unique_address();
    hub.sign_up(&email).await;
    for path in [MAGIC, RESET] {
        assert_accepted(hub.request(path, &email).await).await;
    }
    // The tokens were still issued, in the background; only the send failed.
    hub.wait_for_count("auth_tokens", 3).await;
    hub.wait_for_count("audit_log", 3).await;
}

#[tokio::test]
async fn an_expired_magic_link_is_rejected() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    assert_accepted(hub.request(MAGIC, &email).await).await;
    let token = magic_token(&email).await;
    sqlx::query("UPDATE auth_tokens SET expires_at = now() - interval '1 second'")
        .execute(&hub.db.pool)
        .await
        .unwrap();
    let sessions = hub.count("sessions").await;
    let resp = hub.magic_sign_in(&token, None).await;
    assert_eq!(
        error_of(resp).await,
        (StatusCode::BAD_REQUEST, BAD_MAGIC.into())
    );
    assert_eq!(hub.count("sessions").await, sessions);
}

#[tokio::test]
async fn a_magic_link_used_concurrently_signs_in_exactly_once() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    assert_accepted(hub.request(MAGIC, &email).await).await;
    let token = magic_token(&email).await;
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let req = hub
            .post_req(MAGIC_SIGN_IN, None)
            .json(&json!({ "token": token }));
        set.spawn(async move { req.send().await.unwrap().status() });
    }
    let mut ok = 0;
    while let Some(status) = set.join_next().await {
        match status.unwrap() {
            StatusCode::OK => ok += 1,
            StatusCode::BAD_REQUEST => {}
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(ok, 1);
    assert_eq!(hub.audited("auth.magic_link_sign_in").await.len(), 1);
}

#[tokio::test]
async fn using_one_magic_link_retires_the_others() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    assert_accepted(hub.request(MAGIC, &email).await).await;
    assert_accepted(hub.request(MAGIC, &email).await).await;
    let tokens = mailed_tokens(&email, wipe_hub::mail::MAGIC_LINK_SUBJECT, "magic-link", 2).await;
    assert_ne!(tokens[0], tokens[1]);
    assert_eq!(
        hub.magic_sign_in(&tokens[0], None).await.status(),
        StatusCode::OK
    );
    let resp = hub.magic_sign_in(&tokens[1], None).await;
    assert_eq!(
        error_of(resp).await,
        (StatusCode::BAD_REQUEST, BAD_MAGIC.into())
    );
}

#[tokio::test]
async fn a_magic_link_gets_a_locked_out_user_back_in() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    // Verified, so the magic link keeps the password (decision 8).
    let verification = verification_token(&email).await;
    let resp = hub
        .post(
            "/api/auth/verify-email",
            json!({"token": verification}),
            None,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let key = format!("email:{}", email.to_lowercase());
    for _ in 0..5 {
        hub.sign_in(&email, "wrong password!!").await;
    }
    assert_locked(hub.sign_in(&email, PASSWORD).await).await;
    assert_eq!(hub.throttle_refused(&key).await, Some(1));

    assert_accepted(hub.request(MAGIC, &email).await).await;
    let token = magic_token(&email).await;
    let resp = hub.magic_sign_in(&token, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(hub.throttle_failures(&key).await, None);
    let summary = hub.audit_details("auth.sign_in_lock_summary").await;
    assert_eq!(summary.len(), 1, "{summary:?}");
    assert_eq!(summary[0].0, key);
    assert_eq!(summary[0].1["refused"], 1);
    assert_eq!(summary[0].1["peer"], "ip:127.0.0.1");
    // The password works again at once.
    assert_eq!(hub.sign_in(&email, PASSWORD).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_magic_link_verifies_the_email() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    let verification = verification_token(&email).await;

    assert_accepted(hub.request(MAGIC, &email).await).await;
    let resp = hub.magic_sign_in(&magic_token(&email).await, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let account: Value = resp.json().await.unwrap();
    assert_eq!(account["user"]["email_verified"], json!(true));
    // The earlier verification link was retired.
    let resp = hub
        .post(
            "/api/auth/verify-email",
            json!({"token": verification}),
            None,
        )
        .await;
    assert_eq!(error_of(resp).await.0, StatusCode::BAD_REQUEST);
    assert!(hub.audited("auth.email_verified").await.is_empty());

    // A later magic sign-in did not verify anything.
    assert_accepted(hub.request(MAGIC, &email).await).await;
    let second = mailed_tokens(&email, wipe_hub::mail::MAGIC_LINK_SUBJECT, "magic-link", 2)
        .await
        .pop()
        .unwrap();
    assert_eq!(
        hub.magic_sign_in(&second, None).await.status(),
        StatusCode::OK
    );
    let details: Vec<Value> = hub
        .audit_details("auth.magic_link_sign_in")
        .await
        .into_iter()
        .map(|(_, d)| d)
        .collect();
    assert_eq!(
        details,
        vec![
            json!({"rotated": false, "email_verified": true, "sessions_ended": 1,
                   "password_cleared": true, "peer": "ip:127.0.0.1"}),
            json!({"rotated": false, "email_verified": false, "sessions_ended": 0,
                   "password_cleared": false, "peer": "ip:127.0.0.1"}),
        ]
    );
}

#[tokio::test]
async fn a_magic_sign_in_rotates_the_incoming_session() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (_, a) = hub.sign_up(&email).await;
    assert_accepted(hub.request(MAGIC, &email).await).await;
    let resp = hub
        .magic_sign_in(&magic_token(&email).await, Some(&a))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let b = cookie_pair(&resp);
    assert_eq!(hub.me(Some(&a)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(hub.me(Some(&b)).await.0, StatusCode::OK);
    assert_eq!(
        hub.audit_details("auth.magic_link_sign_in").await[0].1["rotated"],
        json!(true)
    );
}

#[tokio::test]
async fn a_password_reset_ends_every_session_and_changes_the_password() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (body, first) = hub.sign_up(&email).await;
    let second = cookie_pair(&hub.sign_in(&email, PASSWORD).await);
    let (_, other) = hub.sign_up(&unique_address()).await;

    assert_accepted(hub.request(RESET, &email).await).await;
    let token = reset_token(&email).await;
    let resp = hub.complete_reset(&token, NEW_PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cleared = set_cookie(&resp);
    assert!(cleared.starts_with("wipehub_session=;"), "{cleared}");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    assert_eq!(
        resp.json::<Value>().await.unwrap(),
        json!({"password_reset": true})
    );

    for c in [&first, &second] {
        assert_eq!(hub.me(Some(c)).await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(hub.me(Some(&other)).await.0, StatusCode::OK);
    assert_eq!(
        hub.sign_in(&email, PASSWORD).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let resp = hub.sign_in(&email, NEW_PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let account: Value = resp.json().await.unwrap();
    assert_eq!(account["user"]["email_verified"], json!(true));

    let who = user_of(&body);
    assert_eq!(
        hub.audited("auth.password_reset_requested").await,
        vec![("anonymous".to_string(), who.clone())]
    );
    assert_eq!(
        hub.audited("auth.password_reset").await,
        vec![(who.clone(), who)]
    );
    assert_eq!(
        hub.audit_details("auth.password_reset").await[0].1,
        json!({"sessions": 2, "email_verified": true, "peer": "ip:127.0.0.1"})
    );
    let hours: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM expires_at - created_at)::float8 / 3600 FROM auth_tokens \
         WHERE purpose = 'password_reset'",
    )
    .fetch_one(&hub.db.pool)
    .await
    .unwrap();
    assert!((hours - 1.0).abs() < 1e-6, "{hours}");
}

#[tokio::test]
async fn a_weak_reset_password_does_not_burn_the_token() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    assert_accepted(hub.request(RESET, &email).await).await;
    let token = reset_token(&email).await;
    let resp = hub.complete_reset(&token, "elevenchars").await;
    assert_eq!(
        error_of(resp).await,
        (
            StatusCode::BAD_REQUEST,
            "password must be at least 12 characters".into()
        )
    );
    // An invalid password with a bad token is still the password's 400.
    let resp = hub.complete_reset("garbage", "short").await;
    assert_eq!(
        error_of(resp).await.1,
        "password must be at least 12 characters"
    );
    assert_eq!(
        hub.complete_reset(&token, NEW_PASSWORD).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_bad_reset_token_changes_nothing() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (body, session) = hub.sign_up(&email).await;
    let user_id = body["user"]["id"].as_str().unwrap();

    // Used.
    assert_accepted(hub.request(RESET, &email).await).await;
    let used = reset_token(&email).await;
    assert_eq!(
        hub.complete_reset(&used, NEW_PASSWORD).await.status(),
        StatusCode::OK
    );
    let hash = hub.password_hash(user_id).await;
    // Expired.
    assert_accepted(hub.request(RESET, &email).await).await;
    let expired = mailed_tokens(
        &email,
        wipe_hub::mail::PASSWORD_RESET_SUBJECT,
        "reset-password",
        2,
    )
    .await
    .pop()
    .unwrap();
    sqlx::query("UPDATE auth_tokens SET expires_at = now() - interval '1 second'")
        .execute(&hub.db.pool)
        .await
        .unwrap();
    // A fresh session that a bad reset must not end.
    let fresh = cookie_pair(&hub.sign_in(&email, NEW_PASSWORD).await);
    let audit_rows = hub.count("audit_log").await;

    let garbage_43 = "A".repeat(43);
    for t in [
        used.as_str(),
        expired.as_str(),
        "garbage",
        garbage_43.as_str(),
    ] {
        let resp = hub.complete_reset(t, "yet another passphrase").await;
        assert!(resp.headers().get(SET_COOKIE).is_none());
        assert_eq!(
            error_of(resp).await,
            (StatusCode::BAD_REQUEST, BAD_RESET.into()),
            "{t}"
        );
    }
    assert_eq!(hub.password_hash(user_id).await, hash);
    // The used and expired tokens are real: one failure row each. The
    // garbage ones write nothing.
    assert_eq!(hub.count("audit_log").await, audit_rows + 2);
    let failed = hub.audit_details("auth.password_reset_failed").await;
    let who = format!("user:{user_id}");
    assert_eq!(
        failed,
        vec![
            (
                who.clone(),
                json!({"reason": "used", "peer": "ip:127.0.0.1"})
            ),
            (
                who.clone(),
                json!({"reason": "expired", "peer": "ip:127.0.0.1"})
            ),
        ]
    );
    assert_eq!(
        hub.audited("auth.password_reset_failed").await[0].0,
        "anonymous"
    );
    assert_eq!(hub.me(Some(&fresh)).await.0, StatusCode::OK);
    // The successful reset ended the sign-up session.
    assert_eq!(hub.me(Some(&session)).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn completing_a_reset_retires_outstanding_links() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    assert_accepted(hub.request(MAGIC, &email).await).await;
    assert_accepted(hub.request(RESET, &email).await).await;
    assert_accepted(hub.request(RESET, &email).await).await;
    let magic = magic_token(&email).await;
    let resets = mailed_tokens(
        &email,
        wipe_hub::mail::PASSWORD_RESET_SUBJECT,
        "reset-password",
        2,
    )
    .await;

    // Tokens only work for their own purpose.
    let resp = hub.complete_reset(&magic, NEW_PASSWORD).await;
    assert_eq!(error_of(resp).await.1, BAD_RESET);
    let resp = hub.magic_sign_in(&resets[0], None).await;
    assert_eq!(error_of(resp).await.1, BAD_MAGIC);

    assert_eq!(
        hub.complete_reset(&resets[1], NEW_PASSWORD).await.status(),
        StatusCode::OK
    );
    let resp = hub
        .complete_reset(&resets[0], "yet another passphrase")
        .await;
    assert_eq!(
        error_of(resp).await,
        (StatusCode::BAD_REQUEST, BAD_RESET.into())
    );
    let resp = hub.magic_sign_in(&magic, None).await;
    assert_eq!(
        error_of(resp).await,
        (StatusCode::BAD_REQUEST, BAD_MAGIC.into())
    );
}

#[tokio::test]
async fn creating_a_token_purges_that_users_expired_tokens() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    hub.sign_up(&unique_address()).await;
    sqlx::query("UPDATE auth_tokens SET expires_at = now() - interval '1 second'")
        .execute(&hub.db.pool)
        .await
        .unwrap();
    assert_accepted(hub.request(MAGIC, &email).await).await;
    magic_token(&email).await;
    // This user's expired verification token is gone; the other user's stays.
    hub.wait_for_count("auth_tokens", 2).await;
    let purposes: Vec<String> =
        sqlx::query_scalar("SELECT purpose FROM auth_tokens ORDER BY purpose")
            .fetch_all(&hub.db.pool)
            .await
            .unwrap();
    assert_eq!(purposes, vec!["magic_link", "verify_email"]);
}

#[tokio::test]
async fn five_emails_per_address_then_a_one_hour_lock() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    verification_token(&email).await;
    // Start the address's count from zero.
    sqlx::query("DELETE FROM auth_throttle")
        .execute(&hub.db.pool)
        .await
        .unwrap();
    let key = format!("mail:to:{}", email.to_lowercase());

    for i in 1..=5 {
        let resp = hub.request(MAGIC, &email).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED, "request {i}");
    }
    mailed_tokens(&email, wipe_hub::mail::MAGIC_LINK_SUBJECT, "magic-link", 5).await;
    let before = hub.counts().await;
    let retry = assert_mail_locked(hub.request(MAGIC, &email).await).await;
    assert!((3590..=3600).contains(&retry), "{retry}");
    assert_eq!(hub.counts().await, before);
    assert_eq!(hub.throttle_refused(&key).await, Some(1));
    // The refused request's IP charge was given back.
    assert_eq!(hub.throttle_failures("mail:ip:127.0.0.1").await, Some(5));

    let locked = hub.audit_details("auth.mail_locked").await;
    assert_eq!(locked.len(), 1, "{locked:?}");
    assert_eq!(locked[0].0, key);
    assert!(locked[0].1["until"].as_str().unwrap().ends_with('Z'));
    assert_eq!(locked[0].1["peer"], "ip:127.0.0.1");
    assert_eq!(hub.audited("auth.mail_locked").await[0].0, "anonymous");

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        messages_with_subject(&email, wipe_hub::mail::MAGIC_LINK_SUBJECT)
            .await
            .len(),
        5
    );

    // Once the lock elapses, requests are accepted again.
    hub.expire_locks().await;
    assert_accepted(hub.request(MAGIC, &email).await).await;
    assert_eq!(hub.throttle_failures(&key).await, Some(1));
    assert_eq!(hub.throttle_until(&key).await, None);
    let summary = hub.audit_details("auth.mail_lock_summary").await;
    assert_eq!(summary.len(), 1, "{summary:?}");
    assert_eq!(summary[0].0, key);
    assert_eq!(summary[0].1["refused"], 1);
}

#[tokio::test]
async fn every_email_sending_route_shares_the_address_limit() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (_, session) = hub.sign_up(&email).await;
    let sign_up = || {
        json!({
            "email": email.to_uppercase(),
            "password": PASSWORD,
            "name": "Again",
            "organization_name": "Again Ltd",
        })
    };
    let resp = hub.post("/api/auth/sign-up", sign_up(), None).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let resp = hub
        .post_empty("/api/auth/verification-email", Some(&session))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_accepted(hub.request(RESET, &email).await).await;
    assert_accepted(hub.request(MAGIC, &email).await).await;
    // Verification, resend, reset and magic link.
    let mailpit = Mailpit::new();
    let deadline = std::time::Instant::now() + MAIL_WAIT;
    while mailpit.messages_to(&email).await.len() < 4 {
        assert!(std::time::Instant::now() < deadline, "mail did not arrive");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The 6th email-sending request, on any route, is refused and writes nothing.
    let before = hub.counts().await;
    let orgs = hub.count("organizations").await;
    assert_mail_locked(hub.post("/api/auth/sign-up", sign_up(), None).await).await;
    assert_mail_locked(
        hub.post_empty("/api/auth/verification-email", Some(&session))
            .await,
    )
    .await;
    assert_mail_locked(hub.request(RESET, &email).await).await;
    assert_mail_locked(hub.request(MAGIC, &email).await).await;
    assert_eq!(hub.counts().await, before);
    assert_eq!(hub.count("organizations").await, orgs);
    assert_eq!(
        hub.throttle_refused(&format!("mail:to:{}", email.to_lowercase()))
            .await,
        Some(4)
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(mailpit.messages_to(&email).await.len(), 4);
}

#[tokio::test]
async fn twenty_emails_from_one_ip_then_a_lock() {
    let hub = spawn_hub().await;
    for i in 1..=20 {
        let resp = hub.request(MAGIC, &unique_address()).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED, "request {i}");
    }
    let fresh = unique_address();
    assert_mail_locked(hub.request(RESET, &fresh).await).await;
    let locked = hub.audit_details("auth.mail_locked").await;
    assert_eq!(locked.len(), 1, "{locked:?}");
    assert_eq!(locked[0].0, "mail:ip:127.0.0.1");
    // The IP key refused first: the address key was never touched.
    assert_eq!(
        hub.throttle_failures(&format!("mail:to:{fresh}")).await,
        None
    );
    assert_eq!(hub.throttle_refused("mail:ip:127.0.0.1").await, Some(1));
    // Sign-in has its own keys and is unaffected.
    assert_eq!(
        hub.sign_in(&fresh, PASSWORD).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_throttled_sign_up_creates_nothing() {
    let hub = spawn_hub().await;
    let email = unique_address();
    for _ in 0..5 {
        assert_accepted(hub.request(MAGIC, &email).await).await;
    }
    // An invalid body is still a 400 first.
    let resp = hub
        .post(
            "/api/auth/sign-up",
            json!({"email": email, "password": "elevenchars", "name": "A", "organization_name": "O"}),
            None,
        )
        .await;
    assert_eq!(error_of(resp).await.0, StatusCode::BAD_REQUEST);
    let resp = hub
        .post(
            "/api/auth/sign-up",
            json!({"email": email, "password": PASSWORD, "name": "A", "organization_name": "O"}),
            None,
        )
        .await;
    assert_mail_locked(resp).await;
    for table in ["organizations", "users", "sessions", "auth_tokens"] {
        assert_eq!(hub.count(table).await, 0, "{table}");
    }
    assert!(hub.audited("auth.sign_up").await.is_empty());
}

#[tokio::test]
async fn hammering_a_mail_lock_is_counted_then_summarised_once() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let key = format!("mail:to:{email}");
    for _ in 0..5 {
        assert_accepted(hub.request(RESET, &email).await).await;
    }
    let audit_rows = hub.count("audit_log").await;
    for _ in 0..50 {
        assert_mail_locked(hub.request(RESET, &email).await).await;
    }
    assert_eq!(hub.count("audit_log").await, audit_rows);
    assert_eq!(hub.throttle_refused(&key).await, Some(50));
    assert_eq!(hub.throttle_failures(&key).await, Some(5));

    hub.expire_locks().await;
    assert_accepted(hub.request(RESET, &email).await).await;
    let summary = hub.audit_details("auth.mail_lock_summary").await;
    assert_eq!(summary.len(), 1, "{summary:?}");
    assert_eq!(summary[0].0, key);
    assert_eq!(summary[0].1["refused"], 50);
    assert_eq!(summary[0].1["peer"], "ip:127.0.0.1");
    assert_eq!(hub.audited("auth.mail_locked").await.len(), 1);
    assert!(hub.audited("auth.sign_in_lock_summary").await.is_empty());
}

#[tokio::test]
async fn purged_rows_are_summarised_under_their_keys_action() {
    let hub = spawn_hub().await;
    sqlx::query(
        "INSERT INTO auth_throttle (key, failures, refused, window_started_at, locked_until) VALUES \
         ('mail:to:stale@example.com', 5, 4, now() - interval '2 days', now() - interval '1 day'), \
         ('email:stale@example.com', 5, 3, now() - interval '2 days', now() - interval '1 day')",
    )
    .execute(&hub.db.pool)
    .await
    .unwrap();
    // A mail charge purges both stale rows.
    assert_accepted(hub.request(MAGIC, &unique_address()).await).await;
    let mail = hub.audit_details("auth.mail_lock_summary").await;
    assert_eq!(mail.len(), 1, "{mail:?}");
    assert_eq!(mail[0].0, "mail:to:stale@example.com");
    assert_eq!(mail[0].1["refused"], 4);
    assert!(mail[0].1.get("peer").is_none());
    let sign_in = hub.audit_details("auth.sign_in_lock_summary").await;
    assert_eq!(sign_in.len(), 1, "{sign_in:?}");
    assert_eq!(sign_in[0].0, "email:stale@example.com");
}

#[tokio::test]
async fn a_reset_clears_the_sign_in_lock() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    let key = format!("email:{}", email.to_lowercase());
    for _ in 0..5 {
        hub.sign_in(&email, "wrong password!!").await;
    }
    assert_locked(hub.sign_in(&email, PASSWORD).await).await;

    assert_accepted(hub.request(RESET, &email).await).await;
    let token = reset_token(&email).await;
    assert_eq!(
        hub.complete_reset(&token, NEW_PASSWORD).await.status(),
        StatusCode::OK
    );
    assert_eq!(hub.throttle_failures(&key).await, None);
    assert_eq!(
        hub.sign_in(&email, NEW_PASSWORD).await.status(),
        StatusCode::OK
    );
    let summary = hub.audit_details("auth.sign_in_lock_summary").await;
    assert_eq!(summary.len(), 1, "{summary:?}");
    assert_eq!(summary[0].0, key);
}

#[tokio::test]
async fn resend_when_verified_is_409_without_a_mail_charge() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (_, session) = hub.sign_up(&email).await;
    let verification = verification_token(&email).await;
    let resp = hub
        .post(
            "/api/auth/verify-email",
            json!({"token": verification}),
            None,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let to_key = format!("mail:to:{}", email.to_lowercase());
    // Sign-up's own charge.
    assert_eq!(hub.throttle_failures(&to_key).await, Some(1));
    assert_eq!(hub.throttle_failures("mail:ip:127.0.0.1").await, Some(1));

    let resp = hub
        .post_empty("/api/auth/verification-email", Some(&session))
        .await;
    assert_eq!(
        error_of(resp).await,
        (
            StatusCode::CONFLICT,
            "this email address is already verified".into()
        )
    );
    assert_eq!(hub.throttle_failures(&to_key).await, Some(1));
    assert_eq!(hub.throttle_failures("mail:ip:127.0.0.1").await, Some(1));
}

#[tokio::test]
async fn a_reset_completed_concurrently_succeeds_once() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (body, _) = hub.sign_up(&email).await;
    assert_accepted(hub.request(RESET, &email).await).await;
    let token = reset_token(&email).await;
    let mut set = tokio::task::JoinSet::new();
    for i in 0..5 {
        let req = hub
            .post_req(RESET_COMPLETE, None)
            .json(&json!({ "token": token, "password": format!("{NEW_PASSWORD} {i}") }));
        set.spawn(async move { req.send().await.unwrap().status() });
    }
    let (mut ok, mut bad) = (0, 0);
    while let Some(status) = set.join_next().await {
        match status.unwrap() {
            StatusCode::OK => ok += 1,
            StatusCode::BAD_REQUEST => bad += 1,
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!((ok, bad), (1, 4));
    assert_eq!(hub.audited("auth.password_reset").await.len(), 1);
    // Every loser held the real, now-used token, which is audited once.
    let failed = hub.audit_details("auth.password_reset_failed").await;
    assert_eq!(
        failed,
        vec![(
            user_of(&body),
            json!({"reason": "used", "peer": "ip:127.0.0.1"})
        )]
    );
}

#[tokio::test]
async fn a_magic_link_takes_a_pre_registered_account_back_from_its_creator() {
    let hub = spawn_hub().await;
    let victim = unique_address();
    // The attacker signs up with the victim's address and keeps session S;
    // they know the password P (`PASSWORD`).
    let (body, attacker_session) = hub.sign_up(&victim).await;
    let user_id = body["user"]["id"].as_str().unwrap().to_string();

    // The victim gets in through their mailbox, in a fresh browser.
    assert_accepted(hub.request(MAGIC, &victim).await).await;
    let resp = hub.magic_sign_in(&magic_token(&victim).await, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let victim_session = cookie_pair(&resp);

    assert_eq!(
        hub.me(Some(&attacker_session)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(hub.me(Some(&victim_session)).await.0, StatusCode::OK);
    assert_eq!(
        hub.sign_in(&victim, PASSWORD).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(hub.password_hash(&user_id).await, "!unusable");
    assert_eq!(
        hub.audit_details("auth.magic_link_sign_in").await[0].1,
        json!({"rotated": false, "email_verified": true, "sessions_ended": 1,
               "password_cleared": true, "peer": "ip:127.0.0.1"})
    );

    // A reset then sets a working password.
    assert_accepted(hub.request(RESET, &victim).await).await;
    let token = reset_token(&victim).await;
    assert_eq!(
        hub.complete_reset(&token, NEW_PASSWORD).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        hub.sign_in(&victim, NEW_PASSWORD).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_magic_link_on_a_verified_account_keeps_its_password_and_sessions() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (_, session) = hub.sign_up(&email).await;
    let verification = verification_token(&email).await;
    let resp = hub
        .post(
            "/api/auth/verify-email",
            json!({"token": verification}),
            None,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    assert_accepted(hub.request(MAGIC, &email).await).await;
    let resp = hub.magic_sign_in(&magic_token(&email).await, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(hub.me(Some(&session)).await.0, StatusCode::OK);
    assert_eq!(hub.sign_in(&email, PASSWORD).await.status(), StatusCode::OK);
    assert_eq!(
        hub.audit_details("auth.magic_link_sign_in").await[0].1,
        json!({"rotated": false, "email_verified": false, "sessions_ended": 0,
               "password_cleared": false, "peer": "ip:127.0.0.1"})
    );
}

#[tokio::test]
async fn failed_uses_of_real_magic_links_are_audited() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (body, _) = hub.sign_up(&email).await;
    let who = user_of(&body);

    assert_accepted(hub.request(MAGIC, &email).await).await;
    let used = magic_token(&email).await;
    assert_eq!(
        hub.magic_sign_in(&used, None).await.status(),
        StatusCode::OK
    );
    assert_accepted(hub.request(MAGIC, &email).await).await;
    let expired = mailed_tokens(&email, wipe_hub::mail::MAGIC_LINK_SUBJECT, "magic-link", 2)
        .await
        .pop()
        .unwrap();
    sqlx::query("UPDATE auth_tokens SET expires_at = now() - interval '1 second'")
        .execute(&hub.db.pool)
        .await
        .unwrap();

    let audit_rows = hub.count("audit_log").await;
    let garbage_43 = "A".repeat(43);
    for t in [
        used.as_str(),
        expired.as_str(),
        "garbage",
        garbage_43.as_str(),
    ] {
        let resp = hub.magic_sign_in(t, None).await;
        assert_eq!(
            error_of(resp).await,
            (StatusCode::BAD_REQUEST, BAD_MAGIC.into()),
            "{t}"
        );
    }
    assert_eq!(hub.count("audit_log").await, audit_rows + 2);
    assert_eq!(
        hub.audit_details("auth.magic_link_failed").await,
        vec![
            (
                who.clone(),
                json!({"reason": "used", "peer": "ip:127.0.0.1"})
            ),
            (
                who.clone(),
                json!({"reason": "expired", "peer": "ip:127.0.0.1"})
            ),
        ]
    );
    assert_eq!(
        hub.audited("auth.magic_link_failed").await[0].0,
        "anonymous"
    );
    // A reset token posted to the magic route is not a magic link: no row.
    assert_accepted(hub.request(RESET, &email).await).await;
    let reset = reset_token(&email).await;
    let resp = hub.magic_sign_in(&reset, None).await;
    assert_eq!(error_of(resp).await.1, BAD_MAGIC);
    assert_eq!(hub.audited("auth.magic_link_failed").await.len(), 2);
}

#[tokio::test]
async fn a_reset_in_another_users_browser_ends_that_session() {
    let hub = spawn_hub().await;
    let a = unique_address();
    hub.sign_up(&a).await;
    let (_, b_session) = hub.sign_up(&unique_address()).await;

    assert_accepted(hub.request(RESET, &a).await).await;
    let token = reset_token(&a).await;
    let resp = hub
        .post(
            RESET_COMPLETE,
            json!({ "token": token, "password": NEW_PASSWORD }),
            Some(&b_session),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(set_cookie(&resp).starts_with("wipehub_session=;"));
    assert_eq!(hub.me(Some(&b_session)).await.0, StatusCode::UNAUTHORIZED);
    // Only A's own sessions are counted.
    assert_eq!(
        hub.audit_details("auth.password_reset").await[0].1["sessions"],
        json!(1)
    );
}

#[tokio::test]
async fn a_refused_send_leaves_other_stale_rows_and_the_audit_log_alone() {
    let hub = spawn_hub().await;
    let email = unique_address();
    for _ in 0..5 {
        assert_accepted(hub.request(MAGIC, &email).await).await;
    }
    sqlx::query(
        "INSERT INTO auth_throttle (key, failures, refused, window_started_at, locked_until) VALUES \
         ('mail:to:stale@example.com', 5, 4, now() - interval '2 days', now() - interval '1 day')",
    )
    .execute(&hub.db.pool)
    .await
    .unwrap();
    let audit_rows = hub.count("audit_log").await;
    assert_mail_locked(hub.request(MAGIC, &email).await).await;
    assert_eq!(
        hub.throttle_refused("mail:to:stale@example.com").await,
        Some(4)
    );
    assert_eq!(hub.count("audit_log").await, audit_rows);
}

#[tokio::test]
async fn a_used_link_posted_again_and_again_is_audited_once() {
    let hub = spawn_hub().await;
    let email = unique_address();
    hub.sign_up(&email).await;
    assert_accepted(hub.request(MAGIC, &email).await).await;
    let token = magic_token(&email).await;
    assert_eq!(
        hub.magic_sign_in(&token, None).await.status(),
        StatusCode::OK
    );
    for _ in 0..3 {
        let resp = hub.magic_sign_in(&token, None).await;
        assert_eq!(
            error_of(resp).await,
            (StatusCode::BAD_REQUEST, BAD_MAGIC.into())
        );
    }
    assert_eq!(hub.audited("auth.magic_link_failed").await.len(), 1);
}

#[tokio::test]
async fn an_ip_window_restart_is_summarised_even_when_the_address_refuses() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let to_key = format!("mail:to:{email}");
    sqlx::query(
        "INSERT INTO auth_throttle (key, failures, refused, window_started_at, locked_until) VALUES \
         ('mail:ip:127.0.0.1', 20, 7, now() - interval '2 hours', now() - interval '1 hour'), \
         ($1, 5, 0, now(), now() + interval '1 hour')",
    )
    .bind(&to_key)
    .execute(&hub.db.pool)
    .await
    .unwrap();

    assert_mail_locked(hub.request(MAGIC, &email).await).await;
    let summary = hub.audit_details("auth.mail_lock_summary").await;
    assert_eq!(summary.len(), 1, "{summary:?}");
    assert_eq!(summary[0].0, "mail:ip:127.0.0.1");
    assert_eq!(summary[0].1["refused"], 7);
    // The restarted IP window's charge was given back.
    assert_eq!(hub.throttle_failures("mail:ip:127.0.0.1").await, Some(0));
    assert_eq!(hub.throttle_refused(&to_key).await, Some(1));
}
