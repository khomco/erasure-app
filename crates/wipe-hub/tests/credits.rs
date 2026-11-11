//! The organization credit pool and signed credit grants
//! (controlled-licensing story 3, ADR-0013 §3): every hub row of the story's
//! I/O matrix through HTTP, checking the response, the downloaded licence
//! file, the pool on `GET /api/entitlements`, the ledgers and the audit rows.
//! Needs `docker compose up -d`; fails, never skips, when it is down.

mod support;

use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{assert_error, bootstrap_admin, spawn_hub_with_root, Hub, Who};
use uuid::Uuid;
use wipe_hub::credits::{MSG_CREDITS_NEGATIVE, MSG_CREDITS_UNLIMITED, MSG_NOT_ENOUGH_CREDITS};
use wipe_hub::stations::MSG_LICENCE_UP_TO_DATE;
use wipe_license::{
    check_grant_for, check_installable, parse_license, parse_license_file, LicenseCertificate,
    Quota, VendorRoot,
};

struct Fixture {
    hub: Hub,
    root: VendorRoot,
    owner: Who,
    org_id: Uuid,
    /// Annual, one machine seat, unlimited erasures.
    unlimited: String,
    /// One-off, two site seats, 5000 erasures (target reserve 50).
    pool: String,
}

fn new_key() -> String {
    wipe_cert::SigningKey::generate().public_key_id()
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

/// A published one-off site offering with `seats` seats and `quota`
/// erasures, inserted directly: its id. `reserve` is the stored target
/// reserve (NULL reads as 50).
async fn pool_offering(
    hub: &Hub,
    name: &str,
    seats: i32,
    quota: i64,
    reserve: Option<i64>,
) -> String {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO offerings (name, description, price_minor, currency, billing_interval, \
                                status, scope, seats, quota_erasures, features, term_months, \
                                published_at, credit_target_reserve) \
         VALUES ($1, 'Pool.', 1000, 'USD', 'one_off', 'published', 'site', $2, $3, \
                 '{pdf_certificates}', 12, now(), $4) RETURNING id",
    )
    .bind(name)
    .bind(seats)
    .bind(quota)
    .bind(reserve)
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
    let annual = hub.published_offering("Station — annual, 1 seat").await;
    let one_off = hub.published_one_off_offering("Site — 2 years", 24).await;
    let unlimited = buy(&hub, &owner, &annual).await;
    let pool = buy(&hub, &owner, &one_off).await;
    Fixture {
        hub,
        root,
        owner,
        org_id,
        unlimited,
        pool,
    }
}

impl Fixture {
    async fn register(&self, body: Value) -> Response {
        self.hub
            .post("/api/stations", body, Some(&self.owner.cookie))
            .await
    }

    /// Register `key` on `entitlement` with `credits` (`None`: omitted): 201.
    async fn register_ok(&self, entitlement: &str, key: &str, credits: Option<i64>) -> Value {
        let mut body = json!({ "entitlement_id": entitlement, "instance_key_id": key });
        if let Some(n) = credits {
            body["credits"] = json!(n);
        }
        ok_json(self.register(body).await, StatusCode::CREATED).await
    }

    async fn reissue(&self, station: &str, body: Option<Value>) -> Response {
        let path = format!("/api/stations/{station}/reissue");
        match body {
            Some(b) => self.hub.post(&path, b, Some(&self.owner.cookie)).await,
            None => self.hub.post_empty(&path, Some(&self.owner.cookie)).await,
        }
    }

    async fn release(&self, station: &str) -> Value {
        ok_json(
            self.hub
                .post_empty(
                    &format!("/api/stations/{station}/release"),
                    Some(&self.owner.cookie),
                )
                .await,
            StatusCode::OK,
        )
        .await
    }

