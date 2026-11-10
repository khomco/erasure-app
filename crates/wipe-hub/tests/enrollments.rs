//! Station enrollment start and poll (station-enrollment story 1, CAP-1,
//! ADR-0008 §1): every row of the story's I/O matrix through real HTTP, plus
//! the throttle, the purge, the IPv6 /64 key and the stored codes. Approval
//! is story 2's; here an enrollment is approved by registering the station
//! through `POST /api/stations` (`stations::register`) and writing the
//! enrollment row in SQL. Needs `docker compose up -d`; fails, never skips,
//! when it is down.

mod support;

use std::net::SocketAddr;

use reqwest::header::RETRY_AFTER;
use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::hub::{error_of, spawn_hub_with_root, Hub, Who, BASE, PEER};
use support::TestDb;
use uuid::Uuid;
use wipe_cert::SigningKey;
use wipe_hub::enrollments::{
    normalize_user_code, MSG_BAD_PUBLIC_KEY, MSG_BAD_SIGNATURE, MSG_KEY_ID_MISMATCH,
    MSG_START_LOCKED, MSG_STATION_ID_REQUIRED, USER_CODE_ALPHABET,
};
use wipe_hub::throttle::ENROLL_IP_LIMIT;
use wipe_hub::{AppState, Args, Config};
use wipe_license::enrollment::{sign_poll, sign_start, start_message, StartRequest};
use wipe_license::{check_installable, parse_license, VendorRoot};

const START: &str = "/api/enrollments";
const POLL: &str = "/api/enrollments/poll";

struct Fixture {
    hub: Hub,
    root: VendorRoot,
    owner: Who,
    org_id: Uuid,
    /// Annual, one machine seat.
    machine: String,
}

