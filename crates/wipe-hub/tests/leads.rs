//! Sales leads (lead-capture story 1): `POST /api/leads`, its defences
//! (CAP-2: per-IP throttle, honeypot, validation, one lead per address, the
//! email-send throttle), the stored consent (CAP-4), the site-only CORS
//! headers and `POST /api/leads/unsubscribe`. One test per row of the story's
//! I/O matrix. Needs `docker compose up -d`; fails, never skips, when it is
//! down.

mod support;

use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use support::hub::{
    error_of, mail_count, mail_texts, spawn_hub, spawn_hub_with, token_in, Hub, MAIL_LOCKED,
};
use support::{unique_address, TestDb};
use tokio::net::TcpListener;
use uuid::Uuid;
use wipe_hub::accounts;
use wipe_hub::config::DEFAULT_SITE_ORIGIN;
use wipe_hub::leads::{self, LeadKind, NewLead, CONSENT_CONTACT, CONSENT_EARLY_ACCESS};
use wipe_hub::mail::{LEAD_CONTACT_SUBJECT, LEAD_EARLY_ACCESS_SUBJECT};
use wipe_hub::throttle::LEAD_IP_LIMIT;

const SITE: &str = DEFAULT_SITE_ORIGIN;
const NAME: &str = "Ada Lovelace";
const COMPANY: &str = "Analytical Engines Ltd";
const ROLE: &str = "Head of operations";
const MESSAGE: &str = "Please call me about the bench pilot.";

fn early_access(email: &str) -> Value {
    json!({
        "kind": "early-access",
        "name": NAME,
        "email": email,
        "company": COMPANY,
        "role": ROLE,
        "segment": "itad",
        "message": "",
        "consent": true,
        "consent_text": CONSENT_EARLY_ACCESS,
        "website": "",
    })
}

fn contact(email: &str) -> Value {
    json!({
        "kind": "contact",
        "name": "Ada King",
        "email": email,
        "company": "Countess Ltd",
        "message": MESSAGE,
        "consent": true,
        "consent_text": CONSENT_CONTACT,
    })
}

