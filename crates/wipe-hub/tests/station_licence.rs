//! A connected station fetches its own licence (station-enrollment story 5,
//! CAP-5, ADR-0008 §2, §3): every hub row of the story's I/O matrix through
//! real HTTP, signed exactly as the station signs
//! (`wipe_license::station_request`), plus the challenge throttle and purge,
//! concurrent fetches during a renewal, and the audit rows. Stations are
//! registered through `POST /api/stations` (a file-registered station: no
//! enrollment row), so the hub authenticates them from the public key in the
//! body alone. Needs `docker compose up -d`; fails, never skips, when it is
//! down.
//!
//! A line break in a signature header is refused by
//! `station_request::verify` (unit-tested there); no HTTP client sends one,
//! since header values cannot carry it.

mod support;

use reqwest::header::RETRY_AFTER;
use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{error_of, spawn_hub_with_root, Hub, Who, BASE};
use uuid::Uuid;
use wipe_cert::SigningKey;
use wipe_hub::station_link_routes::{
    MSG_AMBIGUOUS_KEY, MSG_BAD_SIGNATURE, MSG_CHALLENGE_LOCKED, MSG_CHALLENGE_USED,
    MSG_KEY_ID_MISMATCH, MSG_NOT_REGISTERED, MSG_UNSIGNED,
};
use wipe_hub::throttle::{PURGE_BATCH, STATION_CHALLENGE_IP_LIMIT};
use wipe_license::station_request::{sign, HEADER_CHALLENGE, HEADER_KEY_ID, HEADER_SIGNATURE};
use wipe_license::{check_installable, parse_license, VendorRoot};

const CHALLENGE: &str = "/api/station/challenge";
const LICENCE: &str = "/api/station/licence";
const THROTTLE_KEY: &str = "station:ip:127.0.0.1";

struct Fixture {
    hub: Hub,
    root: VendorRoot,
    owner: Who,
    /// Annual, one machine seat.
    machine: String,
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
    let annual = hub.published_offering("Station — annual, 1 seat").await;
    let machine = buy(&hub, &owner, &annual).await;
    Fixture {
        hub,
        root,
        owner,
        machine,
    }
}

/// A registered station: its `stations` row id and first licence id.
struct Registered {
    station: String,
    licence: String,
}

impl Fixture {
    /// `owner` registers `key` on `entitlement` from the portal, as
    /// `bench-1`.
    async fn register_as(&self, owner: &Who, entitlement: &str, key: &SigningKey) -> Registered {
        let v = ok_json(
            self.hub
                .post(
                    "/api/stations",
                    json!({
                        "entitlement_id": entitlement,
                        "instance_key_id": key.public_key_id(),
                        "station_id": "bench-1",
                    }),
                    Some(&owner.cookie),
                )
                .await,
            StatusCode::CREATED,
        )
        .await;
        Registered {
            station: v["station"]["id"].as_str().unwrap().to_string(),
            licence: v["licence"]["id"].as_str().unwrap().to_string(),
        }
    }

    async fn register(&self, key: &SigningKey) -> Registered {
        self.register_as(&self.owner, &self.machine, key).await
    }

    /// The owner pays a Fake renewal of the machine entitlement.
    async fn renew(&self) {
        ok_json(
            self.hub
                .post_empty(
                    &format!("/api/payments/fake/entitlements/{}/renew", self.machine),
                    Some(&self.owner.cookie),
                )
                .await,
            StatusCode::OK,
        )
        .await;
    }

    async fn audit_rows(&self) -> i64 {
        self.hub.count("audit_log").await
    }
}

fn assert_no_store(resp: &Response) {
    let h = resp.headers();
    assert_eq!(h["cache-control"], "no-store", "{} {h:?}", resp.status());
    assert_eq!(h["pragma"], "no-cache", "{} {h:?}", resp.status());
}

/// `GET /api/station/challenge`, as a station sends it: no cookie.
async fn challenge_raw(hub: &Hub) -> Response {
    let resp = hub.http.get(hub.url(CHALLENGE)).send().await.unwrap();
    assert_no_store(&resp);
    resp
}

async fn challenge(hub: &Hub) -> String {
    let v = ok_json(challenge_raw(hub).await, StatusCode::OK).await;
    assert_eq!(v["expires_in"], 60, "{v}");
    v["challenge"].as_str().unwrap().to_string()
}