async fn org_of(hub: &Hub, who: &Who) -> Uuid {
    sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1::uuid")
        .bind(&who.id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
}

async fn ok_json(resp: Response, want: StatusCode) -> Value {
    let code = resp.status();
    let v: Value = resp.json().await.unwrap();
    assert_eq!(code, want, "{v}");
    v
}

/// `who` buys `offering` and pays on the Fake checkout page: the new
/// entitlement's id.
async fn buy(hub: &Hub, who: &Who, offering: &str) -> String {
    let v = ok_json(
        hub.post(
            "/api/checkout",
            json!({ "offering_id": offering }),
            Some(&who.cookie),
        )
        .await,
        StatusCode::CREATED,
    )
    .await;
    let order = v["order"]["id"].as_str().unwrap().to_string();
    let url = url::Url::parse(v["checkout_url"].as_str().unwrap()).unwrap();
    let session = url
        .query_pairs()
        .find(|(k, _)| k == "session")
        .unwrap()
        .1
        .into_owned();
    ok_json(
        hub.post_empty(
            &format!("/api/payments/fake/sessions/{session}/pay"),
            Some(&who.cookie),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM entitlements WHERE order_id = $1::uuid")
        .bind(&order)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    id.to_string()
}

async fn fixture() -> Fixture {
    let (hub, root) = spawn_hub_with_root().await;
    let owner = hub.verified_owner("Acme Recyclers").await;
    let org_id = org_of(&hub, &owner).await;
    let annual = hub.published_offering("Station — annual, 1 seat").await;
    let machine = buy(&hub, &owner, &annual).await;
    Fixture {
        hub,
        root,
        owner,
        org_id,
        machine,
    }
}

/// A plain POST: no cookie, no CSRF header, as a station sends it. Every
/// response, success or error, must forbid caching (RFC 6749 §5.1): it may
/// carry a device code or a licence.
async fn post(hub: &Hub, path: &str, body: &Value) -> Response {
    let resp = hub
        .http
        .post(hub.url(path))
        .json(body)
        .send()
        .await
        .unwrap();
    assert_no_store(&resp);
    resp
}

fn assert_no_store(resp: &Response) {
    let h = resp.headers();
    assert_eq!(h["cache-control"], "no-store", "{} {h:?}", resp.status());
    assert_eq!(h["pragma"], "no-cache", "{} {h:?}", resp.status());
}

async fn start_raw(hub: &Hub, req: &StartRequest) -> Response {
    post(hub, START, &serde_json::to_value(req).unwrap()).await
}

/// `key` starts enrolling as `bench-1`: the 200 body.
async fn start(hub: &Hub, key: &SigningKey) -> Value {
    ok_json(
        start_raw(hub, &sign_start(key, "bench-1", Some("Bench 1"))).await,
        StatusCode::OK,
    )
    .await
}

async fn poll(hub: &Hub, key: &SigningKey, device_code: &str) -> Response {
    post(
        hub,
        POLL,
        &serde_json::to_value(sign_poll(key, device_code)).unwrap(),
    )
    .await
}

/// The poll's 400 error code.
async fn poll_error(hub: &Hub, key: &SigningKey, device_code: &str) -> String {
    let (code, error) = error_of(poll(hub, key, device_code).await).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{error}");
    error
}

fn hash(device_code: &str) -> Vec<u8> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    Sha256::digest(URL_SAFE_NO_PAD.decode(device_code).unwrap()).to_vec()
}

async fn enrollment_id(hub: &Hub, device_code: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM enrollments WHERE device_code_hash = $1")
        .bind(hash(device_code))
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
}

async fn status_of(hub: &Hub, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM enrollments WHERE id = $1")
        .bind(id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
}

async fn sql(hub: &Hub, query: &str, id: Uuid) {
    sqlx::query(query)
        .bind(id)
        .execute(&hub.db.pool)
        .await
        .unwrap();
}

/// Let the next poll through the interval check.
async fn backdate_poll(hub: &Hub, id: Uuid) {
    sql(
        hub,
        "UPDATE enrollments SET last_polled_at = now() - interval '1 minute' WHERE id = $1",
        id,
    )
    .await;
}

impl Fixture {
    /// Approve the enrollment `id` for `key` as story 2 will: register the
    /// station on the machine entitlement, then mark the enrollment
    /// approved. The registered station as `GET /api/stations` shows it.
    async fn approve(&self, id: Uuid, key: &SigningKey) -> Value {
        let v = ok_json(
            self.hub
                .post(
                    "/api/stations",
                    json!({
                        "entitlement_id": self.machine,
                        "instance_key_id": key.public_key_id(),
                        "station_id": "bench-1",
                        "label": "Bench 1",
                    }),
                    Some(&self.owner.cookie),
                )
                .await,
            StatusCode::CREATED,
        )
        .await;
        let station: Uuid = v["station"]["id"].as_str().unwrap().parse().unwrap();
        let licence: Uuid = v["licence"]["id"].as_str().unwrap().parse().unwrap();
        sqlx::query(
            "UPDATE enrollments SET status = 'approved', approved_by = $2::uuid, \
                    approved_at = now(), organization_id = $3, registered_station = $4, \
                    licence_id = $5 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(&self.owner.id)
        .bind(self.org_id)
        .bind(station)
        .bind(licence)
        .execute(&self.hub.db.pool)
        .await
        .unwrap();
        let listed = ok_json(
            self.hub
                .get("/api/stations", Some(&self.owner.cookie))
                .await,
            StatusCode::OK,
        )
        .await;
        let list = listed
            .as_array()
            .or_else(|| listed["stations"].as_array())
            .expect("a station list")
            .clone();
        list.into_iter()
            .find(|s| s["id"] == json!(station.to_string()))
            .expect("the registered station is listed")
    }
}

// ---------------------------------------------------------------------------
// Start

#[tokio::test]
async fn a_signed_start_answers_codes_and_stores_one_pending_row() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let v = start(&f.hub, &key).await;

    let mut keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "device_code",
            "expires_in",
            "interval",
            "user_code",
            "verification_uri",
            "verification_uri_complete"
        ]
    );
    assert_eq!(v["expires_in"], 600);
    assert_eq!(v["interval"], 5);
    let user_code = v["user_code"].as_str().unwrap();
    assert_eq!(user_code.len(), 9);
    assert_eq!(&user_code[4..5], "-");
    let undashed = user_code.replace('-', "");
    assert!(undashed.bytes().all(|b| USER_CODE_ALPHABET.contains(&b)));
    assert_eq!(v["verification_uri"], format!("{BASE}/activate"));
    assert_eq!(
        v["verification_uri_complete"],
        format!("{BASE}/activate?code={user_code}")
    );
    let device_code = v["device_code"].as_str().unwrap();
    assert_eq!(device_code.len(), 43);

    let row: (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        i32,
        f64,
    ) = sqlx::query_as(
        "SELECT instance_key_id, public_key_b64, station_id, label, requested_from, \
                    user_code, status, poll_interval_secs, \
                    extract(epoch FROM expires_at - created_at)::float8 \
             FROM enrollments WHERE device_code_hash = $1",
    )
    .bind(hash(device_code))
    .fetch_one(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(row.0, key.public_key_id());
    assert_eq!(row.1, key.verifying_key().to_base64());
    assert_eq!(row.2, "bench-1");
    assert_eq!(row.3.as_deref(), Some("Bench 1"));
    assert_eq!(row.4.as_deref(), Some("127.0.0.1"));
    assert_eq!(row.5, undashed);
    assert_eq!(row.6, "pending");
    assert_eq!(row.7, 5);
    assert_eq!(row.8, 600.0);
    assert_eq!(f.hub.count("enrollments").await, 1);

    let id = enrollment_id(&f.hub, device_code).await;
    let audited = f.hub.audited("enrollment.requested").await;
    assert_eq!(audited.len(), 1);
    let (actor, subject, detail) = &audited[0];
    assert_eq!(actor, &format!("instance-key:{}", key.public_key_id()));
    assert_eq!(subject, &format!("enrollment:{id}"));
    assert_eq!(
        detail,
        &json!({
            "instance_key_id": key.public_key_id(),
            "station_id": "bench-1",
            "label": "Bench 1",
            "replaced": null,
            "peer": PEER,
        })
    );
}

#[tokio::test]
async fn station_id_and_label_are_trimmed_after_the_signature_is_checked() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let v = ok_json(
        start_raw(&f.hub, &sign_start(&key, "  bench-1 ", Some("   "))).await,
        StatusCode::OK,
    )
    .await;
    let (station_id, label): (String, Option<String>) =
        sqlx::query_as("SELECT station_id, label FROM enrollments WHERE device_code_hash = $1")
            .bind(hash(v["device_code"].as_str().unwrap()))
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(station_id, "bench-1");
    assert_eq!(label, None);
}

#[tokio::test]
async fn refused_starts_store_audit_and_charge_nothing() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let other = SigningKey::generate();
    let good = sign_start(&key, "bench-1", None);
    let long = "x".repeat(201);

    let mut mismatch = good.clone();
    mismatch.instance_key_id = other.public_key_id();
    let mut bad_sig = good.clone();
    bad_sig.station_id = "bench-2".into();
    let mut other_signer = good.clone();
    other_signer.signature_b64 = sign_start(&other, "bench-1", None).signature_b64;
    let mut bad_key = good.clone();
    bad_key.public_key_b64 = "not a key".into();
    let mut bad_b64_sig = good.clone();
    bad_b64_sig.signature_b64 = "!!".into();

    let cases: Vec<(StartRequest, String)> = vec![
        (mismatch, MSG_KEY_ID_MISMATCH.into()),
        (bad_sig, MSG_BAD_SIGNATURE.into()),
        (other_signer, MSG_BAD_SIGNATURE.into()),
        (bad_b64_sig, MSG_BAD_SIGNATURE.into()),
        (bad_key, MSG_BAD_PUBLIC_KEY.into()),
        (
            sign_start(&key, "   ", None),
            MSG_STATION_ID_REQUIRED.into(),
        ),
        (sign_start(&key, "", None), MSG_STATION_ID_REQUIRED.into()),
        (
            sign_start(&key, &long, None),
            "station_id must be at most 200 characters".into(),
        ),
        (
            sign_start(&key, "bench-1", Some(&long)),
            "label must be at most 200 characters".into(),
        ),
    ];
    for (req, message) in cases {
        assert_eq!(
            error_of(start_raw(&f.hub, &req).await).await,
            (StatusCode::BAD_REQUEST, message)
        );
    }
    // A body that is not a start request at all.
    let resp = post(&f.hub, START, &json!({ "station_id": "bench-1" })).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    assert_eq!(f.hub.count("enrollments").await, 0);
    assert!(f.hub.audited("enrollment.requested").await.is_empty());
    let charged: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_throttle WHERE key LIKE 'enroll:%'")
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(charged, 0);
}

