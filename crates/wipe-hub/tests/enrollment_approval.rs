//! An owner approves or denies a station's enrollment (station-enrollment
//! story 2, CAP-2, ADR-0008 §1): every row of the story's I/O matrix through
//! real HTTP — lookup, approval (registering through `stations::register`),
//! each refusal leaving the enrollment pending, denial, members and vendor
//! admins, another organization's owner, the per-user code throttle, no
//! vendor root and the CSRF token. Needs `docker compose up -d`; fails,
//! never skips, when it is down.

mod support;

use reqwest::header::RETRY_AFTER;
use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{
    assert_error, bootstrap_admin, error_of, spawn_hub, spawn_hub_with_root, Hub, Who,
};
use uuid::Uuid;
use wipe_cert::SigningKey;
use wipe_hub::enrollments::{
    format_user_code, MSG_LOOKUP_LOCKED, MSG_NO_ENROLLMENT, MSG_OWNER_ONLY_ENROLLMENTS,
};
use wipe_hub::order_routes::MSG_NO_ENTITLEMENT;
use wipe_hub::stations::{
    MSG_ALREADY_REGISTERED, MSG_ENTITLEMENT_NOT_CURRENT, MSG_ISSUANCE_UNAVAILABLE, MSG_NO_FREE_SEAT,
};
use wipe_hub::throttle::{ENROLL_LOOKUP_LIMIT, ENROLL_USER_KEY_PREFIX};
use wipe_license::enrollment::{sign_poll, sign_start};
use wipe_license::{check_installable, parse_license, VendorRoot};

const START: &str = "/api/enrollments";
const POLL: &str = "/api/enrollments/poll";
const LOOKUP: &str = "/api/enrollments/lookup";
const APPROVE: &str = "/api/enrollments/approve";
const DENY: &str = "/api/enrollments/deny";
const CSRF_ERROR: &str = "missing or invalid CSRF token";

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

fn assert_no_store(resp: &Response) {
    let h = resp.headers();
    assert_eq!(h["cache-control"], "no-store", "{} {h:?}", resp.status());
    assert_eq!(h["pragma"], "no-cache", "{} {h:?}", resp.status());
}

/// A station's plain POST: no cookie, no CSRF token.
async fn station_post(hub: &Hub, path: &str, body: &Value) -> Response {
    hub.http
        .post(hub.url(path))
        .json(body)
        .send()
        .await
        .unwrap()
}

/// A started enrollment: its key, device code, user code (`XXXX-XXXX`) and
/// row id.
struct Started {
    key: SigningKey,
    device_code: String,
    user_code: String,
    id: Uuid,
}

/// A fresh key starts enrolling as `bench-1`, labelled `Bench 1`.
async fn start(hub: &Hub) -> Started {
    let key = SigningKey::generate();
    let req = sign_start(&key, "bench-1", Some("Bench 1"));
    let v = ok_json(
        station_post(hub, START, &serde_json::to_value(req).unwrap()).await,
        StatusCode::OK,
    )
    .await;
    let user_code = v["user_code"].as_str().unwrap().to_string();
    let id = sqlx::query_scalar("SELECT id FROM enrollments WHERE user_code = $1")
        .bind(user_code.replace('-', ""))
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    Started {
        key,
        device_code: v["device_code"].as_str().unwrap().to_string(),
        user_code,
        id,
    }
}

async fn poll(hub: &Hub, s: &Started) -> Response {
    station_post(
        hub,
        POLL,
        &serde_json::to_value(sign_poll(&s.key, &s.device_code)).unwrap(),
    )
    .await
}

/// An owner's POST with session and CSRF token; every response must forbid
/// caching.
async fn call(hub: &Hub, path: &str, body: Value, who: &Who) -> Response {
    let resp = hub.post(path, body, Some(&who.cookie)).await;
    assert_no_store(&resp);
    resp
}

async fn lookup(hub: &Hub, who: &Who, code: &str) -> Response {
    call(hub, LOOKUP, json!({ "code": code }), who).await
}

async fn approve(hub: &Hub, who: &Who, code: &str, entitlement: &str) -> Response {
    call(
        hub,
        APPROVE,
        json!({ "code": code, "entitlement_id": entitlement }),
        who,
    )
    .await
}