fn body(key: &SigningKey, held: Option<&str>) -> String {
    json!({
        "instance_public_key": key.verifying_key().to_base64(),
        "licence_id": held,
    })
    .to_string()
}

/// `POST /api/station/licence` with `body` and the three headers as given:
/// no cookie, no CSRF token.
async fn post_signed(hub: &Hub, headers: [(&str, String); 3], body: String) -> Response {
    let mut req = hub
        .http
        .post(hub.url(LICENCE))
        .header("content-type", "application/json");
    for (name, value) in headers {
        req = req.header(name, value);
    }
    let resp = req.body(body).send().await.unwrap();
    assert_no_store(&resp);
    resp
}

/// `body` signed by `key` over `challenge`, for the hub at [`BASE`].
async fn post_with(hub: &Hub, key: &SigningKey, challenge: &str, body: String) -> Response {
    let s = sign(key, challenge, BASE, "POST", LICENCE, body.as_bytes());
    post_signed(
        hub,
        [
            (HEADER_KEY_ID, s.instance_key_id),
            (HEADER_CHALLENGE, s.challenge),
            (HEADER_SIGNATURE, s.signature_b64),
        ],
        body,
    )
    .await
}

/// A fresh challenge, then the signed fetch for `key` holding `held`.
async fn fetch(hub: &Hub, key: &SigningKey, held: Option<&str>) -> Response {
    let c = challenge(hub).await;
    post_with(hub, key, &c, body(key, held)).await
}

/// The licence id of a 200 answer, after checking it installs on `key`'s
/// station against `root`.
async fn fetched_licence(resp: Response, key: &SigningKey, root: &VendorRoot) -> String {
    let v = ok_json(resp, StatusCode::OK).await;
    assert_eq!(v.as_object().unwrap().len(), 1, "{v}");
    let licence = parse_license(&serde_json::to_string(&v["licence"]).unwrap()).unwrap();
    check_installable(&licence, &key.public_key_id(), &[root.verifying_key()]).unwrap();
    licence.body.license_id
}

async fn assert_current(resp: Response) {
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(resp.bytes().await.unwrap().is_empty());
}

async fn challenges(hub: &Hub) -> i64 {
    hub.count("station_challenges").await
}

// ---------------------------------------------------------------------------
// The matrix

#[tokio::test]
async fn a_renewal_is_reissued_by_the_station_and_audited_with_it_as_actor() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let reg = f.register(&key).await;
    f.renew().await;
    let licences = f.hub.count("licences").await;
    let seats_used = f.hub.count("stations").await;

    let new = fetched_licence(fetch(&f.hub, &key, Some(&reg.licence)).await, &key, &f.root).await;
    assert_ne!(new, reg.licence);
    assert_eq!(f.hub.count("licences").await, licences + 1);
    // The station keeps its registration and seat.
    assert_eq!(f.hub.count("stations").await, seats_used);
    let (status, not_after_matches): (String, bool) = sqlx::query_as(
        "SELECT l.status, l.not_after = e.term_end FROM licences l \
         JOIN entitlements e ON e.id = l.entitlement_id WHERE l.id = $1::uuid",
    )
    .bind(&new)
    .fetch_one(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(status, "active");
    assert!(not_after_matches);
    let old: String = sqlx::query_scalar("SELECT status FROM licences WHERE id = $1::uuid")
        .bind(&reg.licence)
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(old, "superseded");

    let actor = format!("station:{}", reg.station);
    let superseded = f.hub.audited("licence.superseded").await;
    assert_eq!(
        superseded,
        [(
            actor.clone(),
            format!("licence:{}", reg.licence),
            json!({
                "station_id": reg.station,
                "instance_key_id": key.public_key_id(),
                "reason": "renewed",
                "superseded_by": new,
            }),
        )]
    );
    let issued = f.hub.audited("licence.issued").await;
    assert_eq!(issued.len(), 2);
    let (who, subject, detail) = &issued[1];
    assert_eq!(who, &actor);
    assert_eq!(subject, &format!("licence:{new}"));
    assert_eq!(detail["reissue_of"], json!(reg.licence));
    assert_eq!(detail["instance_key_id"], json!(key.public_key_id()));
    assert_eq!(detail["station_id"], json!(reg.station));

    // The station now holds it: nothing new, nothing audited.
    let rows = f.audit_rows().await;
    assert_current(fetch(&f.hub, &key, Some(&new)).await).await;
    assert_eq!(f.audit_rows().await, rows);
}