#[tokio::test]
async fn a_new_start_replaces_the_keys_pending_enrollment() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let first = start(&f.hub, &key).await;
    let first_code = first["device_code"].as_str().unwrap();
    let first_id = enrollment_id(&f.hub, first_code).await;
    // Another key's enrollment is untouched.
    let bystander = SigningKey::generate();
    start(&f.hub, &bystander).await;

    let second = start(&f.hub, &key).await;
    let second_code = second["device_code"].as_str().unwrap();
    assert_ne!(first_code, second_code);
    assert_eq!(f.hub.count("enrollments").await, 2);
    let gone: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollments WHERE id = $1")
        .bind(first_id)
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(gone, 0);

    assert_eq!(poll_error(&f.hub, &key, first_code).await, "invalid_grant");
    assert_eq!(
        poll_error(&f.hub, &key, second_code).await,
        "authorization_pending"
    );
    let audited = f.hub.audited("enrollment.requested").await;
    assert_eq!(audited.len(), 3);
    assert_eq!(audited[0].2["replaced"], Value::Null);
    assert_eq!(audited[2].2["replaced"], json!(first_id.to_string()));
}

#[tokio::test]
async fn the_twenty_first_start_from_one_address_in_an_hour_is_locked() {
    let f = fixture().await;
    for _ in 0..ENROLL_IP_LIMIT {
        start(&f.hub, &SigningKey::generate()).await;
    }
    let resp = start_raw(&f.hub, &sign_start(&SigningKey::generate(), "b", None)).await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry: u64 = resp.headers()[RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((3500..=3600).contains(&retry), "{retry}");
    assert_eq!(
        error_of(resp).await,
        (StatusCode::TOO_MANY_REQUESTS, MSG_START_LOCKED.to_string())
    );
    assert_eq!(f.hub.count("enrollments").await, i64::from(ENROLL_IP_LIMIT));

    let locked = f.hub.audited("enrollment.start_locked").await;
    assert_eq!(locked.len(), 1);
    assert_eq!(locked[0].0, "anonymous");
    assert_eq!(locked[0].1, format!("enroll:{PEER}"));
    assert!(locked[0].2["until"].is_string());

    // Once the lock has elapsed, the next start goes through and the
    // refusals are summarised once, under the enrollment action.
    sqlx::query(
        "UPDATE auth_throttle SET locked_until = now() - interval '1 second' WHERE key = $1",
    )
    .bind(format!("enroll:{PEER}"))
    .execute(&f.hub.db.pool)
    .await
    .unwrap();
    start(&f.hub, &SigningKey::generate()).await;
    let summary = f.hub.audited("enrollment.start_lock_summary").await;
    assert_eq!(summary.len(), 1, "{summary:?}");
    assert_eq!(summary[0].0, "anonymous");
    assert_eq!(summary[0].1, format!("enroll:{PEER}"));
    assert_eq!(summary[0].2["refused"], 1);
    assert_eq!(summary[0].2["peer"], PEER);
    assert!(f.hub.audited("auth.sign_in_lock_summary").await.is_empty());
    assert_eq!(f.hub.audited("enrollment.start_locked").await.len(), 1);
}

fn hub_state(db: &TestDb) -> (AppState, tempfile::TempDir) {
    let artifacts = tempfile::tempdir().unwrap();
    let args = Args {
        addr: "127.0.0.1:0".parse().unwrap(),
        database_url: db.url.clone(),
        smtp_host: wipe_hub::config::DEFAULT_SMTP_HOST.into(),
        smtp_port: wipe_hub::config::DEFAULT_SMTP_PORT,
        mail_from: wipe_hub::config::DEFAULT_MAIL_FROM.into(),
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
    let mut config = Config::from_args(args).unwrap();
    config.portal_dir = None;
    (AppState::new(config).unwrap(), artifacts)
}

#[tokio::test]
async fn ipv6_starts_are_throttled_by_their_slash_64() {
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use tower::ServiceExt;

    let db = TestDb::migrated().await;
    let (state, _artifacts) = hub_state(&db);
    let app = wipe_hub::router(state);
    let attempt = |peer: &str| {
        let peer: SocketAddr = peer.parse().unwrap();
        let body = serde_json::to_string(&sign_start(&SigningKey::generate(), "b", None)).unwrap();
        // No CSRF cookie or header: the route is exempt.
        let mut req = axum::http::Request::post(START)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(peer));
        app.clone().oneshot(req)
    };
    for i in 1..=ENROLL_IP_LIMIT {
        let resp = attempt(&format!("[2001:db8:1:2::{i:x}]:5000"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "start {i}");
    }
    let resp = attempt("[2001:db8:1:2:ffff:1:2:3]:5000").await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(resp.headers().get(RETRY_AFTER).is_some());
    // Another /64 is unaffected.
    let resp = attempt("[2001:db8:1:3::1]:5000").await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let failures: i32 = sqlx::query_scalar(
        "SELECT failures FROM auth_throttle WHERE key = 'enroll:ip:2001:db8:1:2::/64'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(failures, ENROLL_IP_LIMIT);
    // The row keeps the full address; the audit keeps the throttle key.
    let (from, peer): (String, Value) = sqlx::query_as(
        "SELECT e.requested_from, a.detail->'peer' FROM enrollments e \
         JOIN audit_log a ON a.subject = 'enrollment:' || e.id \
         WHERE e.requested_from = '2001:db8:1:2::1'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(from, "2001:db8:1:2::1");
    assert_eq!(peer, json!("ip:2001:db8:1:2::/64"));
}

#[tokio::test]
async fn each_start_purges_enrollments_expired_over_a_day_ago() {
    let f = fixture().await;
    let old = SigningKey::generate();
    let recent = SigningKey::generate();
    let collected = SigningKey::generate();
    let old_id = enrollment_id(
        &f.hub,
        start(&f.hub, &old).await["device_code"].as_str().unwrap(),
    )
    .await;
    let recent_id = enrollment_id(
        &f.hub,
        start(&f.hub, &recent).await["device_code"]
            .as_str()
            .unwrap(),
    )
    .await;
    let collected_code = start(&f.hub, &collected).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let collected_id = enrollment_id(&f.hub, &collected_code).await;
    f.approve(collected_id, &collected).await;
    assert_eq!(
        poll(&f.hub, &collected, &collected_code).await.status(),
        StatusCode::OK
    );

    for (id, ago) in [
        (old_id, "25 hours"),
        (recent_id, "23 hours"),
        (collected_id, "25 hours"),
    ] {
        sqlx::query(&format!(
            "UPDATE enrollments SET expires_at = now() - interval '{ago}' WHERE id = $1"
        ))
        .bind(id)
        .execute(&f.hub.db.pool)
        .await
        .unwrap();
    }
    // A late poll inside the day still gets the right answer.
    backdate_poll(&f.hub, recent_id).await;
    assert_eq!(f.hub.count("enrollments").await, 3);

    start(&f.hub, &SigningKey::generate()).await;
    let left: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM enrollments ORDER BY created_at")
        .fetch_all(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(left.len(), 2);
    assert!(left.contains(&recent_id));
    assert!(!left.contains(&old_id));
    assert!(!left.contains(&collected_id));
    // The purge audits nothing.
    assert_eq!(f.hub.audited("enrollment.requested").await.len(), 4);
}

// ---------------------------------------------------------------------------
// Poll

#[tokio::test]
async fn polling_is_pending_then_slow_down_when_too_fast() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let code = start(&f.hub, &key).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let id = enrollment_id(&f.hub, &code).await;
    assert_eq!(
        poll_error(&f.hub, &key, &code).await,
        "authorization_pending"
    );

    let resp = poll(&f.hub, &key, &code).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body, json!({ "error": "slow_down", "interval": 10 }));
    let interval: i32 =
        sqlx::query_scalar("SELECT poll_interval_secs FROM enrollments WHERE id = $1")
            .bind(id)
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(interval, 10);

    // Six seconds later is still too soon for the new interval.
    sql(
        &f.hub,
        "UPDATE enrollments SET last_polled_at = now() - interval '6 seconds' WHERE id = $1",
        id,
    )
    .await;
    let body: Value = poll(&f.hub, &key, &code).await.json().await.unwrap();
    assert_eq!(body["interval"], 15);

    backdate_poll(&f.hub, id).await;
    assert_eq!(
        poll_error(&f.hub, &key, &code).await,
        "authorization_pending"
    );
    assert_eq!(status_of(&f.hub, id).await, "pending");
    // Polls are not audited.
    assert_eq!(
        f.hub
            .actions()
            .await
            .iter()
            .filter(|a| a.starts_with("enrollment."))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_approved_enrollment_hands_over_station_and_licence_once() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let code = start(&f.hub, &key).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let id = enrollment_id(&f.hub, &code).await;
    assert_eq!(
        poll_error(&f.hub, &key, &code).await,
        "authorization_pending"
    );
    let listed = f.approve(id, &key).await;
    backdate_poll(&f.hub, id).await;

    let v = ok_json(poll(&f.hub, &key, &code).await, StatusCode::OK).await;
    let mut keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["licence", "station"]);
    assert_eq!(v["station"], listed);
    assert_eq!(status_of(&f.hub, id).await, "collected");

    // The licence is the stored file, and it installs on this key against
    // the vendor root the station already trusts.
    let licence = parse_license(&v["licence"].to_string()).unwrap();
    check_installable(&licence, &key.public_key_id(), &[f.root.verifying_key()]).unwrap();
    assert_eq!(licence.body.instance_public_key_id, key.public_key_id());
    assert_eq!(
        licence.body.license_id,
        v["station"]["licence"]["id"].as_str().unwrap()
    );
    let stored: String = sqlx::query_scalar("SELECT document FROM licences WHERE id = $1::uuid")
        .bind(&licence.body.license_id)
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&stored).unwrap(),
        v["licence"]
    );
    // Another root does not trust it.
    assert!(check_installable(
        &licence,
        &key.public_key_id(),
        &[VendorRoot::generate().verifying_key()]
    )
    .is_err());

    // Collected: the code is spent, however patiently it is polled again.
    backdate_poll(&f.hub, id).await;
    assert_eq!(poll_error(&f.hub, &key, &code).await, "expired_token");
    assert_eq!(poll_error(&f.hub, &key, &code).await, "expired_token");
    // Neither poll nor collection is audited.
    assert_eq!(f.hub.audited("enrollment.requested").await.len(), 1);
    assert!(f
        .hub
        .actions()
        .await
        .iter()
        .all(|a| !a.starts_with("enrollment.") || a == "enrollment.requested"));
}

#[tokio::test]
async fn expired_and_denied_enrollments_say_so() {
    let f = fixture().await;
    // Pending, expired.
    let a = SigningKey::generate();
    let a_code = start(&f.hub, &a).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let a_id = enrollment_id(&f.hub, &a_code).await;
    sql(
        &f.hub,
        "UPDATE enrollments SET expires_at = now() - interval '1 second' WHERE id = $1",
        a_id,
    )
    .await;
    assert_eq!(poll_error(&f.hub, &a, &a_code).await, "expired_token");

    // Approved but expired before collection.
    let b = SigningKey::generate();
    let b_code = start(&f.hub, &b).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let b_id = enrollment_id(&f.hub, &b_code).await;
    f.approve(b_id, &b).await;
    sql(
        &f.hub,
        "UPDATE enrollments SET expires_at = now() - interval '1 second' WHERE id = $1",
        b_id,
    )
    .await;
    assert_eq!(poll_error(&f.hub, &b, &b_code).await, "expired_token");
    assert_eq!(status_of(&f.hub, b_id).await, "approved");

    // Denied.
    let c = SigningKey::generate();
    let c_code = start(&f.hub, &c).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let c_id = enrollment_id(&f.hub, &c_code).await;
    sql(
        &f.hub,
        "UPDATE enrollments SET status = 'denied' WHERE id = $1",
        c_id,
    )
    .await;
    assert_eq!(poll_error(&f.hub, &c, &c_code).await, "access_denied");
    assert_eq!(poll_error(&f.hub, &c, &c_code).await, "access_denied");
}

#[tokio::test]
async fn a_poll_by_another_key_or_with_a_bad_code_is_invalid_grant() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let other = SigningKey::generate();
    let code = start(&f.hub, &key).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let id = enrollment_id(&f.hub, &code).await;

    assert_eq!(poll_error(&f.hub, &other, &code).await, "invalid_grant");
    // A wrong signer leaves no trace: the station's own first poll is not
    // slowed down.
    let polled: Option<String> =
        sqlx::query_scalar("SELECT last_polled_at::text FROM enrollments WHERE id = $1")
            .bind(id)
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(polled, None);

    // A signature over another code.
    let mut crossed = sign_poll(&key, "something else");
    crossed.device_code = code.clone();
    let (status, error) =
        error_of(post(&f.hub, POLL, &serde_json::to_value(crossed).unwrap()).await).await;
    assert_eq!(
        (status, error.as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant")
    );

    // Unknown, malformed and missing codes.
    let unknown = wipe_hub::csrf::generate_token();
    for bad in [unknown.as_str(), "short", "", "!!!!"] {
        assert_eq!(
            poll_error(&f.hub, &key, bad).await,
            "invalid_grant",
            "{bad:?}"
        );
    }
    let (status, error) = error_of(post(&f.hub, POLL, &json!({})).await).await;
    assert_eq!(
        (status, error.as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant")
    );

    assert_eq!(
        poll_error(&f.hub, &key, &code).await,
        "authorization_pending"
    );
}

// ---------------------------------------------------------------------------
// Stored codes

#[tokio::test]
async fn no_column_or_audit_row_holds_the_raw_device_code() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let code = start(&f.hub, &key).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let id = enrollment_id(&f.hub, &code).await;
    assert_eq!(
        poll_error(&f.hub, &key, &code).await,
        "authorization_pending"
    );
    poll(&f.hub, &key, &code).await;
    f.approve(id, &key).await;
    backdate_poll(&f.hub, id).await;
    assert_eq!(poll(&f.hub, &key, &code).await.status(), StatusCode::OK);

    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT row_to_json(e)::text FROM enrollments e \
         UNION ALL SELECT row_to_json(a)::text FROM audit_log a \
         UNION ALL SELECT row_to_json(t)::text FROM auth_throttle t",
    )
    .fetch_all(&f.hub.db.pool)
    .await
    .unwrap();
    assert!(!rows.is_empty());
    let hex_code = {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        hex::encode(URL_SAFE_NO_PAD.decode(&code).unwrap())
    };
    for row in &rows {
        assert!(!row.contains(&code), "{row}");
        assert!(!row.contains(&hex_code), "{row}");
    }
    // Only the hash is kept.
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT device_code_hash FROM enrollments WHERE id = $1")
            .bind(id)
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(stored, hash(&code));
}