async fn deny(hub: &Hub, who: &Who, code: &str) -> Response {
    call(hub, DENY, json!({ "code": code }), who).await
}

async fn status_of(hub: &Hub, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM enrollments WHERE id = $1")
        .bind(id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
}

fn user_key(who: &Who) -> String {
    format!("{ENROLL_USER_KEY_PREFIX}{}", who.id)
}

/// The user's code-entry charges outstanding (`None`: no throttle row).
async fn failures(hub: &Hub, who: &Who) -> Option<i32> {
    sqlx::query_scalar("SELECT failures FROM auth_throttle WHERE key = $1")
        .bind(user_key(who))
        .fetch_optional(&hub.db.pool)
        .await
        .unwrap()
}

/// Let the station's next poll through the interval check.
async fn backdate_poll(hub: &Hub, id: Uuid) {
    sqlx::query(
        "UPDATE enrollments SET last_polled_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(id)
    .execute(&hub.db.pool)
    .await
    .unwrap();
}

/// A code with the right shape that names no enrollment.
fn unknown_code(not: &str) -> String {
    let code = if not.replace('-', "") == "BCDFGHJK" {
        "ZXWVTSRQ"
    } else {
        "BCDFGHJK"
    };
    format_user_code(code)
}

// ---------------------------------------------------------------------------
// Lookup

#[tokio::test]
async fn an_owner_looks_up_a_pending_code_typed_any_way_and_is_refunded() {
    let f = fixture().await;
    let s = start(&f.hub).await;
    for typed in [
        s.user_code.clone(),
        s.user_code.to_lowercase(),
        s.user_code.replace('-', ""),
        format!("  {}  ", s.user_code.to_lowercase().replace('-', "")),
    ] {
        let v = ok_json(lookup(&f.hub, &f.owner, &typed).await, StatusCode::OK).await;
        let e = &v["enrollment"];
        let keys: Vec<&String> = e.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "expires_at",
                "instance_key_id",
                "label",
                "requested_at",
                "requested_from",
                "station_id",
                "user_code"
            ],
            "{v}"
        );
        assert_eq!(e["user_code"], json!(s.user_code));
        assert_eq!(e["station_id"], "bench-1");
        assert_eq!(e["label"], "Bench 1");
        assert_eq!(e["instance_key_id"], json!(s.key.public_key_id()));
        assert_eq!(e["requested_from"], "127.0.0.1");
        for t in ["requested_at", "expires_at"] {
            let at = e[t].as_str().unwrap();
            assert!(
                at.len() == 20 && at.ends_with('Z') && at.as_bytes()[10] == b'T',
                "{t} {at}"
            );
        }
        assert!(e["requested_at"].as_str() < e["expires_at"].as_str());
        // No row id and no device-code material.
        let text = v.to_string();
        assert!(!text.contains(&s.id.to_string()), "{text}");
        assert!(!text.contains(&s.device_code), "{text}");
    }
    // Every right code was refunded; nothing changed.
    assert_eq!(failures(&f.hub, &f.owner).await, Some(0));
    assert_eq!(status_of(&f.hub, s.id).await, "pending");
    assert!(f.hub.audited("enrollment.approved").await.is_empty());
}

#[tokio::test]
async fn wrong_expired_and_denied_codes_are_one_404_and_count() {
    let f = fixture().await;
    let s = start(&f.hub).await;
    let expired = start(&f.hub).await;
    sqlx::query("UPDATE enrollments SET expires_at = now() - interval '1 second' WHERE id = $1")
        .bind(expired.id)
        .execute(&f.hub.db.pool)
        .await
        .unwrap();
    let denied = start(&f.hub).await;
    assert_eq!(
        deny(&f.hub, &f.owner, &denied.user_code).await.status(),
        StatusCode::OK
    );
    assert_eq!(failures(&f.hub, &f.owner).await, Some(0));

    let codes = [
        unknown_code(&s.user_code),
        expired.user_code.clone(),
        denied.user_code.clone(),
    ];
    for code in &codes {
        for (route, resp) in [
            ("lookup", lookup(&f.hub, &f.owner, code).await),
            ("approve", approve(&f.hub, &f.owner, code, &f.machine).await),
            ("deny", deny(&f.hub, &f.owner, code).await),
        ] {
            let (status, message) = error_of(resp).await;
            assert_eq!(
                (status, message.as_str()),
                (StatusCode::NOT_FOUND, MSG_NO_ENROLLMENT),
                "{route} {code:?}"
            );
        }
    }
    // Every one a counted failure (one short of the lock, tested below).
    assert_eq!(failures(&f.hub, &f.owner).await, Some(9));
    // Nothing moved: the expired one is still pending (and stays unmatched),
    // the others keep their status.
    assert_eq!(status_of(&f.hub, expired.id).await, "pending");
    assert_eq!(status_of(&f.hub, denied.id).await, "denied");
    // The pending one still matches, refunded.
    assert_eq!(
        lookup(&f.hub, &f.owner, &s.user_code).await.status(),
        StatusCode::OK
    );
    assert_eq!(failures(&f.hub, &f.owner).await, Some(9));
}