#[tokio::test]
async fn an_owner_reissued_licence_is_delivered_without_another_reissue() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let reg = f.register(&key).await;
    f.renew().await;
    let v = ok_json(
        f.hub
            .post_empty(
                &format!("/api/stations/{}/reissue", reg.station),
                Some(&f.owner.cookie),
            )
            .await,
        StatusCode::CREATED,
    )
    .await;
    let reissued = v["licence"]["id"].as_str().unwrap().to_string();
    let (licences, rows) = (f.hub.count("licences").await, f.audit_rows().await);

    // The station still holds the old one.
    let got = fetched_licence(fetch(&f.hub, &key, Some(&reg.licence)).await, &key, &f.root).await;
    assert_eq!(got, reissued);
    assert_eq!(f.hub.count("licences").await, licences);
    assert_eq!(f.audit_rows().await, rows);
}

#[tokio::test]
async fn a_current_station_gets_204_and_nothing_is_audited() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let reg = f.register(&key).await;
    let rows = f.audit_rows().await;
    assert_current(fetch(&f.hub, &key, Some(&reg.licence)).await).await;
    assert_eq!(f.audit_rows().await, rows);
}

#[tokio::test]
async fn a_file_registered_station_with_no_licence_gets_its_latest_one() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let reg = f.register(&key).await;
    // Registered from the portal: the hub has no enrollment and no public
    // key for it, only the key id. The key in the body is the proof.
    assert_eq!(f.hub.count("enrollments").await, 0);
    let rows = f.audit_rows().await;
    let got = fetched_licence(fetch(&f.hub, &key, None).await, &key, &f.root).await;
    assert_eq!(got, reg.licence);
    // A licence id the hub never issued is just "not the latest".
    let got = fetched_licence(
        fetch(&f.hub, &key, Some("lic-from-elsewhere")).await,
        &key,
        &f.root,
    )
    .await;
    assert_eq!(got, reg.licence);
    assert_eq!(f.audit_rows().await, rows);
}

#[tokio::test]
async fn a_released_or_unknown_station_is_not_registered() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let reg = f.register(&key).await;
    ok_json(
        f.hub
            .post_empty(
                &format!("/api/stations/{}/release", reg.station),
                Some(&f.owner.cookie),
            )
            .await,
        StatusCode::OK,
    )
    .await;
    let rows = f.audit_rows().await;
    assert_eq!(
        error_of(fetch(&f.hub, &key, Some(&reg.licence)).await).await,
        (StatusCode::NOT_FOUND, MSG_NOT_REGISTERED.to_string())
    );
    let stranger = SigningKey::generate();
    assert_eq!(
        error_of(fetch(&f.hub, &stranger, None).await).await,
        (StatusCode::NOT_FOUND, MSG_NOT_REGISTERED.to_string())
    );
    assert_eq!(f.audit_rows().await, rows);
}

#[tokio::test]
async fn a_revoked_latest_licence_returns_nothing_new() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let reg = f.register(&key).await;
    // Revoked as a vendor admin's `stations::revoke` would.
    sqlx::query(
        "UPDATE licences SET status = 'revoked', revoked_at = now(), \
                revocation_reason = 'test' WHERE id = $1::uuid",
    )
    .bind(&reg.licence)
    .execute(&f.hub.db.pool)
    .await
    .unwrap();
    f.renew().await;
    let (licences, rows) = (f.hub.count("licences").await, f.audit_rows().await);
    assert_current(fetch(&f.hub, &key, Some(&reg.licence)).await).await;
    assert_current(fetch(&f.hub, &key, None).await).await;
    // A revoked licence is never reissued or handed out.
    assert_eq!(f.hub.count("licences").await, licences);
    assert_eq!(f.audit_rows().await, rows);
}