#[test]
fn owners_may_type_the_user_code_any_way() {
    assert_eq!(
        normalize_user_code("bcdf-ghjk").as_deref(),
        Some("BCDFGHJK")
    );
    assert_eq!(normalize_user_code("BCDFGHJK").as_deref(), Some("BCDFGHJK"));
    assert_eq!(
        normalize_user_code(" Bcdf-Ghjk ").as_deref(),
        Some("BCDFGHJK")
    );
    assert_eq!(normalize_user_code("ABCD-EFGH"), None);
    assert_eq!(normalize_user_code("BCDF-GHJ"), None);
}

#[test]
fn the_start_message_is_what_the_hub_verifies() {
    // A station signing by hand (story 3) signs exactly these bytes.
    let key = SigningKey::generate();
    let req = sign_start(&key, "bench-1", Some("Bench 1"));
    let msg = start_message(
        &req.instance_key_id,
        &req.public_key_b64,
        &req.station_id,
        req.label.as_deref(),
    );
    assert!(msg.starts_with(b"wipestation-enrollment-start-v1\n{"));
}

#[tokio::test]
async fn the_schema_refuses_impossible_enrollment_changes() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let code = start(&f.hub, &key).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let id = enrollment_id(&f.hub, &code).await;
    for refused in [
        // Skipping approval, or approving without naming what was approved.
        "UPDATE enrollments SET status = 'collected' WHERE id = $1",
        "UPDATE enrollments SET status = 'approved' WHERE id = $1",
        // The request itself never changes.
        "UPDATE enrollments SET user_code = 'BCDFBCDF' WHERE id = $1",
        "UPDATE enrollments SET station_id = 'bench-9' WHERE id = $1",
        "UPDATE enrollments SET device_code_hash = sha256('x') WHERE id = $1",
        // The CHECK (not the trigger) refuses an interval under 5 s.
        "UPDATE enrollments SET poll_interval_secs = 1 WHERE id = $1",
    ] {
        assert!(
            sqlx::query(refused)
                .bind(id)
                .execute(&f.hub.db.pool)
                .await
                .is_err(),
            "{refused}"
        );
    }
    sql(
        &f.hub,
        "UPDATE enrollments SET status = 'denied' WHERE id = $1",
        id,
    )
    .await;
    assert!(
        sqlx::query("UPDATE enrollments SET status = 'pending' WHERE id = $1")
            .bind(id)
            .execute(&f.hub.db.pool)
            .await
            .is_err()
    );
    assert_eq!(status_of(&f.hub, id).await, "denied");
}