    async fn download(&self, licence: &str) -> String {
        let resp = self
            .hub
            .get(
                &format!("/api/licences/{licence}/download"),
                Some(&self.owner.cookie),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        resp.text().await.unwrap()
    }

    /// The entitlement's `credits` on `GET /api/entitlements`.
    async fn pool_of(&self, entitlement: &str) -> Value {
        let v = ok_json(
            self.hub
                .get("/api/entitlements", Some(&self.owner.cookie))
                .await,
            StatusCode::OK,
        )
        .await;
        v["entitlements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == json!(entitlement))
            .unwrap()["credits"]
            .clone()
    }

    async fn station(&self, id: &str) -> Value {
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
            .find(|s| s["id"] == json!(id))
            .unwrap()
            .clone()
    }

    /// Store a reported balance on the station, as story 4's report will.
    async fn report(&self, station: &str, balance: i64) {
        sqlx::query(
            "UPDATE stations SET credit_balance = $2, credit_balance_reported_at = now() \
             WHERE id = $1::uuid",
        )
        .bind(station)
        .bind(balance)
        .execute(&self.hub.db.pool)
        .await
        .unwrap();
    }

    /// Row counts a refused request must leave unchanged.
    async fn counts(&self) -> Vec<i64> {
        let mut out = Vec::new();
        for t in [
            "stations",
            "licences",
            "credit_grants",
            "credit_returns",
            "audit_log",
        ] {
            out.push(self.hub.count(t).await);
        }
        out
    }

    async fn stored_document(&self, licence: &str) -> String {
        sqlx::query_scalar("SELECT document FROM licences WHERE id = $1::uuid")
            .bind(licence)
            .fetch_one(&self.hub.db.pool)
            .await
            .unwrap()
    }
}

fn pool(purchased: i64, granted: i64, returned: i64, target: i64) -> Value {
    json!({
        "purchased": purchased,
        "granted": granted,
        "returned": returned,
        "available": purchased - granted + returned,
        "target_reserve": target,
    })
}

fn id_of(v: &Value) -> String {
    v.as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_default_registration_puts_the_target_reserve_in_the_licence_file() {
    let f = fixture().await;
    let key = new_key();
    let v = f.register_ok(&f.pool, &key, None).await;
    let mut keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["credit_grant", "licence", "station"]);
    assert_eq!(v["credit_grant"]["credits"], 50);
    let grant_id = id_of(&v["credit_grant"]["id"]);
    let licence_id = id_of(&v["licence"]["id"]);
    let station_id = id_of(&v["station"]["id"]);
    assert_eq!(v["station"]["credits_granted"], 50);
    assert_eq!(v["station"]["credit_balance"], Value::Null);
    assert_eq!(v["station"]["credit_balance_reported_at"], Value::Null);

    // The file carries the licence and a grant for the station's key.
    let text = f.download(&licence_id).await;
    let file = parse_license_file(&text).unwrap();
    let roots = [f.root.verifying_key()];
    check_installable(&file.license, &key, &roots).unwrap();
    let grant = file.credit_grant.as_ref().expect("a credit grant");
    check_grant_for(grant, &key, &roots).unwrap();
    assert_eq!(grant.body.credits, 50);
    assert_eq!(grant.body.grant_id, grant_id);
    assert_eq!(grant.body.organization_id, f.org_id.to_string());
    // The licence inside is the stored one, unchanged, and still counts the
    // order quota (story 4 marks it pool-backed).
    let stored: LicenseCertificate =
        serde_json::from_str(&f.stored_document(&licence_id).await).unwrap();
    assert_eq!(file.license, stored);
    assert_eq!(
        file.license.body.entitlements.quota,
        Quota::Count { erasures: 5000 }
    );
    // The licence-only parser takes the same file.
    assert_eq!(parse_license(&text).unwrap(), stored);
    let value: Value = serde_json::from_str(&text).unwrap();
    let mut keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["credit_grant", "license"]);

    // The pool shrank by the grant.
    assert_eq!(f.pool_of(&f.pool).await, pool(5000, 50, 0, 50));
    assert_eq!(f.pool_of(&f.unlimited).await, Value::Null);

    // Stored and audited.
    let row: (String, String, i64, String) = sqlx::query_as(
        "SELECT station_id::text, licence_id::text, credits, issued_by FROM credit_grants \
         WHERE id = $1::uuid",
    )
    .bind(&grant_id)
    .fetch_one(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            station_id.clone(),
            licence_id.clone(),
            50,
            format!("user:{}", f.owner.id)
        )
    );
    assert_eq!(
        f.hub.audited("credits.granted").await,
        vec![(
            format!("user:{}", f.owner.id),
            format!("entitlement:{}", f.pool),
            json!({
                "grant_id": grant_id,
                "entitlement_id": f.pool,
                "station_id": station_id,
                "licence_id": licence_id,
                "credits": 50,
                "available_after": 4950,
            })
        )]
    );
}

