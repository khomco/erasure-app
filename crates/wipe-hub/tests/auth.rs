//! `/api/auth/*` over real HTTP against a migrated test database and
//! Mailpit (portal-accounts story 1). Needs `docker compose up -d`; fails,
//! never skips, when the compose services are down.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::header::{COOKIE, SET_COOKIE};
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
}

async fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

async fn spawn_hub_with(smtp_port: u16, public_base_url: &str) -> Hub {
    spawn_hub_from(smtp_port, public_base_url, DEFAULT_MAIL_FROM).await
}

async fn spawn_hub_from(smtp_port: u16, public_base_url: &str, mail_from: &str) -> Hub {
    let db = TestDb::migrated().await;
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
    let state = AppState::new(config).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(wipe_hub::serve_on(listener, state));
    // No cookie store: every test passes cookies explicitly.
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    Hub { addr, http, db }
}

async fn spawn_hub() -> Hub {
    spawn_hub_with(DEFAULT_SMTP_PORT, "http://127.0.0.1:7900").await
}

impl Hub {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn post(&self, path: &str, body: Value, cookie: Option<&str>) -> Response {
        let mut req = self.http.post(self.url(path)).json(&body);
        if let Some(c) = cookie {
            req = req.header(COOKIE, c);
        }
        req.send().await.unwrap()
    }

    async fn post_empty(&self, path: &str, cookie: Option<&str>) -> Response {
        let mut req = self.http.post(self.url(path));
        if let Some(c) = cookie {
            req = req.header(COOKIE, c);
        }
        req.send().await.unwrap()
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
    let prefix = format!("{base}/verify-email?token=");
    let start = text
        .find(&prefix)
        .unwrap_or_else(|| panic!("no verification link in {text:?}"))
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
        .http
        .post(hub.url("/api/auth/sign-up"))
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
        .http
        .post(hub.url("/api/auth/sign-up"))
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