#[tokio::test]
async fn start_and_poll_responses_are_never_cached() {
    let f = fixture().await;
    let key = SigningKey::generate();
    // `post` asserts the headers on each of these: 200 start, 400 start,
    // 400 malformed body, 400 poll error and 200 poll.
    let code = start(&f.hub, &key).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let mut bad = sign_start(&key, "bench-1", None);
    bad.station_id = "x".into();
    assert_eq!(
        start_raw(&f.hub, &bad).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(&f.hub, START, &json!({})).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        poll_error(&f.hub, &key, &code).await,
        "authorization_pending"
    );
    let id = enrollment_id(&f.hub, &code).await;
    f.approve(id, &key).await;
    backdate_poll(&f.hub, id).await;
    assert_eq!(poll(&f.hub, &key, &code).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_station_released_after_approval_gets_expired_token_not_its_licence() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let code = start(&f.hub, &key).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let id = enrollment_id(&f.hub, &code).await;
    let station = f.approve(id, &key).await;
    ok_json(
        f.hub
            .post_empty(
                &format!("/api/stations/{}/release", station["id"].as_str().unwrap()),
                Some(&f.owner.cookie),
            )
            .await,
        StatusCode::OK,
    )
    .await;
    backdate_poll(&f.hub, id).await;
    assert_eq!(poll_error(&f.hub, &key, &code).await, "expired_token");
    assert_eq!(status_of(&f.hub, id).await, "approved");
}

#[tokio::test]
async fn concurrent_starts_for_one_key_leave_one_pending_enrollment() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let a = sign_start(&key, "bench-1", None);
    let b = sign_start(&key, "bench-1", Some("Bench 1"));
    let (ra, rb) = tokio::join!(start_raw(&f.hub, &a), start_raw(&f.hub, &b));
    let va = ok_json(ra, StatusCode::OK).await;
    let vb = ok_json(rb, StatusCode::OK).await;
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM enrollments WHERE instance_key_id = $1 AND status = 'pending'",
    )
    .bind(key.public_key_id())
    .fetch_one(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(pending, 1);
    let codes = [
        va["device_code"].as_str().unwrap().to_string(),
        vb["device_code"].as_str().unwrap().to_string(),
    ];
    let mut answers = Vec::new();
    for code in &codes {
        answers.push(poll_error(&f.hub, &key, code).await);
    }
    answers.sort();
    assert_eq!(answers, ["authorization_pending", "invalid_grant"]);
}