#[tokio::test]
async fn approved_and_collected_codes_are_the_same_404() {
    let f = fixture().await;
    let approved = start(&f.hub).await;
    let other = f.hub.verified_owner("Other Org").await;
    let offering = f.hub.published_offering("Other annual").await;
    let theirs = buy(&f.hub, &other, &offering).await;
    assert_eq!(
        approve(&f.hub, &other, &approved.user_code, &theirs)
            .await
            .status(),
        StatusCode::CREATED
    );
    assert_error(
        lookup(&f.hub, &f.owner, &approved.user_code).await,
        StatusCode::NOT_FOUND,
        MSG_NO_ENROLLMENT,
    )
    .await;
    backdate_poll(&f.hub, approved.id).await;
    assert_eq!(poll(&f.hub, &approved).await.status(), StatusCode::OK);
    for resp in [
        lookup(&f.hub, &f.owner, &approved.user_code).await,
        deny(&f.hub, &f.owner, &approved.user_code).await,
    ] {
        assert_error(resp, StatusCode::NOT_FOUND, MSG_NO_ENROLLMENT).await;
    }
    assert_eq!(failures(&f.hub, &f.owner).await, Some(3));
}

#[tokio::test]
async fn malformed_codes_are_the_same_404_and_count() {
    let f = fixture().await;
    for code in ["", "BCDF", "AEIO-UAEI", "BCDF--GHJK", "ＢCDFGHJK"] {
        assert_error(
            lookup(&f.hub, &f.owner, code).await,
            StatusCode::NOT_FOUND,
            MSG_NO_ENROLLMENT,
        )
        .await;
    }
    assert_eq!(failures(&f.hub, &f.owner).await, Some(5));
}