#[tokio::test]
async fn a_key_in_two_organizations_is_chosen_by_its_licence_or_refused() {
    let f = fixture().await;
    let other = f.hub.verified_owner("Beta Recyclers").await;
    let annual = f.hub.published_offering("Station — annual, other").await;
    let other_machine = buy(&f.hub, &other, &annual).await;
    let key = SigningKey::generate();
    let a = f.register(&key).await;
    let b = f.register_as(&other, &other_machine, &key).await;

    for held in [None, Some("lic-from-elsewhere")] {
        assert_eq!(
            error_of(fetch(&f.hub, &key, held).await).await,
            (StatusCode::CONFLICT, MSG_AMBIGUOUS_KEY.to_string())
        );
    }
    // Holding either organization's licence picks that organization.
    assert_current(fetch(&f.hub, &key, Some(&a.licence)).await).await;
    assert_current(fetch(&f.hub, &key, Some(&b.licence)).await).await;
    // And its renewal is reissued there, and only there.
    f.renew().await;
    let new = fetched_licence(fetch(&f.hub, &key, Some(&a.licence)).await, &key, &f.root).await;
    let owner: String =
        sqlx::query_scalar("SELECT station_id::text FROM licences WHERE id = $1::uuid")
            .bind(&new)
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(owner, a.station);
    assert_current(fetch(&f.hub, &key, Some(&b.licence)).await).await;
}

// ---------------------------------------------------------------------------
// Challenges and signatures

#[tokio::test]
async fn a_challenge_is_single_use() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let reg = f.register(&key).await;
    let c = challenge(&f.hub).await;
    assert_current(post_with(&f.hub, &key, &c, body(&key, Some(&reg.licence))).await).await;
    assert_eq!(
        error_of(post_with(&f.hub, &key, &c, body(&key, Some(&reg.licence))).await).await,
        (StatusCode::UNAUTHORIZED, MSG_CHALLENGE_USED.to_string())
    );
    // A challenge the hub never issued, likewise.
    let made_up = "A".repeat(43);
    assert_eq!(
        error_of(post_with(&f.hub, &key, &made_up, body(&key, None)).await).await,
        (StatusCode::UNAUTHORIZED, MSG_CHALLENGE_USED.to_string())
    );
}