#[tokio::test]
async fn concurrent_polls_of_an_approved_enrollment_collect_once() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let code = start(&f.hub, &key).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let id = enrollment_id(&f.hub, &code).await;
    f.approve(id, &key).await;
    backdate_poll(&f.hub, id).await;
    let (a, b) = tokio::join!(poll(&f.hub, &key, &code), poll(&f.hub, &key, &code));
    let mut statuses = [a.status(), b.status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::BAD_REQUEST]);
    for resp in [a, b] {
        if resp.status() == StatusCode::BAD_REQUEST {
            let (_, error) = error_of(resp).await;
            assert_eq!(error, "expired_token");
        }
    }
    assert_eq!(status_of(&f.hub, id).await, "collected");
}

#[tokio::test]
async fn a_decided_enrollments_approval_cannot_change() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let code = start(&f.hub, &key).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let id = enrollment_id(&f.hub, &code).await;
    f.approve(id, &key).await;
    let changes = [
        "UPDATE enrollments SET licence_id = gen_random_uuid() WHERE id = $1",
        "UPDATE enrollments SET organization_id = gen_random_uuid() WHERE id = $1",
        "UPDATE enrollments SET approved_by = gen_random_uuid() WHERE id = $1",
    ];
    let refused_by_trigger = |q: &'static str| {
        let pool = f.hub.db.pool.clone();
        async move {
            let err = sqlx::query(q).bind(id).execute(&pool).await.unwrap_err();
            assert!(
                err.to_string()
                    .contains("a decided enrollment's approval cannot change"),
                "{q}: {err}"
            );
        }
    };
    for q in changes {
        refused_by_trigger(q).await;
    }
    backdate_poll(&f.hub, id).await;
    assert_eq!(poll(&f.hub, &key, &code).await.status(), StatusCode::OK);
    assert_eq!(status_of(&f.hub, id).await, "collected");
    for q in changes {
        refused_by_trigger(q).await;
    }
}

#[tokio::test]
async fn a_denied_enrollment_past_its_expiry_is_still_access_denied() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let code = start(&f.hub, &key).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let id = enrollment_id(&f.hub, &code).await;
    sql(
        &f.hub,
        "UPDATE enrollments SET status = 'denied', expires_at = now() - interval '1 minute' \
         WHERE id = $1",
        id,
    )
    .await;
    assert_eq!(poll_error(&f.hub, &key, &code).await, "access_denied");
}

#[tokio::test]
async fn an_approved_enrollment_polled_too_soon_is_slowed_down_and_stays_approved() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let code = start(&f.hub, &key).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let id = enrollment_id(&f.hub, &code).await;
    assert_eq!(
        poll_error(&f.hub, &key, &code).await,
        "authorization_pending"
    );
    f.approve(id, &key).await;
    // Polled again within the 5 s interval.
    let resp = poll(&f.hub, &key, &code).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body, json!({ "error": "slow_down", "interval": 10 }));
    assert_eq!(status_of(&f.hub, id).await, "approved");
}