#[tokio::test]
async fn the_vendor_admin_downloads_and_inspects_the_same_licence_file() {
    let f = fixture().await;
    let key = new_key();
    let v = f.register_ok(&f.pool, &key, None).await;
    let licence_id = id_of(&v["licence"]["id"]);
    let customer = f.download(&licence_id).await;
    let admin = bootstrap_admin(&f.hub).await;

    // The admin download is the customer's file, grant included.
    let resp = f
        .hub
        .get(
            &format!("/api/admin/licences/{licence_id}/download"),
            Some(&admin.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let text = resp.text().await.unwrap();
    assert_eq!(text, customer);
    let grant = parse_license_file(&text).unwrap().credit_grant.unwrap();
    assert_eq!(grant.body.credits, 50);

    // Pasting that file into inspect verifies its licence and finds it.
    let v = ok_json(
        f.hub
            .post(
                "/api/admin/licences/inspect",
                json!({ "document": customer }),
                Some(&admin.cookie),
            )
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(v["verification"]["status"], "verified");
    assert_eq!(v["licence"]["id"], json!(licence_id));
}

#[tokio::test]
async fn a_small_pool_grants_what_is_left() {
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
    let entitlement = id_of(&taken["entitlement"]["id"]);
    let v = f.register_ok(&entitlement, &new_key(), None).await;
    assert_eq!(v["credit_grant"]["credits"], 25);
    assert_eq!(f.pool_of(&entitlement).await, pool(25, 25, 0, 50));
}

#[tokio::test]
async fn zero_credits_issue_the_bare_licence_byte_for_byte() {
    let f = fixture().await;
    let v = f.register_ok(&f.pool, &new_key(), Some(0)).await;
    assert_eq!(v["credit_grant"], Value::Null);
    assert_eq!(v["station"]["credits_granted"], 0);
    let licence = id_of(&v["licence"]["id"]);
    assert_eq!(
        f.download(&licence).await,
        f.stored_document(&licence).await
    );
    assert_eq!(f.hub.count("credit_grants").await, 0);
    assert!(f.hub.audited("credits.granted").await.is_empty());
    assert_eq!(f.pool_of(&f.pool).await, pool(5000, 0, 0, 50));
}

#[tokio::test]
async fn an_empty_pool_registers_without_a_grant() {
    let f = fixture().await;
    let small = pool_offering(&f.hub, "Small pool", 2, 50, None).await;
    let small = buy(&f.hub, &f.owner, &small).await;
    let first = f.register_ok(&small, &new_key(), None).await;
    assert_eq!(first["credit_grant"]["credits"], 50);
    let second = f.register_ok(&small, &new_key(), None).await;
    assert_eq!(second["credit_grant"], Value::Null);
    assert_eq!(f.pool_of(&small).await, pool(50, 50, 0, 50));
}

#[tokio::test]
async fn a_stored_target_reserve_is_the_default() {
    let f = fixture().await;
    let offering = pool_offering(&f.hub, "Big reserve", 2, 1000, Some(200)).await;
    let ent = buy(&f.hub, &f.owner, &offering).await;
    let v = f.register_ok(&ent, &new_key(), None).await;
    assert_eq!(v["credit_grant"]["credits"], 200);
    assert_eq!(f.pool_of(&ent).await, pool(1000, 200, 0, 200));
}

#[tokio::test]
async fn refused_credit_counts_write_nothing() {
    let f = fixture().await;
    let before = f.counts().await;
    // More than the pool holds.
    assert_error(
        f.register(json!({
            "entitlement_id": f.pool,
            "instance_key_id": new_key(),
            "credits": 5001,
        }))
        .await,
        StatusCode::CONFLICT,
        MSG_NOT_ENOUGH_CREDITS,
    )
    .await;
    // Any count on an unlimited entitlement, 0 included.
    for n in [5, 0] {
        assert_error(
            f.register(json!({
                "entitlement_id": f.unlimited,
                "instance_key_id": new_key(),
                "station_id": "bench-1",
                "credits": n,
            }))
            .await,
            StatusCode::BAD_REQUEST,
            MSG_CREDITS_UNLIMITED,
        )
        .await;
    }
    // Negative.
    assert_error(
        f.register(json!({
            "entitlement_id": f.pool,
            "instance_key_id": new_key(),
            "credits": -1,
        }))
        .await,
        StatusCode::BAD_REQUEST,
        MSG_CREDITS_NEGATIVE,
    )
    .await;
    // Not a whole number.
    let resp = f
        .register(json!({
            "entitlement_id": f.pool,
            "instance_key_id": new_key(),
            "credits": 1.5,
        }))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(f.counts().await, before);
    // `null` is omitted: an unlimited registration still works.
    let v = ok_json(
        f.register(json!({
            "entitlement_id": f.unlimited,
            "instance_key_id": new_key(),
            "station_id": "bench-1",
            "credits": null,
        }))
        .await,
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(v["credit_grant"], Value::Null);
    assert_eq!(v["station"]["credits_granted"], 0);
}

#[tokio::test]
async fn racing_registrations_never_overdraw_the_pool() {
    let f = fixture().await;
    let offering = pool_offering(&f.hub, "Sixty", 2, 60, None).await;
    let ent = buy(&f.hub, &f.owner, &offering).await;
    let calls = (0..2)
        .map(|_| {
            f.register(json!({
                "entitlement_id": ent,
                "instance_key_id": new_key(),
                "credits": 50,
            }))
        })
        .collect();
    let mut created = 0;
    for resp in futures_join_all(calls).await {
        match resp.status() {
            StatusCode::CREATED => created += 1,
            _ => assert_error(resp, StatusCode::CONFLICT, MSG_NOT_ENOUGH_CREDITS).await,
        }
    }
    assert_eq!(created, 1);
    assert_eq!(f.hub.count("stations").await, 1);
    assert_eq!(f.pool_of(&ent).await, pool(60, 50, 0, 50));
}

#[tokio::test]
async fn an_up_to_date_station_is_reissued_to_add_credits() {
    let f = fixture().await;
    let key = new_key();
    let v = f.register_ok(&f.pool, &key, None).await;
    let station = id_of(&v["station"]["id"]);
    let old = id_of(&v["licence"]["id"]);

    let r = ok_json(
        f.reissue(&station, Some(json!({ "credits": 20 }))).await,
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(r["credit_grant"]["credits"], 20);
    assert_eq!(r["station"]["credits_granted"], 70);
    let new = id_of(&r["licence"]["id"]);
    assert_ne!(new, old);
    assert_eq!(r["licence"]["status"], "active");
    let status: String = sqlx::query_scalar("SELECT status FROM licences WHERE id = $1::uuid")
        .bind(&old)
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(status, "superseded");
    let superseded = f.hub.audited("licence.superseded").await;
    assert_eq!(superseded.len(), 1);
    assert_eq!(superseded[0].1, format!("licence:{old}"));
    assert_eq!(superseded[0].2["reason"], "credits added");
    assert_eq!(superseded[0].2["superseded_by"], json!(new));

    // The new file carries the new grant for the same key.
    let file = parse_license_file(&f.download(&new).await).unwrap();
    let grant = file.credit_grant.unwrap();
    assert_eq!(grant.body.credits, 20);
    check_grant_for(&grant, &key, &[f.root.verifying_key()]).unwrap();
    assert_eq!(f.pool_of(&f.pool).await, pool(5000, 70, 0, 50));
    let granted = f.hub.audited("credits.granted").await;
    assert_eq!(granted.len(), 2);
    assert_eq!(granted[1].2["available_after"], 4930);
    assert_eq!(granted[1].2["licence_id"], json!(new));
}

#[tokio::test]
async fn an_up_to_date_station_with_nothing_to_add_is_not_reissued() {
    let f = fixture().await;
    let v = f.register_ok(&f.pool, &new_key(), None).await;
    let station = id_of(&v["station"]["id"]);
    let u = ok_json(
        f.register(json!({
            "entitlement_id": f.unlimited,
            "instance_key_id": new_key(),
            "station_id": "bench-1",
        }))
        .await,
        StatusCode::CREATED,
    )
    .await;
    let unlimited_station = id_of(&u["station"]["id"]);
    let before = f.counts().await;
    for body in [None, Some(json!({})), Some(json!({ "credits": 0 }))] {
        assert_error(
            f.reissue(&station, body).await,
            StatusCode::CONFLICT,
            MSG_LICENCE_UP_TO_DATE,
        )
        .await;
    }
    for body in [None, Some(json!({})), Some(json!({ "credits": null }))] {
        assert_error(
            f.reissue(&unlimited_station, body).await,
            StatusCode::CONFLICT,
            MSG_LICENCE_UP_TO_DATE,
        )
        .await;
    }
    // Any count on an unlimited entitlement, 0 included.
    for n in [0, 5] {
        assert_error(
            f.reissue(&unlimited_station, Some(json!({ "credits": n })))
                .await,
            StatusCode::BAD_REQUEST,
            MSG_CREDITS_UNLIMITED,
        )
        .await;
    }
    assert_error(
        f.reissue(&station, Some(json!({ "credits": 4951 }))).await,
        StatusCode::CONFLICT,
        MSG_NOT_ENOUGH_CREDITS,
    )
    .await;
    assert_error(
        f.reissue(&station, Some(json!({ "credits": -2 }))).await,
        StatusCode::BAD_REQUEST,
        MSG_CREDITS_NEGATIVE,
    )
    .await;
    let resp = f.reissue(&station, Some(json!({ "credit": 5 }))).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn a_renewal_reissue_grants_the_default_reserve() {
    let f = fixture().await;
    let v = f.register_ok(&f.pool, &new_key(), Some(0)).await;
    let station = id_of(&v["station"]["id"]);
    sqlx::query(
        "UPDATE entitlements SET term_end = term_end + interval '1 year' WHERE id = $1::uuid",
    )
    .bind(&f.pool)
    .execute(&f.hub.db.pool)
    .await
    .unwrap();
    let r = ok_json(f.reissue(&station, None).await, StatusCode::CREATED).await;
    assert_eq!(r["credit_grant"]["credits"], 50);
    assert_eq!(
        f.hub.audited("licence.superseded").await[0].2["reason"],
        "renewed"
    );
    assert_eq!(f.pool_of(&f.pool).await, pool(5000, 50, 0, 50));
}

#[tokio::test]
async fn a_release_returns_the_reported_balance_capped_at_what_was_granted() {
    let f = fixture().await;
    let offering = pool_offering(&f.hub, "Three seats", 3, 5000, None).await;
    let ent = buy(&f.hub, &f.owner, &offering).await;
    let mut stations = Vec::new();
    for _ in 0..3 {
        let v = f.register_ok(&ent, &new_key(), None).await;
        stations.push(id_of(&v["station"]["id"]));
    }
    assert_eq!(f.pool_of(&ent).await, pool(5000, 150, 0, 50));

    // Reported 30 of 50: 30 come back.
    f.report(&stations[0], 30).await;
    let listed = f.station(&stations[0]).await;
    assert_eq!(listed["credit_balance"], 30);
    assert!(listed["credit_balance_reported_at"]
        .as_str()
        .unwrap()
        .ends_with('Z'));
    assert_eq!(listed["credits_granted"], 50);
    let released = f.release(&stations[0]).await;
    assert_eq!(released["station"]["credit_balance"], 30);
    assert_eq!(f.pool_of(&ent).await, pool(5000, 150, 30, 50));
    // Reported 80 of 50: only 50 come back.
    f.report(&stations[1], 80).await;
    f.release(&stations[1]).await;
    assert_eq!(f.pool_of(&ent).await, pool(5000, 150, 80, 50));
    // Never reported: 0, still recorded.
    f.release(&stations[2]).await;
    assert_eq!(f.pool_of(&ent).await, pool(5000, 150, 80, 50));

    let returned = f.hub.audited("credits.returned").await;
    let credits: Vec<(Value, Value, bool)> = returned
        .iter()
        .map(|(actor, subject, d)| {
            assert_eq!(actor, &format!("user:{}", f.owner.id));
            assert_eq!(subject, &format!("entitlement:{ent}"));
            assert_eq!(d["entitlement_id"], json!(ent));
            (
                d["station_id"].clone(),
                d["credits"].clone(),
                d["reported_at"].is_null(),
            )
        })
        .collect();
    assert_eq!(
        credits,
        vec![
            (json!(stations[0]), json!(30), false),
            (json!(stations[1]), json!(50), false),
            (json!(stations[2]), json!(0), true),
        ]
    );
    assert_eq!(f.hub.count("credit_returns").await, 3);
}

#[tokio::test]
async fn releasing_an_unlimited_station_returns_nothing() {
    let f = fixture().await;
    let v = ok_json(
        f.register(json!({
            "entitlement_id": f.unlimited,
            "instance_key_id": new_key(),
            "station_id": "bench-1",
        }))
        .await,
        StatusCode::CREATED,
    )
    .await;
    f.release(v["station"]["id"].as_str().unwrap()).await;
    assert_eq!(f.hub.count("credit_returns").await, 0);
    assert!(f.hub.audited("credits.returned").await.is_empty());
}

#[tokio::test]
async fn the_ledgers_are_append_only() {
    let f = fixture().await;
    let v = f.register_ok(&f.pool, &new_key(), None).await;
    f.release(v["station"]["id"].as_str().unwrap()).await;
    for sql in [
        "UPDATE credit_grants SET credits = credits + 1",
        "DELETE FROM credit_grants",
        "TRUNCATE credit_grants",
        "UPDATE credit_returns SET credits = credits + 1",
        "DELETE FROM credit_returns",
        "TRUNCATE credit_returns",
    ] {
        let err = sqlx::query(sql)
            .execute(&f.hub.db.pool)
            .await
            .expect_err(sql);
        let msg = err.as_database_error().unwrap().message().to_string();
        assert!(msg.contains("append-only"), "{sql}: {msg}");
    }
    // A grant of nothing, or a negative return, is refused outright.
    let err = sqlx::query(
        "INSERT INTO credit_returns (organization_id, entitlement_id, station_id, credits, \
                                     returned_by) \
         SELECT organization_id, entitlement_id, id, -1, 'x' FROM stations",
    )
    .execute(&f.hub.db.pool)
    .await
    .unwrap_err();
    assert!(err.as_database_error().unwrap().constraint().is_some());
}

/// Run every future concurrently on this task; their outputs in order.
async fn futures_join_all<F: std::future::Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut pinned: Vec<_> = futures.into_iter().map(Box::pin).collect();
    let mut out: Vec<Option<F::Output>> = (0..pinned.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (i, f) in pinned.iter_mut().enumerate() {
            if out[i].is_none() {
                match f.as_mut().poll(cx) {
                    std::task::Poll::Ready(v) => out[i] = Some(v),
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    out.into_iter().map(Option::unwrap).collect()
}