/// `POST /api/leads` from the site, with the CSRF cookie and header.
async fn submit(hub: &Hub, body: Value) -> reqwest::Response {
    hub.post_req("/api/leads", None)
        .header("origin", SITE)
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn unsubscribe(hub: &Hub, token: &str) -> reqwest::Response {
    hub.post("/api/leads/unsubscribe", json!({ "token": token }), None)
        .await
}

async fn assert_accepted(resp: reqwest::Response) {
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_site_cors(resp.headers());
    assert_eq!(resp.json::<Value>().await.unwrap(), json!({}));
}

fn assert_site_cors(h: &HeaderMap) {
    assert_eq!(h["access-control-allow-origin"], SITE);
    assert_eq!(h["access-control-allow-credentials"], "true");
    assert_eq!(h["access-control-expose-headers"], "Retry-After");
    assert!(
        h.get_all("vary").iter().any(|v| v == "Origin"),
        "Vary: Origin in {h:?}"
    );
}

fn assert_no_cors(h: &HeaderMap) {
    let cors: Vec<_> = h
        .keys()
        .filter(|k| k.as_str().starts_with("access-control-"))
        .collect();
    assert!(cors.is_empty(), "unexpected {cors:?}");
}

#[derive(Debug, sqlx::FromRow)]
struct Lead {
    id: Uuid,
    email: String,
    name: String,
    company: String,
    role: Option<String>,
    segment: Option<String>,
    status: String,
    unsubscribed: bool,
}

async fn leads_of(hub: &Hub) -> Vec<Lead> {
    sqlx::query_as(
        "SELECT id, email, name, company, role, segment, status, \
                unsubscribed_at IS NOT NULL AS unsubscribed \
         FROM leads ORDER BY created_at",
    )
    .fetch_all(&hub.db.pool)
    .await
    .unwrap()
}

/// `(kind, message, consent_text, consented_at is set)` of every submission.
async fn submissions(hub: &Hub) -> Vec<(String, Option<String>, String, bool)> {
    sqlx::query_as(
        "SELECT kind, message, consent_text, consented_at IS NOT NULL \
         FROM lead_submissions ORDER BY consented_at, id",
    )
    .fetch_all(&hub.db.pool)
    .await
    .unwrap()
}

/// Every `lead.*` audit action.
async fn lead_actions(hub: &Hub) -> Vec<String> {
    hub.actions()
        .await
        .into_iter()
        .filter(|a| a.starts_with("lead."))
        .collect()
}

/// The unsubscribe tokens of `n` confirmation emails to `addr`, newest first.
async fn unsubscribe_tokens(addr: &str, subject: &str, n: usize) -> Vec<String> {
    mail_texts(addr, subject, n)
        .await
        .iter()
        .map(|t| token_in(t, "unsubscribe"))
        .collect()
}

#[tokio::test]
async fn a_new_early_access_lead_is_stored_with_its_consent_and_confirmed() {
    let hub = spawn_hub().await;
    let email = unique_address();
    assert_accepted(submit(&hub, early_access(&email)).await).await;

    let leads = leads_of(&hub).await;
    assert_eq!(leads.len(), 1);
    let lead = &leads[0];
    assert_eq!(lead.email, email);
    assert_eq!(lead.name, NAME);
    assert_eq!(lead.company, COMPANY);
    assert_eq!(lead.role.as_deref(), Some(ROLE));
    assert_eq!(lead.segment.as_deref(), Some("itad"));
    assert_eq!(lead.status, "new");
    assert!(!lead.unsubscribed);
    assert_eq!(
        submissions(&hub).await,
        [(
            "early-access".to_string(),
            None,
            CONSENT_EARLY_ACCESS.to_string(),
            true
        )]
    );

    let audited = hub.audited("lead.submitted").await;
    assert_eq!(
        audited,
        [(
            "anonymous".to_string(),
            format!("lead:{}", lead.id),
            json!({"kind": "early-access", "created": true, "suppressed": false}),
        )]
    );
    // No personal data in any audit row about the lead.
    let all: Vec<String> =
        sqlx::query_scalar("SELECT subject || ' ' || detail::text FROM audit_log")
            .fetch_all(&hub.db.pool)
            .await
            .unwrap();
    for row in all.iter().filter(|r| r.starts_with("lead:")) {
        for personal in [email.as_str(), NAME, COMPANY, ROLE, "127.0.0.1"] {
            assert!(!row.contains(personal), "{personal} in {row}");
        }
    }

    let text = mail_texts(&email, LEAD_EARLY_ACCESS_SUBJECT, 1)
        .await
        .remove(0);
    assert!(text.contains("early-access list"), "{text}");
    assert!(text.contains("reply by email"), "{text}");
    for echoed in [NAME, COMPANY, ROLE] {
        assert!(!text.contains(echoed), "{echoed} echoed in {text}");
    }
    let token = token_in(&text, "unsubscribe");
    assert_eq!(token.len(), 43);
    // Only the hash is stored: the column is the token's SHA-256 ...
    let stored: Vec<u8> = sqlx::query_scalar("SELECT unsubscribe_token_hash FROM lead_submissions")
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    assert_eq!(stored, accounts::hash_token(&token).unwrap());
    // ... and no column of the row holds the raw token text.
    let raw: i64 =
        sqlx::query_scalar("SELECT count(*) FROM lead_submissions s WHERE strpos(s::text, $1) > 0")
            .bind(&token)
            .fetch_one(&hub.db.pool)
            .await
            .unwrap();
    assert_eq!(raw, 0);
}

#[tokio::test]
async fn a_known_address_updates_its_lead_and_keeps_every_submission() {
    let hub = spawn_hub().await;
    let email = unique_address();
    assert_accepted(submit(&hub, early_access(&email)).await).await;
    sqlx::query("UPDATE leads SET status = 'qualified'")
        .execute(&hub.db.pool)
        .await
        .unwrap();
    let shouted = email.to_uppercase();
    assert_accepted(submit(&hub, contact(&shouted)).await).await;

    let leads = leads_of(&hub).await;
    assert_eq!(leads.len(), 1, "{leads:?}");
    let lead = &leads[0];
    assert_eq!(lead.email, shouted, "the address as typed this time");
    assert_eq!(lead.name, "Ada King");
    assert_eq!(lead.company, "Countess Ltd");
    assert_eq!(lead.role.as_deref(), Some(ROLE), "kept: contact has none");
    assert_eq!(lead.segment.as_deref(), Some("itad"), "kept");
    assert_eq!(lead.status, "qualified", "the vendor's status is kept");
    assert_eq!(
        submissions(&hub).await,
        [
            (
                "early-access".to_string(),
                None,
                CONSENT_EARLY_ACCESS.to_string(),
                true
            ),
            (
                "contact".to_string(),
                Some(MESSAGE.to_string()),
                CONSENT_CONTACT.to_string(),
                true
            ),
        ]
    );
    let details: Vec<Value> = hub
        .audited("lead.submitted")
        .await
        .into_iter()
        .map(|(_, _, d)| d)
        .collect();
    assert_eq!(
        details,
        [
            json!({"kind": "early-access", "created": true, "suppressed": false}),
            json!({"kind": "contact", "created": false, "suppressed": false}),
        ]
    );
    // Mailpit matches recipients case-insensitively.
    let text = mail_texts(&email, LEAD_CONTACT_SUBJECT, 1).await.remove(0);
    assert!(
        !text.contains(MESSAGE),
        "the message is never echoed: {text}"
    );
}

/// `unsubscribed_at` of the only lead, as text so it compares exactly.
async fn unsubscribed_at(hub: &Hub) -> Option<String> {
    sqlx::query_scalar("SELECT unsubscribed_at::text FROM leads")
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
}

/// Email-send throttle charges against `mail:to:<addr>`, if any.
async fn mail_to_charges(hub: &Hub, addr: &str) -> Option<i32> {
    sqlx::query_scalar("SELECT failures FROM auth_throttle WHERE key = $1")
        .bind(format!("mail:to:{}", addr.to_lowercase()))
        .fetch_optional(&hub.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn resubmitting_after_unsubscribing_is_stored_but_never_emailed() {
    let hub = spawn_hub().await;
    let email = unique_address();
    assert_accepted(submit(&hub, early_access(&email)).await).await;
    let token = unsubscribe_tokens(&email, LEAD_EARLY_ACCESS_SUBJECT, 1)
        .await
        .remove(0);
    assert_eq!(unsubscribe(&hub, &token).await.status(), StatusCode::OK);
    let suppressed_since = unsubscribed_at(&hub).await;
    assert!(suppressed_since.is_some());
    let charged = mail_to_charges(&hub, &email).await.unwrap();

    // A third party who knows the address cannot lift the unsubscribe.
    assert_accepted(submit(&hub, contact(&email)).await).await;
    assert_eq!(unsubscribed_at(&hub).await, suppressed_since, "unchanged");
    let lead = &leads_of(&hub).await[0];
    assert_eq!(lead.name, "Ada King", "the details still update");
    assert_eq!(hub.count("lead_submissions").await, 2);
    assert_eq!(
        submissions(&hub).await[1],
        (
            "contact".to_string(),
            Some(MESSAGE.to_string()),
            CONSENT_CONTACT.to_string(),
            true
        )
    );
    let last = hub.audited("lead.submitted").await.pop().unwrap().2;
    assert_eq!(
        last,
        json!({"kind": "contact", "created": false, "suppressed": true})
    );
    // Charged like any other submission, so a throttle refusal cannot tell
    // an unsubscribed address from any other ...
    assert_eq!(mail_to_charges(&hub, &email).await, Some(charged + 1));
    // ... but nothing is sent: only the first confirmation ever arrived.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(mail_count(&email).await, 1);
}

#[tokio::test]
async fn a_lead_is_stored_when_smtp_is_down() {
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let hub = spawn_hub_with(port).await;
    let email = unique_address();
    assert_accepted(submit(&hub, early_access(&email)).await).await;
    assert_eq!(hub.count("leads").await, 1);
    assert_eq!(hub.count("lead_submissions").await, 1);
    assert_eq!(hub.audited("lead.submitted").await.len(), 1);
    // The failed send is only logged; the hub keeps serving.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_accepted(submit(&hub, contact(&email)).await).await;
    assert_eq!(hub.count("lead_submissions").await, 2);
    assert_eq!(mail_count(&email).await, 0);
}

#[tokio::test]
async fn concurrent_first_submissions_make_one_lead() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let (a, b) = tokio::join!(
        submit(&hub, early_access(&email)),
        submit(&hub, contact(&email.to_uppercase())),
    );
    assert_accepted(a).await;
    assert_accepted(b).await;
    assert_eq!(hub.count("leads").await, 1);
    assert_eq!(hub.count("lead_submissions").await, 2);
    let created: Vec<bool> = hub
        .audited("lead.submitted")
        .await
        .into_iter()
        .map(|(_, _, d)| d["created"].as_bool().unwrap())
        .collect();
    assert_eq!(created.len(), 2);
    assert_eq!(created.iter().filter(|c| **c).count(), 1, "{created:?}");
    // Both confirmations go out.
    let started = Instant::now();
    mail_texts(&email, LEAD_EARLY_ACCESS_SUBJECT, 1).await;
    mail_texts(&email, LEAD_CONTACT_SUBJECT, 1).await;
    assert!(started.elapsed() < Duration::from_secs(15));
}

/// The insert race in `leads::upsert`, made deterministic: the second
/// transaction's insert blocks on the first's uncommitted row in the unique
/// `lower(email)` index, finds the conflict once the first commits, and
/// retries as an update of the same lead.
#[tokio::test]
async fn an_insert_that_loses_the_race_retries_as_an_update() {
    let db = TestDb::migrated().await;
    let email = unique_address();
    let new = |email: String| NewLead {
        kind: LeadKind::EarlyAccess,
        name: NAME.into(),
        email,
        company: COMPANY.into(),
        role: Some(ROLE.into()),
        segment: Some("itad".into()),
        message: None,
    };

    let mut tx1 = db.pool.begin().await.unwrap();
    let first = leads::submit(&mut tx1, &new(email.clone())).await.unwrap();
    assert!(first.created);

    let pool = db.pool.clone();
    let shouted = new(email.to_uppercase());
    let second = tokio::spawn(async move {
        let mut tx2 = pool.begin().await.unwrap();
        let s = leads::submit(&mut tx2, &shouted).await.unwrap();
        tx2.commit().await.unwrap();
        s
    });
    // Wait until tx2 is blocked on tx1's row lock, then let tx1 commit.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        if waiting > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "tx2 never blocked on tx1");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tx1.commit().await.unwrap();

    let second = second.await.unwrap();
    assert_eq!(second.lead_id, first.lead_id);
    assert!(!second.created);
    assert!(!second.suppressed);
    let count = |table: &'static str| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(count("leads").await, 1);
    assert_eq!(count("lead_submissions").await, 2);
}

#[tokio::test]
async fn a_filled_honeypot_is_accepted_and_ignored() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let mut body = early_access(&email);
    body["website"] = json!("https://spam.example");
    assert_accepted(submit(&hub, body).await).await;

    assert_eq!(hub.count("leads").await, 0);
    assert_eq!(hub.count("lead_submissions").await, 0);
    assert!(lead_actions(&hub).await.is_empty());
    // Not charged against the email-send throttle either.
    let mail_keys: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_throttle WHERE key LIKE 'mail:%'")
            .fetch_one(&hub.db.pool)
            .await
            .unwrap();
    assert_eq!(mail_keys, 0);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(mail_count(&email).await, 0);
}

#[tokio::test]
async fn invalid_submissions_are_400_and_change_nothing() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let cases: Vec<(Value, &str)> = vec![
        (
            {
                let mut b = early_access(&email);
                b["email"] = json!("not-an-address");
                b
            },
            "email is not a valid email address",
        ),
        (
            {
                let mut b = early_access(&email);
                b.as_object_mut().unwrap().remove("company");
                b
            },
            "company must not be blank",
        ),
        (
            {
                let mut b = early_access(&email);
                b.as_object_mut().unwrap().remove("segment");
                b
            },
            leads::MSG_BAD_SEGMENT,
        ),
        (
            {
                let mut b = contact(&email);
                b["message"] = json!("   ");
                b
            },
            "message must not be blank",
        ),
        (
            {
                let mut b = early_access(&email);
                b["consent"] = json!(false);
                b
            },
            leads::MSG_CONSENT_REQUIRED,
        ),
        (
            {
                let mut b = early_access(&email);
                b.as_object_mut().unwrap().remove("consent");
                b
            },
            leads::MSG_CONSENT_REQUIRED,
        ),
        (
            {
                let mut b = early_access(&email);
                b["consent_text"] = json!("I agree to anything.");
                b
            },
            leads::MSG_CONSENT_CHANGED,
        ),
        (
            {
                let mut b = contact(&email);
                b["consent_text"] = json!(CONSENT_EARLY_ACCESS);
                b
            },
            leads::MSG_CONSENT_CHANGED,
        ),
    ];
    let before = hub.counts().await;
    for (body, want) in cases {
        let resp = submit(&hub, body.clone()).await;
        assert_site_cors(resp.headers());
        assert_eq!(
            error_of(resp).await,
            (StatusCode::BAD_REQUEST, want.to_string()),
            "{body}"
        );
    }
    assert_eq!(
        hub.counts().await,
        before,
        "nothing stored, charged or audited"
    );
    assert_eq!(hub.count("leads").await, 0);
    assert_eq!(hub.count("lead_submissions").await, 0);
}

