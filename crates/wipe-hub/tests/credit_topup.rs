//! A connected station tops up its credit reserve (controlled-licensing
//! story 4, ADR-0013 §3): every hub row of the story's I/O matrix through
//! `POST /api/station/credits`, signed exactly as the station signs
//! (`wipe_license::station_request`), checking the answer, the grant, the
//! pool, the station row's report and flag, and the audit rows. Stations are
//! registered through `POST /api/stations` with the credits each test needs.
//! Needs `docker compose up -d`; fails, never skips, when it is down.

mod support;

use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{assert_error, error_of, spawn_hub_with_root, Hub, Who, BASE};
use uuid::Uuid;
use wipe_cert::SigningKey;
use wipe_hub::station_link::{MSG_FOREIGN_LICENCE, MSG_NO_CREDIT_POOL};
use wipe_hub::station_link_routes::{
    MSG_BAD_CREDITS_BODY, MSG_BAD_SIGNATURE, MSG_CHALLENGE_USED, MSG_NOT_REGISTERED, MSG_UNSIGNED,
};
use wipe_license::station_credits::{TopUpAnswer, TopUpRequest};
use wipe_license::station_request::{sign, HEADER_CHALLENGE, HEADER_KEY_ID, HEADER_SIGNATURE};
use wipe_license::{check_grant_for, CreditGrant, VendorRoot};

const CHALLENGE: &str = "/api/station/challenge";
const CREDITS: &str = wipe_license::station_credits::STATION_CREDITS_PATH;

struct Fixture {
    hub: Hub,
    root: VendorRoot,
    owner: Who,
    org_id: Uuid,
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
    let org_id: Uuid = sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1::uuid")
        .bind(&owner.id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    Fixture {
        hub,
        root,
        owner,
        org_id,
    }
}

/// A station registered on the portal.
struct Station {
    key: SigningKey,
    /// The `stations` row.
    id: String,
    licence: String,
}

impl Fixture {
    /// A published one-off site offering (two seats, `quota` erasures, the
    /// default target reserve 50 and threshold 10), bought: the
    /// entitlement's id.
    async fn pool(&self, quota: i64) -> String {
        self.pool_with(quota, None, None).await
    }

    /// As [`Self::pool`], with the offering's own target reserve and top-up
    /// threshold (`None`: NULL, the defaults).
    async fn pool_with(&self, quota: i64, target: Option<i64>, threshold: Option<i64>) -> String {
        let offering: Uuid = sqlx::query_scalar(
            "INSERT INTO offerings (name, description, price_minor, currency, \
                                    billing_interval, status, scope, seats, quota_erasures, \
                                    features, term_months, published_at, \
                                    credit_target_reserve, credit_topup_threshold) \
             VALUES ('Pool ' || gen_random_uuid(), 'Pool.', 1000, 'USD', 'one_off', \
                     'published', 'site', 2, $1, '{}', 12, now(), $2, $3) RETURNING id",
        )
        .bind(quota)
        .bind(target)
        .bind(threshold)
        .fetch_one(&self.hub.db.pool)
        .await
        .unwrap();
        buy(&self.hub, &self.owner, &offering.to_string()).await
    }

    /// Register a new key on `entitlement` with `credits` in its file
    /// (`None`: the field omitted, as for an unlimited entitlement).
    async fn register(&self, entitlement: &str, credits: Option<i64>) -> Station {
        self.register_key(entitlement, credits, SigningKey::generate())
            .await
    }

    /// As [`Self::register`], for `key`.
    async fn register_key(
        &self,
        entitlement: &str,
        credits: Option<i64>,
        key: SigningKey,
    ) -> Station {
        let mut body = json!({
            "entitlement_id": entitlement,
            "instance_key_id": key.public_key_id(),
            "station_id": "bench-1",
        });
        if let Some(n) = credits {
            body["credits"] = json!(n);
        }
        let v = ok_json(
            self.hub
                .post("/api/stations", body, Some(&self.owner.cookie))
                .await,
            StatusCode::CREATED,
        )
        .await;
        Station {
            key,
            id: v["station"]["id"].as_str().unwrap().to_string(),
            licence: v["licence"]["id"].as_str().unwrap().to_string(),
        }
    }