#[tokio::test]
async fn members_vendor_admins_and_strangers_are_refused_before_anything_is_charged() {
    let f = fixture().await;
    let s = start(&f.hub).await;
    let member = f.hub.add(&f.owner, "member").await;
    let admin = bootstrap_admin(&f.hub).await;
    for who in [&member, &admin] {
        for resp in [
            lookup(&f.hub, who, &s.user_code).await,
            approve(&f.hub, who, &s.user_code, &f.machine).await,
            deny(&f.hub, who, &s.user_code).await,
            // Refused before the body is read.
            call(&f.hub, LOOKUP, json!({ "nonsense": true }), who).await,
            call(&f.hub, APPROVE, json!([]), who).await,
        ] {
            assert_error(resp, StatusCode::FORBIDDEN, MSG_OWNER_ONLY_ENROLLMENTS).await;
        }
        assert_eq!(failures(&f.hub, who).await, None);
    }
    assert!(MSG_OWNER_ONLY_ENROLLMENTS.contains("organization owner"));
    // Signed out: 401, still nothing charged.
    for path in [LOOKUP, APPROVE, DENY] {
        let resp = f.hub.post(path, json!({ "code": s.user_code }), None).await;
        assert_no_store(&resp);
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_throttle WHERE key LIKE $1")
        .bind(format!("{ENROLL_USER_KEY_PREFIX}%"))
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    assert_eq!(status_of(&f.hub, s.id).await, "pending");
}

#[tokio::test]
async fn owner_routes_need_the_csrf_token_and_charge_nothing_without_it() {
    let f = fixture().await;
    let s = start(&f.hub).await;
    for (path, body) in [
        (LOOKUP, json!({ "code": s.user_code })),
        (
            APPROVE,
            json!({ "code": s.user_code, "entitlement_id": f.machine }),
        ),
        (DENY, json!({ "code": s.user_code })),
    ] {
        let resp = f
            .hub
            .http
            .post(f.hub.url(path))
            .header(reqwest::header::COOKIE, &f.owner.cookie)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_error(resp, StatusCode::FORBIDDEN, CSRF_ERROR).await;
    }
    assert_eq!(failures(&f.hub, &f.owner).await, None);
    assert_eq!(status_of(&f.hub, s.id).await, "pending");
}

#[tokio::test]
async fn an_owners_malformed_body_is_400_and_charges_nothing() {
    let f = fixture().await;
    let s = start(&f.hub).await;
    for (path, body) in [
        (LOOKUP, json!({ "nonsense": true })),
        (LOOKUP, json!({ "code": 7 })),
        (
            APPROVE,
            json!({ "code": s.user_code, "entitlement_id": "not-a-uuid" }),
        ),
        (APPROVE, json!({ "code": s.user_code })),
        (DENY, json!([])),
    ] {
        let resp = call(&f.hub, path, body, &f.owner).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{path}");
    }
    // The body is read before the charge.
    assert_eq!(failures(&f.hub, &f.owner).await, None);
    assert_eq!(status_of(&f.hub, s.id).await, "pending");
}

// ---------------------------------------------------------------------------
// Approve

#[tokio::test]
async fn approval_registers_the_station_and_the_station_collects_a_licence_for_its_key() {
    let f = fixture().await;
    let s = start(&f.hub).await;
    let before = f.hub.actions().await.len();
    let v = ok_json(
        approve(&f.hub, &f.owner, &s.user_code.to_lowercase(), &f.machine).await,
        StatusCode::CREATED,
    )
    .await;
    // The `POST /api/stations` shape.
    let station = &v["station"];
    let licence = &v["licence"];
    assert_eq!(station["instance_key_id"], json!(s.key.public_key_id()));
    assert_eq!(station["station_id"], "bench-1");
    assert_eq!(station["label"], "Bench 1");
    assert_eq!(station["site"], Value::Null);
    assert_eq!(station["entitlement_id"], json!(f.machine));
    assert_eq!(station["licence"]["id"], licence["id"]);
    assert_eq!(licence["status"], "active");
    let station_id: Uuid = station["id"].as_str().unwrap().parse().unwrap();
    let licence_id: Uuid = licence["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(failures(&f.hub, &f.owner).await, Some(0));

    // The enrollment is approved with every approval column.
    let row: (
        String,
        Option<Uuid>,
        bool,
        Option<Uuid>,
        Option<Uuid>,
        Option<Uuid>,
    ) = sqlx::query_as(
        "SELECT status, approved_by, approved_at IS NOT NULL, organization_id, \
                    registered_station, licence_id \
             FROM enrollments WHERE id = $1",
    )
    .bind(s.id)
    .fetch_one(&f.hub.db.pool)
    .await
    .unwrap();
    let owner_id: Uuid = f.owner.id.parse().unwrap();
    assert_eq!(
        row,
        (
            "approved".to_string(),
            Some(owner_id),
            true,
            Some(f.org_id),
            Some(station_id),
            Some(licence_id)
        )
    );

    // Three audit rows, in order, in the approval's transaction.
    let actions = f.hub.actions().await;
    assert_eq!(
        &actions[before..],
        [
            "station.registered",
            "licence.issued",
            "enrollment.approved"
        ]
    );
    let approved = f.hub.audited("enrollment.approved").await;
    assert_eq!(
        approved,
        vec![(
            format!("user:{}", f.owner.id),
            format!("enrollment:{}", s.id),
            json!({
                "station": station_id,
                "licence": licence_id,
                "entitlement_id": f.machine,
                "instance_key_id": s.key.public_key_id(),
            })
        )]
    );

    // The station's next poll collects it, and the licence installs on its
    // key against the root it trusts.
    backdate_poll(&f.hub, s.id).await;
    let resp = poll(&f.hub, &s).await;
    let got = ok_json(resp, StatusCode::OK).await;
    assert_eq!(got["station"]["id"], json!(station_id.to_string()));
    let file = parse_license(&got["licence"].to_string()).unwrap();
    check_installable(&file, &s.key.public_key_id(), &[f.root.verifying_key()]).unwrap();
    assert_eq!(file.body.instance_public_key_id, s.key.public_key_id());
    assert_eq!(file.body.license_id, licence_id.to_string());
    assert!(check_installable(
        &file,
        &s.key.public_key_id(),
        &[VendorRoot::generate().verifying_key()]
    )
    .is_err());
    assert_eq!(status_of(&f.hub, s.id).await, "collected");

    // The code is spent.
    assert_error(
        lookup(&f.hub, &f.owner, &s.user_code).await,
        StatusCode::NOT_FOUND,
        MSG_NO_ENROLLMENT,
    )
    .await;
}

#[tokio::test]
async fn every_register_refusal_comes_back_unchanged_and_leaves_the_enrollment_pending() {
    let f = fixture().await;
    let pool = &f.hub.db.pool;

    // Key already registered in this organization (through the portal),
    // against the machine seat: also leaves no free seat.
    let registered = start(&f.hub).await;
    let resp = f
        .hub
        .post(
            "/api/stations",
            json!({
                "entitlement_id": f.machine,
                "instance_key_id": registered.key.public_key_id(),
                "station_id": "bench-1",
            }),
            Some(&f.owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let second_offering = f.hub.published_offering("Second annual").await;
    let second = buy(&f.hub, &f.owner, &second_offering).await;

    let s = start(&f.hub).await;
    // Another organization's current entitlement.
    let other = f.hub.verified_owner("Other Org").await;
    let other_offering = f.hub.published_offering("Other annual").await;
    let foreign = buy(&f.hub, &other, &other_offering).await;
    // A refunded entitlement is not current.
    let refunded = buy(&f.hub, &f.owner, &second_offering).await;
    sqlx::query(
        "UPDATE entitlements SET status = 'refunded', refunded_at = now(), renews = false \
         WHERE id = $1::uuid",
    )
    .bind(&refunded)
    .execute(pool)
    .await
    .unwrap();

    let stations_before = f.hub.count("stations").await;
    let licences_before = f.hub.count("licences").await;
    let actions_before = f.hub.actions().await.len();
    let cases: Vec<(&Started, String, StatusCode, &str)> = vec![
        (
            &s,
            f.machine.clone(),
            StatusCode::CONFLICT,
            MSG_NO_FREE_SEAT,
        ),
        (
            &s,
            refunded.clone(),
            StatusCode::CONFLICT,
            MSG_ENTITLEMENT_NOT_CURRENT,
        ),
        (
            &s,
            foreign.clone(),
            StatusCode::NOT_FOUND,
            MSG_NO_ENTITLEMENT,
        ),
        (
            &s,
            Uuid::new_v4().to_string(),
            StatusCode::NOT_FOUND,
            MSG_NO_ENTITLEMENT,
        ),
        (
            &registered,
            second.clone(),
            StatusCode::CONFLICT,
            MSG_ALREADY_REGISTERED,
        ),
    ];
    for (who, entitlement, code, message) in cases {
        assert_error(
            approve(&f.hub, &f.owner, &who.user_code, &entitlement).await,
            code,
            message,
        )
        .await;
        assert_eq!(status_of(&f.hub, who.id).await, "pending", "{message}");
    }
    assert_eq!(f.hub.count("stations").await, stations_before);
    assert_eq!(f.hub.count("licences").await, licences_before);
    assert_eq!(
        f.hub.actions().await.len(),
        actions_before,
        "nothing audited"
    );
    // A right code is refunded even when the approval is refused.
    assert_eq!(failures(&f.hub, &f.owner).await, Some(0));
    // The other organization's entitlement was untouched.
    let foreign_stations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM stations WHERE entitlement_id = $1::uuid")
            .bind(&foreign)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(foreign_stations, 0);

    // The station still polls pending, and a good approval still works.
    let resp = poll(&f.hub, &s).await;
    assert_eq!(
        error_of(resp).await,
        (StatusCode::BAD_REQUEST, "authorization_pending".into())
    );
    assert_eq!(
        approve(&f.hub, &f.owner, &s.user_code, &second)
            .await
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(status_of(&f.hub, s.id).await, "approved");
}

#[tokio::test]
async fn without_a_vendor_root_approval_is_503_after_the_owner_check() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Rootless").await;
    let member = hub.add(&owner, "member").await;
    let s = start(&hub).await;
    let entitlement = Uuid::new_v4().to_string();
    assert_error(
        approve(&hub, &member, &s.user_code, &entitlement).await,
        StatusCode::FORBIDDEN,
        MSG_OWNER_ONLY_ENROLLMENTS,
    )
    .await;
    assert_error(
        approve(&hub, &owner, &s.user_code, &entitlement).await,
        StatusCode::SERVICE_UNAVAILABLE,
        MSG_ISSUANCE_UNAVAILABLE,
    )
    .await;
    // Before the body is read, and nothing charged.
    assert_error(
        call(&hub, APPROVE, json!("not a body"), &owner).await,
        StatusCode::SERVICE_UNAVAILABLE,
        MSG_ISSUANCE_UNAVAILABLE,
    )
    .await;
    assert_eq!(failures(&hub, &owner).await, None);
    assert_eq!(status_of(&hub, s.id).await, "pending");
    // Lookup and deny need no root.
    assert_eq!(
        lookup(&hub, &owner, &s.user_code).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        deny(&hub, &owner, &s.user_code).await.status(),
        StatusCode::OK
    );
}

// ---------------------------------------------------------------------------
// Deny

#[tokio::test]
async fn denial_is_audited_and_the_station_hears_access_denied() {
    let f = fixture().await;
    let s = start(&f.hub).await;
    let before = f.hub.actions().await.len();
    let v = ok_json(deny(&f.hub, &f.owner, &s.user_code).await, StatusCode::OK).await;
    assert_eq!(v, json!({}));
    assert_eq!(status_of(&f.hub, s.id).await, "denied");
    assert_eq!(&f.hub.actions().await[before..], ["enrollment.denied"]);
    assert_eq!(
        f.hub.audited("enrollment.denied").await,
        vec![(
            format!("user:{}", f.owner.id),
            format!("enrollment:{}", s.id),
            json!({ "instance_key_id": s.key.public_key_id(), "station_id": "bench-1" })
        )]
    );
    assert_eq!(f.hub.count("stations").await, 0);
    assert_eq!(failures(&f.hub, &f.owner).await, Some(0));

    let resp = poll(&f.hub, &s).await;
    assert_eq!(
        error_of(resp).await,
        (StatusCode::BAD_REQUEST, "access_denied".into())
    );
    // Spent: approving it now is the one 404.
    assert_error(
        approve(&f.hub, &f.owner, &s.user_code, &f.machine).await,
        StatusCode::NOT_FOUND,
        MSG_NO_ENROLLMENT,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Another organization

#[tokio::test]
async fn another_organizations_owner_may_look_up_approve_into_their_own_or_deny() {
    let f = fixture().await;
    let other = f.hub.verified_owner("Other Org").await;
    let other_org = org_of(&f.hub, &other).await;
    let offering = f.hub.published_offering("Other annual").await;
    let theirs = buy(&f.hub, &other, &offering).await;

    // Approve: the station lands in the approver's organization only.
    let s = start(&f.hub).await;
    assert_eq!(
        lookup(&f.hub, &other, &s.user_code).await.status(),
        StatusCode::OK
    );
    // Not with Acme's entitlement: that is another organization's to them.
    assert_error(
        approve(&f.hub, &other, &s.user_code, &f.machine).await,
        StatusCode::NOT_FOUND,
        MSG_NO_ENTITLEMENT,
    )
    .await;
    let v = ok_json(
        approve(&f.hub, &other, &s.user_code, &theirs).await,
        StatusCode::CREATED,
    )
    .await;
    let org: Uuid = sqlx::query_scalar("SELECT organization_id FROM stations WHERE id = $1::uuid")
        .bind(v["station"]["id"].as_str().unwrap())
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(org, other_org);
    let enrolled: Option<Uuid> =
        sqlx::query_scalar("SELECT organization_id FROM enrollments WHERE id = $1")
            .bind(s.id)
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(enrolled, Some(other_org));
    // Acme sees no station.
    let listed = ok_json(
        f.hub.get("/api/stations", Some(&f.owner.cookie)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed["stations"], json!([]));

    // Deny.
    let d = start(&f.hub).await;
    assert_eq!(
        deny(&f.hub, &other, &d.user_code).await.status(),
        StatusCode::OK
    );
    assert_eq!(status_of(&f.hub, d.id).await, "denied");
}

// ---------------------------------------------------------------------------
// Throttle

#[tokio::test]
async fn the_eleventh_wrong_code_in_fifteen_minutes_is_locked_and_right_codes_are_refunded() {
    let f = fixture().await;
    let s = start(&f.hub).await;
    let wrong = unknown_code(&s.user_code);
    let limit = ENROLL_LOOKUP_LIMIT as usize;
    assert_eq!(limit, 10);

    for _ in 0..limit - 1 {
        assert_error(
            lookup(&f.hub, &f.owner, &wrong).await,
            StatusCode::NOT_FOUND,
            MSG_NO_ENROLLMENT,
        )
        .await;
    }
    // Right codes in between never use up the allowance.
    for _ in 0..5 {
        assert_eq!(
            lookup(&f.hub, &f.owner, &s.user_code).await.status(),
            StatusCode::OK
        );
    }
    assert_eq!(failures(&f.hub, &f.owner).await, Some(9));
    assert!(f.hub.audited("enrollment.lookup_locked").await.is_empty());
    // The tenth failure locks.
    assert_error(
        deny(&f.hub, &f.owner, &wrong).await,
        StatusCode::NOT_FOUND,
        MSG_NO_ENROLLMENT,
    )
    .await;
    let locked = f.hub.audited("enrollment.lookup_locked").await;
    assert_eq!(locked.len(), 1);
    assert_eq!(locked[0].0, format!("user:{}", f.owner.id));
    assert_eq!(locked[0].1, user_key(&f.owner));
    assert!(locked[0].2["until"].as_str().unwrap().ends_with('Z'));

    // Then every route is 429 with Retry-After, even with the right code.
    for resp in [
        lookup(&f.hub, &f.owner, &wrong).await,
        lookup(&f.hub, &f.owner, &s.user_code).await,
        approve(&f.hub, &f.owner, &s.user_code, &f.machine).await,
        deny(&f.hub, &f.owner, &s.user_code).await,
    ] {
        let retry: u64 = resp.headers()[RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=15 * 60).contains(&retry), "{retry}");
        assert_error(resp, StatusCode::TOO_MANY_REQUESTS, MSG_LOOKUP_LOCKED).await;
    }
    assert_eq!(status_of(&f.hub, s.id).await, "pending");
    // Audited once, however hard it is hammered.
    assert_eq!(f.hub.audited("enrollment.lookup_locked").await.len(), 1);

    // Another owner is not locked out by this one.
    let other = f.hub.verified_owner("Other Org").await;
    assert_eq!(
        lookup(&f.hub, &other, &s.user_code).await.status(),
        StatusCode::OK
    );

    // Once the lock lapses, the next charge restarts the window and
    // summarises the refusals.
    sqlx::query(
        "UPDATE auth_throttle SET locked_until = now() - interval '1 second' WHERE key = $1",
    )
    .bind(user_key(&f.owner))
    .execute(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(
        lookup(&f.hub, &f.owner, &s.user_code).await.status(),
        StatusCode::OK
    );
    let summaries = f.hub.audited("enrollment.lookup_lock_summary").await;
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].1, user_key(&f.owner));
    assert_eq!(summaries[0].2["refused"], 4);
    assert!(f
        .hub
        .audited("enrollment.start_lock_summary")
        .await
        .is_empty());
}

/// Approval on a credit-pool entitlement issues the bare licence and moves
/// no credits (controlled-licensing story 3): its response carries only the
/// licence, so a grant nobody receives would strand credits.
#[tokio::test]
async fn approval_on_a_credit_pool_grants_nothing() {
    let f = fixture().await;
    let one_off = f.hub.published_one_off_offering("Site pool", 12).await;
    let pool = buy(&f.hub, &f.owner, &one_off).await;
    let s = start(&f.hub).await;
    let v = ok_json(
        approve(&f.hub, &f.owner, &s.user_code, &pool).await,
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(v["station"]["credits_granted"], 0);
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
    // The station collects the bare licence.
    backdate_poll(&f.hub, s.id).await;
    let got = ok_json(poll(&f.hub, &s).await, StatusCode::OK).await;
    let file = parse_license(&got["licence"].to_string()).unwrap();
    check_installable(&file, &s.key.public_key_id(), &[f.root.verifying_key()]).unwrap();
}