#[tokio::test]
async fn the_eleventh_submission_from_one_ip_in_an_hour_is_refused() {
    let hub = spawn_hub().await;
    for _ in 0..LEAD_IP_LIMIT {
        assert_accepted(submit(&hub, early_access(&unique_address())).await).await;
    }
    assert_eq!(hub.count("leads").await, i64::from(LEAD_IP_LIMIT));
    let locked = hub.audited("lead.submit_locked").await;
    assert_eq!(locked.len(), 1, "{locked:?}");
    assert_eq!(locked[0].1, "lead-form:ip:127.0.0.1");

    let email = unique_address();
    let resp = submit(&hub, early_access(&email)).await;
    assert_site_cors(resp.headers());
    let retry: u64 = resp.headers()[RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((3500..=3600).contains(&retry), "{retry}");
    assert_eq!(
        error_of(resp).await,
        (
            StatusCode::TOO_MANY_REQUESTS,
            leads::MSG_SUBMIT_LOCKED.to_string()
        )
    );
    // A refused honeypot submission is refused too: the charge comes first.
    let mut bot = early_access(&unique_address());
    bot["website"] = json!("x");
    assert_eq!(
        submit(&hub, bot).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(hub.count("leads").await, i64::from(LEAD_IP_LIMIT));
    assert_eq!(
        hub.count("lead_submissions").await,
        i64::from(LEAD_IP_LIMIT)
    );
    assert_eq!(hub.audited("lead.submit_locked").await.len(), 1);
    // Neither lock row is ever linked to a sales lead.
    for (_, subject, _) in hub.audited("lead.submit_locked").await {
        assert!(!subject.starts_with("lead:"), "{subject}");
    }
}

#[tokio::test]
async fn an_elapsed_lock_admits_one_more_and_is_summarised_once() {
    let hub = spawn_hub().await;
    for _ in 0..LEAD_IP_LIMIT {
        assert_accepted(submit(&hub, early_access(&unique_address())).await).await;
    }
    assert_eq!(
        submit(&hub, early_access(&unique_address())).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    sqlx::query(
        "UPDATE auth_throttle SET locked_until = now() - interval '1 second' WHERE key = $1",
    )
    .bind("lead-form:ip:127.0.0.1")
    .execute(&hub.db.pool)
    .await
    .unwrap();

    assert_accepted(submit(&hub, early_access(&unique_address())).await).await;
    assert_eq!(hub.count("leads").await, i64::from(LEAD_IP_LIMIT) + 1);
    let summary = hub.audited("lead.submit_lock_summary").await;
    assert_eq!(summary.len(), 1, "{summary:?}");
    let (actor, subject, detail) = &summary[0];
    assert_eq!(actor, "anonymous");
    assert_eq!(subject, "lead-form:ip:127.0.0.1");
    assert_eq!(detail["refused"], 1);
    assert_eq!(detail["peer"], "ip:127.0.0.1");
    // Not mistaken for any other throttle's summary.
    assert!(hub.audited("auth.sign_in_lock_summary").await.is_empty());
    assert!(hub.audited("auth.mail_lock_summary").await.is_empty());
}

#[tokio::test]
async fn the_email_send_throttle_refuses_a_sixth_confirmation_to_one_address() {
    let hub = spawn_hub().await;
    let email = unique_address();
    for _ in 0..5 {
        assert_accepted(submit(&hub, early_access(&email)).await).await;
    }
    let resp = submit(&hub, early_access(&email)).await;
    assert_site_cors(resp.headers());
    assert!(resp.headers().contains_key(RETRY_AFTER));
    assert_eq!(
        error_of(resp).await,
        (StatusCode::TOO_MANY_REQUESTS, MAIL_LOCKED.to_string())
    );
    assert_eq!(hub.count("leads").await, 1);
    assert_eq!(hub.count("lead_submissions").await, 5);
    assert_eq!(hub.audited("lead.submitted").await.len(), 5);
}

#[tokio::test]
async fn preflights_get_cors_headers_only_from_the_site_origin() {
    let hub = spawn_hub().await;
    let preflight = |path: &'static str, origin: &'static str| {
        hub.http
            .request(Method::OPTIONS, hub.url(path))
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "content-type, x-csrf-token",
            )
            .send()
    };
    for path in ["/api/leads", "/api/auth/csrf"] {
        let resp = preflight(path, SITE).await.unwrap();
        assert!(resp.status().is_success(), "{path}: {}", resp.status());
        let h = resp.headers();
        assert_site_cors(h);
        assert_eq!(h["access-control-allow-methods"], "GET, POST");
        assert_eq!(
            h["access-control-allow-headers"],
            "content-type, x-csrf-token"
        );
        assert_eq!(h["access-control-max-age"], "600");

        for other in [
            "https://evil.example",
            "http://localhost:4322",
            "http://127.0.0.1:4321",
        ] {
            let resp = preflight(path, other).await.unwrap();
            assert_no_cors(resp.headers());
        }
    }
    // Other paths never get CORS headers, even from the site.
    for path in ["/api/auth/sign-in", "/api/leads/unsubscribe", "/api/health"] {
        let resp = preflight(path, SITE).await.unwrap();
        assert_no_cors(resp.headers());
    }

    // The CSRF token itself: readable by the site, and only by it.
    let get_csrf = |origin: &'static str| {
        hub.http
            .get(hub.url("/api/auth/csrf"))
            .header("origin", origin)
            .send()
    };
    let resp = get_csrf(SITE).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_site_cors(resp.headers());
    let resp = get_csrf("https://evil.example").await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_no_cors(resp.headers());
    // A lead posted from another origin gets no CORS headers either.
    let resp = hub
        .post_req("/api/leads", None)
        .header("origin", "https://evil.example")
        .json(&early_access(&unique_address()))
        .send()
        .await
        .unwrap();
    assert_no_cors(resp.headers());
}

#[tokio::test]
async fn a_lead_without_a_csrf_token_is_403_readable_by_the_site() {
    let hub = spawn_hub().await;
    let resp = hub
        .http
        .post(hub.url("/api/leads"))
        .header("origin", SITE)
        .json(&early_access(&unique_address()))
        .send()
        .await
        .unwrap();
    assert_site_cors(resp.headers());
    assert_eq!(
        error_of(resp).await,
        (
            StatusCode::FORBIDDEN,
            "missing or invalid CSRF token".to_string()
        )
    );
    assert_eq!(hub.count("auth_throttle").await, 0);
    assert_eq!(hub.count("leads").await, 0);
}

#[tokio::test]
async fn unsubscribing_is_idempotent_and_audited_once() {
    let hub = spawn_hub().await;
    let email = unique_address();
    assert_accepted(submit(&hub, early_access(&email)).await).await;
    assert_accepted(submit(&hub, contact(&email)).await).await;
    let first = unsubscribe_tokens(&email, LEAD_EARLY_ACCESS_SUBJECT, 1)
        .await
        .remove(0);
    let second = unsubscribe_tokens(&email, LEAD_CONTACT_SUBJECT, 1)
        .await
        .remove(0);
    assert_ne!(first, second);

    for token in [&first, &first, &second] {
        let resp = unsubscribe(&hub, token).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_no_cors(resp.headers());
        assert_eq!(
            resp.json::<Value>().await.unwrap(),
            json!({"unsubscribed": true})
        );
    }
    let lead = &leads_of(&hub).await[0];
    assert!(lead.unsubscribed);
    assert_eq!(
        hub.audited("lead.unsubscribed").await,
        [(
            "anonymous".to_string(),
            format!("lead:{}", lead.id),
            json!({})
        )]
    );

    let audit_rows = hub.count("audit_log").await;
    for bad in [
        "".to_string(),
        "not-a-token".to_string(),
        "A".repeat(43),
        format!("{first}x"),
    ] {
        assert_eq!(
            error_of(unsubscribe(&hub, &bad).await).await,
            (
                StatusCode::NOT_FOUND,
                leads::MSG_BAD_UNSUBSCRIBE.to_string()
            ),
            "{bad:?}"
        );
    }
    assert_eq!(hub.count("audit_log").await, audit_rows);
}