    async fn available(&self, entitlement: &str) -> i64 {
        let v = ok_json(
            self.hub
                .get("/api/entitlements", Some(&self.owner.cookie))
                .await,
            StatusCode::OK,
        )
        .await;
        let ent = v["entitlements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == entitlement)
            .unwrap()
            .clone();
        ent["credits"]["available"].as_i64().unwrap()
    }

    /// The station as `GET /api/stations` shows it.
    async fn station(&self, s: &Station) -> Value {
        let v = ok_json(
            self.hub
                .get("/api/stations", Some(&self.owner.cookie))
                .await,
            StatusCode::OK,
        )
        .await;
        v["stations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == s.id.as_str())
            .unwrap()
            .clone()
    }

    /// `(credit_balance, credit_spent_reported, mismatch flagged)`.
    async fn report_of(&self, s: &Station) -> (Option<i64>, Option<i64>, bool) {
        sqlx::query_as(
            "SELECT credit_balance, credit_spent_reported, \
                    credit_report_mismatch_at IS NOT NULL \
             FROM stations WHERE id = $1::uuid",
        )
        .bind(&s.id)
        .fetch_one(&self.hub.db.pool)
        .await
        .unwrap()
    }

    async fn grants(&self) -> i64 {
        self.hub.count("credit_grants").await
    }

    async fn release(&self, s: &Station) {
        ok_json(
            self.hub
                .post_empty(
                    &format!("/api/stations/{}/release", s.id),
                    Some(&self.owner.cookie),
                )
                .await,
            StatusCode::OK,
        )
        .await;
    }
}

async fn challenge(hub: &Hub) -> String {
    let resp = hub.http.get(hub.url(CHALLENGE)).send().await.unwrap();
    ok_json(resp, StatusCode::OK).await["challenge"]
        .as_str()
        .unwrap()
        .to_string()
}

fn body(s: &Station, balance: i64, spent: i64, last: Option<&str>) -> String {
    body_naming(s, Some(&s.licence), balance, spent, last)
}

/// As [`body`], naming `licence` as the licence the station holds. Built
/// from the wire type the station's client sends, so a renamed field fails
/// to compile here too.
fn body_naming(
    s: &Station,
    licence: Option<&str>,
    balance: i64,
    spent: i64,
    last: Option<&str>,
) -> String {
    serde_json::to_string(&TopUpRequest {
        instance_public_key: s.key.verifying_key().to_base64(),
        licence_id: licence.map(str::to_owned),
        balance: balance.try_into().unwrap(),
        credits_spent: spent.try_into().unwrap(),
        last_grant_id: last.map(str::to_owned),
    })
    .unwrap()
}

/// `body` signed by `key` over `challenge`, no cookie and no CSRF token.
async fn post_with(hub: &Hub, key: &SigningKey, challenge: &str, body: String) -> Response {
    let s = sign(key, challenge, BASE, "POST", CREDITS, body.as_bytes());
    let resp = hub
        .http
        .post(hub.url(CREDITS))
        .header("content-type", "application/json")
        .header(HEADER_KEY_ID, s.instance_key_id)
        .header(HEADER_CHALLENGE, s.challenge)
        .header(HEADER_SIGNATURE, s.signature_b64)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.headers()["cache-control"], "no-store");
    resp
}

/// A top-up reporting `balance`, `spent` and `last`.
async fn top_up(hub: &Hub, s: &Station, balance: i64, spent: i64, last: Option<&str>) -> Response {
    let c = challenge(hub).await;
    post_with(hub, &s.key, &c, body(s, balance, spent, last)).await
}

async fn top_up_ok(hub: &Hub, s: &Station, balance: i64, spent: i64, last: Option<&str>) -> Value {
    let v = ok_json(top_up(hub, s, balance, spent, last).await, StatusCode::OK).await;
    let mut keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["available", "credit_grant", "target_reserve", "threshold"],
        "{v}"
    );
    v
}

/// The answer's grant, checked against `root` for `s`'s key. The answer
/// parses as the wire type the station reads.
fn grant_of(v: &Value, s: &Station, root: &VendorRoot) -> CreditGrant {
    let answer: TopUpAnswer = serde_json::from_value(v.clone()).unwrap();
    let grant = answer.credit_grant.expect("a credit grant");
    check_grant_for(&grant, &s.key.public_key_id(), &[root.verifying_key()]).unwrap();
    grant
}

// ---------------------------------------------------------------------------
// The matrix

#[tokio::test]
async fn a_station_below_the_threshold_is_topped_up_to_the_target_and_its_report_audited() {
    let f = fixture().await;
    let pool = f.pool(1000).await;
    let s = f.register(&pool, Some(9)).await;
    assert_eq!(f.available(&pool).await, 991);

    let v = top_up_ok(&f.hub, &s, 9, 0, None).await;
    assert_eq!(v["threshold"], 10);
    assert_eq!(v["target_reserve"], 50);
    let grant = grant_of(&v, &s, &f.root);
    assert_eq!(grant.body.credits, 41);
    assert_eq!(grant.body.organization_id, f.org_id.to_string());
    assert_eq!(v["available"], 950);
    assert_eq!(f.available(&pool).await, 950);

    // Stored with no licence, granted by the station itself.
    let (licence, issued_by): (Option<Uuid>, String) =
        sqlx::query_as("SELECT licence_id, issued_by FROM credit_grants WHERE id = $1::uuid")
            .bind(&grant.body.grant_id)
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(licence, None);
    let actor = format!("station:{}", s.id);
    assert_eq!(issued_by, actor);
    let (a, subject, detail) = f.hub.audited("credits.granted").await.pop().unwrap();
    assert_eq!(a, actor);
    assert_eq!(subject, format!("entitlement:{pool}"));
    assert_eq!(detail["licence_id"], Value::Null);
    assert_eq!(detail["credits"], 41);

    // The report is recorded and audited. The stored balance is what the
    // station holds once the answer lands: 9 reported + 41 granted.
    assert_eq!(f.report_of(&s).await, (Some(50), Some(0), false));
    let (a, subject, detail) = f.hub.audited("credits.reported").await.pop().unwrap();
    assert_eq!(a, actor);
    assert_eq!(subject, format!("entitlement:{pool}"));
    assert_eq!(
        detail,
        json!({"station_id": s.id, "balance": 9, "credits_spent": 0,
               "spent_since_last": 0, "adds_up": true,
               "grant_id": grant.body.grant_id, "credits_delivered": 41, "resent": false})
    );
    // The portal shows the reported balance.
    let v = f.station(&s).await;
    assert_eq!(v["credit_balance"], 50);
    assert_eq!(v["credit_report_mismatch_at"], Value::Null);
}

#[tokio::test]
async fn a_station_at_or_above_the_threshold_gets_nothing_but_its_report_is_kept() {
    let f = fixture().await;
    let pool = f.pool(1000).await;
    let s = f.register(&pool, Some(30)).await;
    let v = top_up_ok(&f.hub, &s, 30, 0, None).await;
    assert_eq!(v["credit_grant"], Value::Null);
    assert_eq!(v["available"], 970);
    let v = top_up_ok(&f.hub, &s, 10, 20, None).await;
    assert_eq!(v["credit_grant"], Value::Null);
    assert_eq!(f.report_of(&s).await, (Some(10), Some(20), false));
    let (_, _, detail) = f.hub.audited("credits.reported").await.pop().unwrap();
    assert_eq!(detail["spent_since_last"], 20);
}

#[tokio::test]
async fn a_small_pool_grants_what_it_has_and_an_empty_one_nothing() {
    let f = fixture().await;
    let pool = f.pool(25).await;
    let s = f.register(&pool, Some(0)).await;
    let v = top_up_ok(&f.hub, &s, 0, 0, None).await;
    let grant = grant_of(&v, &s, &f.root);
    assert_eq!(grant.body.credits, 25);
    assert_eq!(v["available"], 0);

    // Spent; the pool is empty.
    let before = f.grants().await;
    let id = grant.body.grant_id.clone();
    let v = top_up_ok(&f.hub, &s, 0, 25, Some(&id)).await;
    assert_eq!(v["credit_grant"], Value::Null);
    assert_eq!(v["available"], 0);
    assert_eq!(f.grants().await, before);
    assert_eq!(f.report_of(&s).await, (Some(0), Some(25), false));
}

#[tokio::test]
async fn a_lost_answer_is_resent_and_nothing_new_is_granted() {
    let f = fixture().await;
    let pool = f.pool(1000).await;
    let s = f.register(&pool, Some(0)).await;
    let first = grant_of(&top_up_ok(&f.hub, &s, 0, 0, None).await, &s, &f.root);
    assert_eq!(first.body.credits, 50);
    let grants = f.grants().await;
    // The answer was lost: the station still names no top-up grant.
    let again = grant_of(&top_up_ok(&f.hub, &s, 0, 0, None).await, &s, &f.root);
    assert_eq!(again, first);
    assert_eq!(f.grants().await, grants);
    assert_eq!(f.available(&pool).await, 950);
    // The resend is audited as one, and the stored balance counts it.
    let (_, _, detail) = f.hub.audited("credits.reported").await.pop().unwrap();
    assert_eq!(detail["grant_id"], json!(first.body.grant_id));
    assert_eq!(detail["credits_delivered"], 50);
    assert_eq!(detail["resent"], true);
    assert_eq!(f.report_of(&s).await.0, Some(50));
    // It took the first now: naming it, holding 50, nothing is due.
    let id1 = first.body.grant_id.clone();
    let v = top_up_ok(&f.hub, &s, 50, 0, Some(&id1)).await;
    assert_eq!(v["credit_grant"], Value::Null);
    assert_eq!(f.report_of(&s).await.0, Some(50));
    let (_, _, detail) = f.hub.audited("credits.reported").await.pop().unwrap();
    assert_eq!(
        (
            detail["grant_id"].clone(),
            detail["credits_delivered"].clone(),
            detail["resent"].clone()
        ),
        (Value::Null, json!(0), json!(false))
    );

    // Spent 45; the second answer is lost too.
    let second = grant_of(&top_up_ok(&f.hub, &s, 5, 45, Some(&id1)).await, &s, &f.root);
    assert_eq!(second.body.credits, 45);
    let resent = grant_of(&top_up_ok(&f.hub, &s, 5, 45, Some(&id1)).await, &s, &f.root);
    assert_eq!(resent, second);
    let (_, _, detail) = f.hub.audited("credits.reported").await.pop().unwrap();
    assert_eq!(detail["resent"], true);
    assert_eq!(detail["credits_delivered"], 45);
    assert_eq!(f.report_of(&s).await.0, Some(50));
    assert_eq!(f.available(&pool).await, 905);
    // Naming the latest: the normal rules (50 held, nothing due).
    let id2 = second.body.grant_id.clone();
    let v = top_up_ok(&f.hub, &s, 50, 45, Some(&id2)).await;
    assert_eq!(v["credit_grant"], Value::Null);
    assert!(f
        .hub
        .audited("credits.reported")
        .await
        .iter()
        .all(|(_, _, d)| d["adds_up"] == true));
}

#[tokio::test]
async fn a_spent_count_that_went_back_is_flagged_and_never_gets_a_resend() {
    let f = fixture().await;
    let pool = f.pool(1000).await;
    let s = f.register(&pool, Some(0)).await;
    let first = grant_of(&top_up_ok(&f.hub, &s, 0, 0, None).await, &s, &f.root);
    let id1 = first.body.grant_id.clone();
    top_up_ok(&f.hub, &s, 20, 30, Some(&id1)).await;
    assert_eq!(f.report_of(&s).await, (Some(20), Some(30), false));

    // A restored lease: spent 10, naming nothing. Not a resend of the first
    // grant: a new grant under the normal rules, and flagged.
    let v = top_up_ok(&f.hub, &s, 0, 10, None).await;
    let grant = grant_of(&v, &s, &f.root);
    assert_ne!(grant.body.grant_id, id1);
    assert_eq!(grant.body.credits, 50);
    let (_, _, flagged) = f.report_of(&s).await;
    assert!(flagged);
    let (_, _, detail) = f.hub.audited("credits.reported").await.pop().unwrap();
    assert_eq!(detail["adds_up"], false);
    assert_eq!(detail["spent_since_last"], -20);
    // The portal shows the flag, and it stays after a report that adds up.
    let id2 = grant.body.grant_id.clone();
    top_up_ok(&f.hub, &s, 50, 10, Some(&id2)).await;
    let v = f.station(&s).await;
    assert!(v["credit_report_mismatch_at"]
        .as_str()
        .unwrap()
        .ends_with('Z'));
}

#[tokio::test]
async fn an_over_report_is_flagged_and_still_granted() {
    let f = fixture().await;
    let pool = f.pool(1000).await;
    let s = f.register(&pool, Some(20)).await;
    // Granted 20; claims 5 held and 30 spent.
    let v = top_up_ok(&f.hub, &s, 5, 30, None).await;
    assert_eq!(grant_of(&v, &s, &f.root).body.credits, 45);
    assert!(f.report_of(&s).await.2);
    let (_, _, detail) = f.hub.audited("credits.reported").await.pop().unwrap();
    assert_eq!(detail["adds_up"], false);
}

#[tokio::test]
async fn racing_top_ups_never_overdraw_the_pool() {
    let f = fixture().await;
    let pool = f.pool(100).await;
    let a = f.register(&pool, Some(30)).await;
    let b = f.register(&pool, Some(30)).await;
    assert_eq!(f.available(&pool).await, 40);
    let (ca, cb) = (challenge(&f.hub).await, challenge(&f.hub).await);
    let (ra, rb) = tokio::join!(
        post_with(&f.hub, &a.key, &ca, body(&a, 0, 30, None)),
        post_with(&f.hub, &b.key, &cb, body(&b, 0, 30, None)),
    );
    let (va, vb) = (
        ok_json(ra, StatusCode::OK).await,
        ok_json(rb, StatusCode::OK).await,
    );
    let credits = |v: &Value, s: &Station| {
        if v["credit_grant"].is_null() {
            0
        } else {
            grant_of(v, s, &f.root).body.credits
        }
    };
    let total = credits(&va, &a) + credits(&vb, &b);
    assert_eq!(total, 40, "{va} {vb}");
    assert_eq!(f.available(&pool).await, 0);
}

#[tokio::test]
async fn a_station_on_an_unlimited_entitlement_has_no_pool_and_nothing_is_written() {
    let f = fixture().await;
    let annual = f.hub.published_offering("Station — annual, 1 seat").await;
    let unlimited = buy(&f.hub, &f.owner, &annual).await;
    let s = f.register(&unlimited, None).await;
    let audits = f.hub.count("audit_log").await;
    assert_error(
        top_up(&f.hub, &s, 0, 0, None).await,
        StatusCode::CONFLICT,
        MSG_NO_CREDIT_POOL,
    )
    .await;
    assert_eq!(f.hub.count("audit_log").await, audits);
    assert_eq!(f.report_of(&s).await, (None, None, false));
}

#[tokio::test]
async fn unregistered_unsigned_forged_and_replayed_requests_are_refused() {
    let f = fixture().await;
    let pool = f.pool(1000).await;
    let s = f.register(&pool, Some(0)).await;
    let audits = f.hub.count("audit_log").await;

    // An unregistered key.
    let stranger = Station {
        key: SigningKey::generate(),
        id: Uuid::nil().to_string(),
        licence: Uuid::nil().to_string(),
    };
    assert_error(
        top_up(&f.hub, &stranger, 0, 0, None).await,
        StatusCode::NOT_FOUND,
        MSG_NOT_REGISTERED,
    )
    .await;

    // Unsigned.
    let resp = f
        .hub
        .http
        .post(f.hub.url(CREDITS))
        .header("content-type", "application/json")
        .body(body(&s, 0, 0, None))
        .send()
        .await
        .unwrap();
    assert_eq!(
        error_of(resp).await,
        (StatusCode::UNAUTHORIZED, MSG_UNSIGNED.to_string())
    );

    // Signed by another key under this key's id.
    let c = challenge(&f.hub).await;
    let good = body(&s, 0, 0, None);
    let thief = SigningKey::generate();
    let forged = sign(&thief, &c, BASE, "POST", CREDITS, good.as_bytes());
    let resp = f
        .hub
        .http
        .post(f.hub.url(CREDITS))
        .header("content-type", "application/json")
        .header(HEADER_KEY_ID, s.key.public_key_id())
        .header(HEADER_CHALLENGE, forged.challenge)
        .header(HEADER_SIGNATURE, forged.signature_b64)
        .body(good.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(
        error_of(resp).await,
        (StatusCode::UNAUTHORIZED, MSG_BAD_SIGNATURE.to_string())
    );
    // Signed for the licence route.
    let wrong_path = sign(
        &s.key,
        &c,
        BASE,
        "POST",
        "/api/station/licence",
        good.as_bytes(),
    );
    let resp = f
        .hub
        .http
        .post(f.hub.url(CREDITS))
        .header("content-type", "application/json")
        .header(HEADER_KEY_ID, wrong_path.instance_key_id)
        .header(HEADER_CHALLENGE, wrong_path.challenge)
        .header(HEADER_SIGNATURE, wrong_path.signature_b64)
        .body(good.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(
        error_of(resp).await,
        (StatusCode::UNAUTHORIZED, MSG_BAD_SIGNATURE.to_string())
    );

    // Bodies that are not a report: 400.
    for bad in [
        json!({"instance_public_key": s.key.verifying_key().to_base64(), "licence_id": null,
               "balance": -1, "credits_spent": 0, "last_grant_id": null}),
        json!({"instance_public_key": s.key.verifying_key().to_base64(), "licence_id": null,
               "balance": 1.5, "credits_spent": 0, "last_grant_id": null}),
        json!({"instance_public_key": s.key.verifying_key().to_base64(), "licence_id": null,
               "balance": 0, "credits_spent": 0, "last_grant_id": 7}),
        json!({"instance_public_key": s.key.verifying_key().to_base64(), "licence_id": null,
               "credits_spent": 0, "last_grant_id": null}),
    ] {
        let resp = post_with(&f.hub, &s.key, &c, bad.to_string()).await;
        assert_error(resp, StatusCode::BAD_REQUEST, MSG_BAD_CREDITS_BODY).await;
    }

    // The genuine request spends the challenge; a replay is refused.
    assert_eq!(
        post_with(&f.hub, &s.key, &c, good.clone()).await.status(),
        StatusCode::OK
    );
    let after = f.hub.count("audit_log").await;
    assert_error(
        post_with(&f.hub, &s.key, &c, good).await,
        StatusCode::UNAUTHORIZED,
        MSG_CHALLENGE_USED,
    )
    .await;
    assert_eq!(f.hub.count("audit_log").await, after);
    // Only the genuine request wrote: a grant and a report.
    assert_eq!(after - audits, 2);
}

#[tokio::test]
async fn a_released_station_returns_its_reported_balance() {
    let f = fixture().await;
    let pool = f.pool(1000).await;
    let s = f.register(&pool, Some(0)).await;
    let id = grant_of(&top_up_ok(&f.hub, &s, 0, 0, None).await, &s, &f.root)
        .body
        .grant_id;
    top_up_ok(&f.hub, &s, 35, 15, Some(&id)).await;
    ok_json(
        f.hub
            .post_empty(
                &format!("/api/stations/{}/release", s.id),
                Some(&f.owner.cookie),
            )
            .await,
        StatusCode::OK,
    )
    .await;
    // 1000 − 50 granted + 35 returned.
    assert_eq!(f.available(&pool).await, 985);
}

/// AC 1: a trial's first connected top-up. The station registered with no
/// credits in its file and has never had an answer (terms unknown), so it
/// asks once with balance 0: the hub grants the whole trial, and the portal
/// shows the balance the station now holds.
#[tokio::test]
async fn a_trial_station_s_first_top_up_takes_the_whole_trial() {
    let f = fixture().await;
    let trial = f.hub.published_trial_offering("Free trial").await;
    let taken = ok_json(
        f.hub
            .post(
                "/api/trials",
                json!({ "offering_id": trial }),
                Some(&f.owner.cookie),
            )
            .await,
        StatusCode::CREATED,
    )
    .await;
    let entitlement = taken["entitlement"]["id"].as_str().unwrap().to_string();
    let s = f.register(&entitlement, Some(0)).await;

    let v = top_up_ok(&f.hub, &s, 0, 0, None).await;
    assert_eq!(grant_of(&v, &s, &f.root).body.credits, 25);
    assert_eq!(v["available"], 0);
    // A trial's target reserve is the default 50, its threshold 10.
    assert_eq!(
        (v["threshold"].clone(), v["target_reserve"].clone()),
        (json!(10), json!(50))
    );
    let v = f.station(&s).await;
    assert_eq!(v["credit_balance"], 25, "{v}");
    assert_eq!(v["credits_granted"], 25, "{v}");
}

/// The offering's own threshold and target: echoed in every answer; 15
/// (below 20) is topped up to 80; 25 gets nothing.
#[tokio::test]
async fn an_offering_s_own_threshold_and_target_reserve_apply() {
    let f = fixture().await;
    let pool = f.pool_with(1000, Some(80), Some(20)).await;
    let s = f.register(&pool, Some(15)).await;
    let v = top_up_ok(&f.hub, &s, 15, 0, None).await;
    assert_eq!(v["threshold"], 20);
    assert_eq!(v["target_reserve"], 80);
    let grant = grant_of(&v, &s, &f.root);
    assert_eq!(grant.body.credits, 65);
    assert_eq!(f.report_of(&s).await.0, Some(80));

    let id = grant.body.grant_id.clone();
    let before = f.grants().await;
    let v = top_up_ok(&f.hub, &s, 25, 55, Some(&id)).await;
    assert_eq!(v["credit_grant"], Value::Null);
    assert_eq!(
        (v["threshold"].clone(), v["target_reserve"].clone()),
        (json!(20), json!(80))
    );
    assert_eq!(f.grants().await, before);
    assert_eq!(f.report_of(&s).await, (Some(25), Some(55), false));
}

/// A refunded (not current) entitlement grants nothing and answers
/// `available: 0`; the report is still recorded.
#[tokio::test]
async fn an_entitlement_that_is_not_current_grants_nothing() {
    let f = fixture().await;
    let pool = f.pool(1000).await;
    let s = f.register(&pool, Some(0)).await;
    sqlx::query(
        "UPDATE entitlements SET status = 'refunded', refunded_at = now(), renews = false \
         WHERE id = $1::uuid",
    )
    .bind(&pool)
    .execute(&f.hub.db.pool)
    .await
    .unwrap();
    let before = f.grants().await;
    let v = top_up_ok(&f.hub, &s, 0, 0, None).await;
    assert_eq!(v["credit_grant"], Value::Null);
    assert_eq!(v["available"], 0);
    assert_eq!(f.grants().await, before);
    assert_eq!(f.report_of(&s).await, (Some(0), Some(0), false));
}

/// A report naming no licence, or a licence of an earlier registration of
/// the same key, is refused: nothing is granted, recorded or audited.
#[tokio::test]
async fn a_report_from_another_registration_is_refused_and_nothing_is_written() {
    let f = fixture().await;
    let pool = f.pool(1000).await;
    let seed: [u8; 32] = rand_seed();
    let old = f
        .register_key(&pool, Some(0), SigningKey::from_seed(seed))
        .await;
    f.release(&old).await;
    let s = f
        .register_key(&pool, Some(0), SigningKey::from_seed(seed))
        .await;
    let snapshot = (
        f.hub.count("audit_log").await,
        f.grants().await,
        f.report_of(&s).await,
    );

    for licence in [None, Some(old.licence.as_str())] {
        let c = challenge(&f.hub).await;
        let resp = post_with(&f.hub, &s.key, &c, body_naming(&s, licence, 0, 0, None)).await;
        assert_error(resp, StatusCode::CONFLICT, MSG_FOREIGN_LICENCE).await;
        assert_eq!(
            (
                f.hub.count("audit_log").await,
                f.grants().await,
                f.report_of(&s).await,
            ),
            snapshot
        );
    }
    // Its own licence is served.
    top_up_ok(&f.hub, &s, 0, 0, None).await;
}

fn rand_seed() -> [u8; 32] {
    *Uuid::new_v4().as_bytes().repeat(2).first_chunk().unwrap()
}

/// A station whose latest licence was revoked (its row not released) is
/// granted and resent nothing: its report is kept, and it is answered
/// `available: 0`.
#[tokio::test]
async fn a_station_whose_licence_was_revoked_gets_no_credits() {
    let f = fixture().await;
    let admin = support::hub::bootstrap_admin(&f.hub).await;
    let pool = f.pool(1000).await;
    let s = f.register(&pool, Some(0)).await;
    ok_json(
        f.hub
            .post(
                &format!("/api/admin/licences/{}/revoke", s.licence),
                json!({ "reason": "chargeback" }),
                Some(&admin.cookie),
            )
            .await,
        StatusCode::OK,
    )
    .await;
    let before = f.grants().await;
    let v = top_up_ok(&f.hub, &s, 0, 0, None).await;
    assert_eq!(v["credit_grant"], Value::Null);
    assert_eq!(v["available"], 0);
    assert_eq!(f.grants().await, before);
    assert_eq!(f.report_of(&s).await, (Some(0), Some(0), false));
}

/// A count the database cannot hold is a 400 before the challenge is
/// spent: the same challenge then carries a good request.
#[tokio::test]
async fn an_out_of_range_count_is_refused_without_spending_the_challenge() {
    let f = fixture().await;
    let pool = f.pool(1000).await;
    let s = f.register(&pool, Some(0)).await;
    let c = challenge(&f.hub).await;
    for (balance, spent) in [(u64::MAX, 0), (0, i64::MAX as u64 + 1)] {
        let huge = serde_json::to_string(&TopUpRequest {
            instance_public_key: s.key.verifying_key().to_base64(),
            licence_id: Some(s.licence.clone()),
            balance,
            credits_spent: spent,
            last_grant_id: None,
        })
        .unwrap();
        assert_error(
            post_with(&f.hub, &s.key, &c, huge).await,
            StatusCode::BAD_REQUEST,
            MSG_BAD_CREDITS_BODY,
        )
        .await;
    }
    assert_eq!(
        post_with(&f.hub, &s.key, &c, body(&s, 0, 0, None))
            .await
            .status(),
        StatusCode::OK
    );
}