#[tokio::test]
async fn an_expired_challenge_is_refused() {
    let f = fixture().await;
    let key = SigningKey::generate();
    f.register(&key).await;
    let c = challenge(&f.hub).await;
    sqlx::query(
        "UPDATE station_challenges SET created_at = now() - interval '2 minutes', \
                expires_at = now() - interval '1 minute' WHERE challenge = $1",
    )
    .bind(&c)
    .execute(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(
        error_of(post_with(&f.hub, &key, &c, body(&key, None)).await).await,
        (StatusCode::UNAUTHORIZED, MSG_CHALLENGE_USED.to_string())
    );
}

#[tokio::test]
async fn a_bad_signature_or_key_is_refused_and_consumes_nothing() {
    let f = fixture().await;
    let key = SigningKey::generate();
    f.register(&key).await;
    let c = challenge(&f.hub).await;
    let good = body(&key, None);

    // Signed by another key, sent with this key's id and public key.
    let thief = SigningKey::generate();
    let mut s = sign(&thief, &c, BASE, "POST", LICENCE, good.as_bytes());
    s.instance_key_id = key.public_key_id();
    let forged = [
        (HEADER_KEY_ID, s.instance_key_id.clone()),
        (HEADER_CHALLENGE, s.challenge.clone()),
        (HEADER_SIGNATURE, s.signature_b64.clone()),
    ];
    assert_eq!(
        error_of(post_signed(&f.hub, forged, good.clone()).await).await,
        (StatusCode::UNAUTHORIZED, MSG_BAD_SIGNATURE.to_string())
    );
    // Signed for another origin, path or body.
    for (origin, path, signed_body) in [
        ("https://other.example", LICENCE, good.clone()),
        (BASE, "/api/station/licence?x=1", good.clone()),
        (BASE, LICENCE, body(&key, Some("lic-1"))),
    ] {
        let s = sign(&key, &c, origin, "POST", path, signed_body.as_bytes());
        let headers = [
            (HEADER_KEY_ID, s.instance_key_id),
            (HEADER_CHALLENGE, s.challenge),
            (HEADER_SIGNATURE, s.signature_b64),
        ];
        assert_eq!(
            error_of(post_signed(&f.hub, headers, good.clone()).await).await,
            (StatusCode::UNAUTHORIZED, MSG_BAD_SIGNATURE.to_string())
        );
    }
    // A header key id that is not the body's key.
    let s = sign(&thief, &c, BASE, "POST", LICENCE, good.as_bytes());
    let mismatched = [
        (HEADER_KEY_ID, s.instance_key_id),
        (HEADER_CHALLENGE, s.challenge),
        (HEADER_SIGNATURE, s.signature_b64),
    ];
    assert_eq!(
        error_of(post_signed(&f.hub, mismatched, good.clone()).await).await,
        (StatusCode::UNAUTHORIZED, MSG_KEY_ID_MISMATCH.to_string())
    );
    // Unsigned.
    let resp = f
        .hub
        .http
        .post(f.hub.url(LICENCE))
        .header("content-type", "application/json")
        .body(good.clone())
        .send()
        .await
        .unwrap();
    assert_no_store(&resp);
    assert_eq!(
        error_of(resp).await,
        (StatusCode::UNAUTHORIZED, MSG_UNSIGNED.to_string())
    );
    // A body that is not the request, or a key that does not decode.
    for bad in [
        "not json".to_string(),
        json!({"licence_id": null}).to_string(),
        json!({"instance_public_key": "nope", "licence_id": null}).to_string(),
    ] {
        let s = sign(&key, &c, BASE, "POST", LICENCE, bad.as_bytes());
        let headers = [
            (HEADER_KEY_ID, s.instance_key_id),
            (HEADER_CHALLENGE, s.challenge),
            (HEADER_SIGNATURE, s.signature_b64),
        ];
        let (code, _) = error_of(post_signed(&f.hub, headers, bad).await).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
    }

    // None of that spent the challenge: the genuine request still works.
    assert_eq!(challenges(&f.hub).await, 1);
    let ok = post_with(&f.hub, &key, &c, good).await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(challenges(&f.hub).await, 0);
}

#[tokio::test]
async fn issuing_challenges_purges_expired_ones_and_audits_nothing() {
    let f = fixture().await;
    let rows = f.audit_rows().await;
    let n = PURGE_BATCH + 3;
    sqlx::query(
        "INSERT INTO station_challenges (challenge, created_at, expires_at) \
         SELECT rpad(i::text, 43, 'x'), now() - interval '2 minutes', \
                now() - interval '1 minute' \
         FROM generate_series(1, $1) AS i",
    )
    .bind(n)
    .execute(&f.hub.db.pool)
    .await
    .unwrap();
    challenge(&f.hub).await;
    // One batch purged, and the new one stored.
    assert_eq!(challenges(&f.hub).await, n - PURGE_BATCH + 1);
    challenge(&f.hub).await;
    assert_eq!(challenges(&f.hub).await, 2);
    assert_eq!(f.audit_rows().await, rows);
}

#[tokio::test]
async fn challenges_are_throttled_per_address() {
    let f = fixture().await;
    // The address has used all but one of its challenges this hour.
    sqlx::query(
        "INSERT INTO auth_throttle (key, failures, refused, window_started_at) \
         VALUES ($1, $2, 0, now())",
    )
    .bind(THROTTLE_KEY)
    .bind(STATION_CHALLENGE_IP_LIMIT - 1)
    .execute(&f.hub.db.pool)
    .await
    .unwrap();
    // The last one is issued, and locks the address.
    challenge(&f.hub).await;
    let locked = f.hub.audited("station.challenge_locked").await;
    assert_eq!(locked.len(), 1);
    assert_eq!(locked[0].0, "anonymous");
    assert_eq!(locked[0].1, THROTTLE_KEY);
    // Then refused, with the time to wait.
    let before = challenges(&f.hub).await;
    let resp = challenge_raw(&f.hub).await;
    let retry: u64 = resp.headers()[RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(retry > 3000 && retry <= 3600, "{retry}");
    assert_eq!(
        error_of(resp).await,
        (
            StatusCode::TOO_MANY_REQUESTS,
            MSG_CHALLENGE_LOCKED.to_string()
        )
    );
    assert_eq!(challenges(&f.hub).await, before);
    // Audited once, however often it is refused.
    challenge_raw(&f.hub).await;
    assert_eq!(f.hub.audited("station.challenge_locked").await.len(), 1);
}

#[tokio::test]
async fn concurrent_fetches_during_a_renewal_reissue_once() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let reg = f.register(&key).await;
    f.renew().await;
    let licences = f.hub.count("licences").await;

    let fetches = (0..4).map(|_| fetch(&f.hub, &key, Some(&reg.licence)));
    let answers = futures::future::join_all(fetches).await;
    let mut got = Vec::new();
    for resp in answers {
        got.push(fetched_licence(resp, &key, &f.root).await);
    }
    // Every fetch got the one renewed licence.
    assert!(got.iter().all(|id| id == &got[0]), "{got:?}");
    assert_ne!(got[0], reg.licence);
    assert_eq!(f.hub.count("licences").await, licences + 1);
    assert_eq!(f.hub.audited("licence.superseded").await.len(), 1);
    assert_eq!(f.hub.audited("licence.issued").await.len(), 2);
}

/// A second hub serving `db_url` with no vendor root: its address.
async fn hub_without_root(db_url: &str, artifacts: &std::path::Path) -> String {
    let args = wipe_hub::Args {
        addr: "127.0.0.1:0".parse().unwrap(),
        database_url: db_url.to_string(),
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
        artifact_dir: artifacts.to_path_buf(),
    };
    let mut config = wipe_hub::Config::from_args(args).unwrap();
    config.portal_dir = None;
    let state = wipe_hub::AppState::new(config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(wipe_hub::serve_on(listener, state));
    format!("http://{addr}")
}

#[tokio::test]
async fn a_hub_without_a_vendor_root_reissues_nothing() {
    let f = fixture().await;
    let key = SigningKey::generate();
    let reg = f.register(&key).await;
    f.renew().await;
    let (licences, rows) = (f.hub.count("licences").await, f.audit_rows().await);

    let rootless = hub_without_root(&f.hub.db.url, f.hub.artifacts.path()).await;
    let fetch_from = |held: Option<&str>| {
        let body = body(&key, held);
        let http = f.hub.http.clone();
        let rootless = rootless.clone();
        let key = &key;
        async move {
            let c: Value = http
                .get(format!("{rootless}{CHALLENGE}"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let c = c["challenge"].as_str().unwrap().to_string();
            let s = sign(key, &c, BASE, "POST", LICENCE, body.as_bytes());
            http.post(format!("{rootless}{LICENCE}"))
                .header("content-type", "application/json")
                .header(HEADER_KEY_ID, s.instance_key_id)
                .header(HEADER_CHALLENGE, s.challenge)
                .header(HEADER_SIGNATURE, s.signature_b64)
                .body(body)
                .send()
                .await
                .unwrap()
        }
    };
    // Holding the latest active licence: nothing new.
    assert_current(fetch_from(Some(&reg.licence)).await).await;
    // Holding none: the latest active licence, not a reissue.
    let got = fetched_licence(fetch_from(None).await, &key, &f.root).await;
    assert_eq!(got, reg.licence);
    assert_eq!(f.hub.count("licences").await, licences);
    assert_eq!(f.audit_rows().await, rows);
}

/// The renewal fetch on a credit-pool entitlement moves no credits
/// (controlled-licensing story 3): its answer carries the bare licence, so a
/// grant would strand credits until release.
#[tokio::test]
async fn a_renewal_fetch_on_a_credit_pool_grants_nothing() {
    let f = fixture().await;
    let one_off = f.hub.published_one_off_offering("Site pool", 12).await;
    let pool = buy(&f.hub, &f.owner, &one_off).await;
    let key = SigningKey::generate();
    let v = ok_json(
        f.hub
            .post(
                "/api/stations",
                json!({
                    "entitlement_id": pool,
                    "instance_key_id": key.public_key_id(),
                    "credits": 0,
                }),
                Some(&f.owner.cookie),
            )
            .await,
        StatusCode::CREATED,
    )
    .await;
    let held = v["licence"]["id"].as_str().unwrap().to_string();
    sqlx::query(
        "UPDATE entitlements SET term_end = term_end + interval '1 year' WHERE id = $1::uuid",
    )
    .bind(&pool)
    .execute(&f.hub.db.pool)
    .await
    .unwrap();

    let new = fetched_licence(fetch(&f.hub, &key, Some(&held)).await, &key, &f.root).await;
    assert_ne!(new, held);
    assert_eq!(f.hub.count("credit_grants").await, 0);
    assert!(f.hub.audited("credits.granted").await.is_empty());
    let ents = ok_json(
        f.hub.get("/api/entitlements", Some(&f.owner.cookie)).await,
        StatusCode::OK,
    )
    .await;
    let credits = ents["entitlements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == json!(pool))
        .unwrap()["credits"]
        .clone();
    assert_eq!(credits["granted"], 0);
    assert_eq!(credits["available"], 5000);
}
