//! Catalog, checkout, orders, billing and the Fake provider's checkout page
//! (offerings-and-payments story 2, CAP-2 and CAP-5). One test per row of
//! the story's I/O matrix, through HTTP, checking the response, the stored
//! row and the audit rows. Needs `docker compose up -d`; fails, never skips,
//! when it is down.

mod support;

use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{assert_error, bootstrap_admin, spawn_hub, spawn_hub_args, Hub, Who};
use support::stripe_mock::{spawn_stripe_hub, StripeMock};
use uuid::Uuid;
use wipe_hub::entitlements::renewal_notice_now;
use wipe_hub::order_routes::{
    MSG_CHECKOUT_ENDED, MSG_NOT_FOR_SALE, MSG_NO_CHECKOUT, MSG_NO_ORDER, MSG_OWNER_ONLY_BILLING,
    MSG_OWNER_ONLY_BUY, MSG_PROVIDER_FAILED, MSG_UNVERIFIED_BUYER,
};
use wipe_hub::orders::ApplyOutcome;
use wipe_hub::payments::{
    Fake, PaymentProvider as _, FAKE_CHECKOUT_COMPLETED, FAKE_CHECKOUT_EXPIRED, MSG_FAKE_LOCAL_ONLY,
};

const NOT_SIGNED_IN: &str = "not signed in";

/// A hub with one organization: a verified owner and a member.
struct Fixture {
    hub: Hub,
    owner: Who,
    member: Who,
}

async fn fixture_on(hub: Hub) -> Fixture {
    let owner = hub.verified_owner("Acme Recyclers").await;
    let member = hub.add(&owner, "member").await;
    Fixture { hub, owner, member }
}

async fn fixture() -> Fixture {
    fixture_on(spawn_hub().await).await
}

async fn ok_json(resp: Response) -> Value {
    let code = resp.status();
    let v: Value = resp.json().await.unwrap();
    assert_eq!(code, StatusCode::OK, "{v}");
    v
}

/// The `session` query parameter of a Fake checkout URL.
fn session_of(checkout_url: &str) -> String {
    let url = url::Url::parse(checkout_url).unwrap();
    url.query_pairs()
        .find(|(k, _)| k == "session")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| panic!("no session in {checkout_url}"))
}

impl Fixture {
    async fn checkout(&self, who: &Who, offering_id: &str) -> Response {
        self.hub
            .post(
                "/api/checkout",
                json!({ "offering_id": offering_id }),
                Some(&who.cookie),
            )
            .await
    }

    /// A pending order bought by the owner: `(order, session id)`.
    async fn bought(&self, offering_id: &str) -> (Value, String) {
        let resp = self.checkout(&self.owner, offering_id).await;
        let code = resp.status();
        let v: Value = resp.json().await.unwrap();
        assert_eq!(code, StatusCode::CREATED, "{v}");
        let session = session_of(v["checkout_url"].as_str().unwrap());
        (v["order"].clone(), session)
    }

    async fn fake(&self, who: &Who, session: &str, action: &str) -> Response {
        self.hub
            .post_empty(
                &format!("/api/payments/fake/sessions/{session}/{action}"),
                Some(&who.cookie),
            )
            .await
    }

    async fn stored(&self, id: &str) -> Value {
        sqlx::query_scalar("SELECT row_to_json(o) FROM orders o WHERE id = $1::uuid")
            .bind(id)
            .fetch_one(&self.hub.db.pool)
            .await
            .unwrap()
    }

    async fn audit_count(&self) -> i64 {
        self.hub.count("audit_log").await
    }

    /// `(action, actor, detail)` of every audit row about order `id`.
    async fn order_audits(&self, id: &str) -> Vec<(String, String, Value)> {
        sqlx::query_as("SELECT action, actor, detail FROM audit_log WHERE subject = $1 ORDER BY id")
            .bind(format!("order:{id}"))
            .fetch_all(&self.hub.db.pool)
            .await
            .unwrap()
    }
}

/// An offering with `status` (`draft`, `published` or `retired`): its id.
async fn insert_offering(hub: &Hub, name: &str, status: &str) -> String {
    if status == "draft" {
        return sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO offerings (name, price_minor, currency, billing_interval, scope, \
                                    seats, term_months) \
             VALUES ($1, 500, 'USD', 'one_off', 'site', 1, 1) RETURNING id",
        )
        .bind(name)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
        .to_string();
    }
    let id = hub.published_offering(name).await;
    if status == "retired" {
        sqlx::query(
            "UPDATE offerings SET status = 'retired', retired_at = now() WHERE id = $1::uuid",
        )
        .bind(&id)
        .execute(&hub.db.pool)
        .await
        .unwrap();
    }
    id
}

// ---------------------------------------------------------------------------
// Catalog

#[tokio::test]
async fn the_catalog_lists_published_offerings_to_signed_in_users() {
    let f = fixture().await;
    let zeta = f.hub.published_offering("zeta plan").await;
    let alpha = f.hub.published_offering("Alpha plan").await;
    let one_off = f.hub.published_one_off_offering("Middle plan", 24).await;
    insert_offering(&f.hub, "Retired plan", "retired").await;
    insert_offering(&f.hub, "Draft plan", "draft").await;

    let resp = f.hub.get("/api/offerings", None).await;
    assert_error(resp, StatusCode::UNAUTHORIZED, NOT_SIGNED_IN).await;

    for who in [&f.owner, &f.member] {
        let v = ok_json(f.hub.get("/api/offerings", Some(&who.cookie)).await).await;
        let list = v["offerings"].as_array().unwrap();
        let ids: Vec<&str> = list.iter().map(|o| o["id"].as_str().unwrap()).collect();
        assert_eq!(ids, [alpha.as_str(), one_off.as_str(), zeta.as_str()]);
        // An annual offering carries the auto-renewal notice, dated today; a
        // one-off carries none (story 4).
        let notice = renewal_notice_now("annual", 120000, "USD").unwrap();
        assert_eq!(
            notice.headline,
            "Renews automatically every year at 1,200.00 USD until you cancel."
        );
        assert_eq!(list[1]["renewal_notice"], Value::Null);
        assert_eq!(list[1]["interval"], "one_off");
        assert_eq!(
            list[0],
            json!({
                "id": alpha,
                "name": "Alpha plan",
                "description": "One station.",
                "price_minor": 120000,
                "currency": "USD",
                "interval": "annual",
                "grants": {
                    "scope": "machine",
                    "seats": 1,
                    "quota_erasures": null,
                    "features": ["hub_sync", "pdf_certificates"],
                    "term_months": 12
                },
                "renewal_notice": {"headline": notice.headline, "detail": notice.detail},
            })
        );
    }
}

// ---------------------------------------------------------------------------
// Checkout

#[tokio::test]
async fn a_verified_owner_checks_out_and_the_terms_are_snapshotted() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station annual").await;
    let resp = f.checkout(&f.owner, &offering).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = resp.json().await.unwrap();
    let url = v["checkout_url"].as_str().unwrap();
    let session = session_of(url);
    assert!(session.starts_with("fake_cs_"), "{session}");
    assert_eq!(
        url,
        format!("http://127.0.0.1:7900/checkout/fake?session={session}")
    );

    let order = &v["order"];
    let id = order["id"].as_str().unwrap().to_string();
    let keys: Vec<&String> = order.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "created_at",
            "currency",
            "expired_at",
            "grants",
            "id",
            "interval",
            "offering_id",
            "offering_name",
            "paid_at",
            "price_minor",
            "provider",
            "refunded_at",
            "status",
            "updated_at"
        ]
    );
    assert_eq!(order["offering_id"], json!(offering));
    assert_eq!(order["offering_name"], "Station annual");
    assert_eq!(order["price_minor"], 120000);
    assert_eq!(order["currency"], "USD");
    assert_eq!(order["interval"], "annual");
    assert_eq!(
        order["grants"]["features"],
        json!(["hub_sync", "pdf_certificates"])
    );
    assert_eq!(order["status"], "pending");
    assert_eq!(order["provider"], "fake");
    assert_eq!(order["paid_at"], Value::Null);
    assert_eq!(order["expired_at"], Value::Null);
    assert!(order["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(!v.to_string().contains(&format!("\"{session}\"")));

    let row = f.stored(&id).await;
    assert_eq!(row["provider_session_id"], json!(session));
    assert_eq!(row["created_by"], json!(f.owner.id));
    assert_eq!(row["status"], "pending");
    assert_eq!(row["scope"], "machine");
    assert_eq!(row["seats"], 1);
    assert_eq!(row["term_months"], 12);

    let audits = f.order_audits(&id).await;
    assert_eq!(
        audits,
        [(
            "order.created".to_string(),
            format!("user:{}", f.owner.id),
            json!({
                "offering_id": offering,
                "price_minor": 120000,
                "currency": "USD",
                "provider": "fake"
            })
        )]
    );
}

#[tokio::test]
async fn refused_checkouts_write_nothing() {
    let f = fixture().await;
    let admin = bootstrap_admin(&f.hub).await;
    let unverified = f
        .hub
        .sign_up(&support::unique_address(), "Unverified Ltd")
        .await;
    let published = f.hub.published_offering("For sale").await;
    let draft = insert_offering(&f.hub, "Not yet", "draft").await;
    let retired = insert_offering(&f.hub, "Gone", "retired").await;
    let before = (f.hub.counts().await, f.hub.count("orders").await);

    for who in [&f.member, &admin] {
        assert_error(
            f.checkout(who, &published).await,
            StatusCode::FORBIDDEN,
            MSG_OWNER_ONLY_BUY,
        )
        .await;
    }
    assert_error(
        f.checkout(&unverified, &published).await,
        StatusCode::FORBIDDEN,
        MSG_UNVERIFIED_BUYER,
    )
    .await;
    for id in [Uuid::new_v4().to_string(), draft, retired] {
        assert_error(
            f.checkout(&f.owner, &id).await,
            StatusCode::NOT_FOUND,
            MSG_NOT_FOR_SALE,
        )
        .await;
    }
    for body in [
        json!({"offering_id": "not-a-uuid"}),
        json!({}),
        json!({"offering_id": published, "price_minor": 1}),
    ] {
        let resp = f
            .hub
            .post("/api/checkout", body.clone(), Some(&f.owner.cookie))
            .await;
        let code = resp.status();
        let v: Value = resp.json().await.unwrap();
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body} {v}");
        assert!(v["error"].is_string(), "{v}");
    }
    let resp = f
        .hub
        .post("/api/checkout", json!({"offering_id": published}), None)
        .await;
    assert_error(resp, StatusCode::UNAUTHORIZED, NOT_SIGNED_IN).await;

    // Bootstrapping the admin wrote rows before `before`; nothing since.
    assert_eq!((f.hub.counts().await, f.hub.count("orders").await), before);
}

#[tokio::test]
async fn a_failing_provider_answers_502_and_writes_nothing() {
    // Stripe, pointed at a local mock that answers 500 (story 3).
    let mock = StripeMock::start().await;
    let f = fixture_on(spawn_stripe_hub(&mock).await).await;
    let offering = f.hub.published_offering("Station").await;
    let before = f.audit_count().await;
    mock.reply_next(500, json!({"error": {"type": "api_error"}}));
    assert_error(
        f.checkout(&f.owner, &offering).await,
        StatusCode::BAD_GATEWAY,
        MSG_PROVIDER_FAILED,
    )
    .await;
    assert_eq!(mock.requests().len(), 1);
    // The Fake page does not exist under another provider.
    for (method, path) in [
        ("GET", "/api/payments/fake/sessions/fake_cs_x"),
        ("POST", "/api/payments/fake/sessions/fake_cs_x/pay"),
        ("POST", "/api/payments/fake/sessions/fake_cs_x/abandon"),
    ] {
        let resp = if method == "GET" {
            f.hub.get(path, Some(&f.owner.cookie)).await
        } else {
            f.hub.post_empty(path, Some(&f.owner.cookie)).await
        };
        assert_error(resp, StatusCode::NOT_FOUND, MSG_NO_CHECKOUT).await;
    }
    assert_eq!(f.hub.count("orders").await, 0);
    assert_eq!(f.audit_count().await, before);
}

#[tokio::test]
async fn the_fake_provider_refuses_a_non_local_hub() {
    let hub = spawn_hub_args(|a| a.public_base_url = "https://hub.example.com".into()).await;
    // No member: invitation links would point at the non-local host.
    let owner = hub.verified_owner("Acme Recyclers").await;
    let f = Fixture {
        member: owner.clone(),
        owner,
        hub,
    };
    let offering = f.hub.published_offering("Station").await;
    let before = f.audit_count().await;
    assert_error(
        f.checkout(&f.owner, &offering).await,
        StatusCode::SERVICE_UNAVAILABLE,
        MSG_FAKE_LOCAL_ONLY,
    )
    .await;
    let resp = f
        .hub
        .post_empty("/api/billing/portal", Some(&f.owner.cookie))
        .await;
    assert_error(resp, StatusCode::SERVICE_UNAVAILABLE, MSG_FAKE_LOCAL_ONLY).await;
    assert_eq!(f.hub.count("orders").await, 0);

    // A pending Fake order that got here somehow still cannot be paid.
    let org: Uuid = sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1::uuid")
        .bind(&f.owner.id)
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO orders (organization_id, offering_id, offering_name, price_minor, currency, \
                             billing_interval, scope, seats, term_months, provider, \
                             provider_session_id) \
         VALUES ($1, $2::uuid, 'Station', 120000, 'USD', 'annual', 'machine', 1, 12, 'fake', \
                 'fake_cs_planted')",
    )
    .bind(org)
    .bind(&offering)
    .execute(&f.hub.db.pool)
    .await
    .unwrap();
    for action in ["pay", "abandon"] {
        assert_error(
            f.fake(&f.owner, "fake_cs_planted", action).await,
            StatusCode::SERVICE_UNAVAILABLE,
            MSG_FAKE_LOCAL_ONLY,
        )
        .await;
    }
    let status: String = sqlx::query_scalar(
        "SELECT status FROM orders WHERE provider_session_id = 'fake_cs_planted'",
    )
    .fetch_one(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(f.audit_count().await, before);
}

// ---------------------------------------------------------------------------
// Orders

#[tokio::test]
async fn every_role_sees_its_own_organizations_orders_only() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station").await;
    let (first, _) = f.bought(&offering).await;
    let (second, _) = f.bought(&offering).await;

    let rival = f.hub.verified_owner("Rival Inc").await;
    let resp = f.checkout(&rival, &offering).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let rival_order: Value = resp.json().await.unwrap();
    let rival_id = rival_order["order"]["id"].as_str().unwrap().to_string();

    for who in [&f.owner, &f.member] {
        let v = ok_json(f.hub.get("/api/orders", Some(&who.cookie)).await).await;
        let mut ids: Vec<String> = v["orders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids.len(), 2, "{v}");
        ids.sort();
        let mut want = vec![
            first["id"].as_str().unwrap().to_string(),
            second["id"].as_str().unwrap().to_string(),
        ];
        want.sort();
        assert_eq!(ids, want);

        let path = format!("/api/orders/{}", second["id"].as_str().unwrap());
        let v = ok_json(f.hub.get(&path, Some(&who.cookie)).await).await;
        assert_eq!(v["order"], second);

        for bad in [rival_id.clone(), Uuid::new_v4().to_string(), "nope".into()] {
            let resp = f
                .hub
                .get(&format!("/api/orders/{bad}"), Some(&who.cookie))
                .await;
            assert_error(resp, StatusCode::NOT_FOUND, MSG_NO_ORDER).await;
        }
    }
    let resp = f.hub.get("/api/orders", None).await;
    assert_error(resp, StatusCode::UNAUTHORIZED, NOT_SIGNED_IN).await;
}

#[tokio::test]
async fn orders_are_listed_newest_first() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station").await;
    let (first, _) = f.bought(&offering).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (second, _) = f.bought(&offering).await;
    let v = ok_json(f.hub.get("/api/orders", Some(&f.owner.cookie)).await).await;
    let ids: Vec<&Value> = v["orders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| &o["id"])
        .collect();
    assert_eq!(ids, [&second["id"], &first["id"]]);
}

// ---------------------------------------------------------------------------
// The Fake checkout page

#[tokio::test]
async fn the_fake_page_reads_the_owners_checkout_only() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station").await;
    let (order, session) = f.bought(&offering).await;
    let path = format!("/api/payments/fake/sessions/{session}");
    let v = ok_json(f.hub.get(&path, Some(&f.owner.cookie)).await).await;
    let notice = renewal_notice_now("annual", 120000, "USD").unwrap();
    assert_eq!(
        v,
        json!({ "order": order, "renewal_notice": {"headline": notice.headline, "detail": notice.detail} })
    );
    // A one-off checkout has no notice.
    let one_off = f.hub.published_one_off_offering("Station once", 24).await;
    let (_, once) = f.bought(&one_off).await;
    let v = ok_json(
        f.hub
            .get(
                &format!("/api/payments/fake/sessions/{once}"),
                Some(&f.owner.cookie),
            )
            .await,
    )
    .await;
    assert_eq!(v["renewal_notice"], Value::Null);
    assert_eq!(v["order"]["interval"], "one_off");

    let resp = f.hub.get(&path, Some(&f.member.cookie)).await;
    assert_error(resp, StatusCode::FORBIDDEN, MSG_OWNER_ONLY_BUY).await;
    let rival = f.hub.verified_owner("Rival Inc").await;
    let resp = f.hub.get(&path, Some(&rival.cookie)).await;
    assert_error(resp, StatusCode::NOT_FOUND, MSG_NO_CHECKOUT).await;
    let resp = f
        .hub
        .get(
            "/api/payments/fake/sessions/fake_cs_unknown",
            Some(&f.owner.cookie),
        )
        .await;
    assert_error(resp, StatusCode::NOT_FOUND, MSG_NO_CHECKOUT).await;
}

#[tokio::test]
async fn paying_marks_the_order_paid_through_a_verified_event() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station").await;
    let (order, session) = f.bought(&offering).await;
    let id = order["id"].as_str().unwrap().to_string();

    // Only the owner of this organization may pay.
    let before = f.audit_count().await;
    assert_error(
        f.fake(&f.member, &session, "pay").await,
        StatusCode::FORBIDDEN,
        MSG_OWNER_ONLY_BUY,
    )
    .await;
    let rival = f.hub.verified_owner("Rival Inc").await;
    let before_rival = f.audit_count().await;
    assert_error(
        f.fake(&rival, &session, "pay").await,
        StatusCode::NOT_FOUND,
        MSG_NO_CHECKOUT,
    )
    .await;
    assert_eq!(f.audit_count().await, before_rival);
    assert!(before_rival >= before);

    let v = ok_json(f.fake(&f.owner, &session, "pay").await).await;
    let paid = &v["order"];
    assert_eq!(paid["id"], json!(id));
    assert_eq!(paid["status"], "paid");
    assert!(paid["paid_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(paid["expired_at"], Value::Null);
    assert_eq!(paid["offering_name"], order["offering_name"]);

    let audits = f.order_audits(&id).await;
    assert_eq!(audits.len(), 2, "{audits:?}");
    assert_eq!(audits[0].0, "order.created");
    assert_eq!(audits[1].0, "order.paid");
    assert_eq!(audits[1].1, "provider:fake");
    let detail = audits[1].2.as_object().unwrap();
    assert_eq!(detail.len(), 1);
    assert!(detail["event_id"]
        .as_str()
        .unwrap()
        .starts_with("fake_evt_"));
    // The event went through the event store (story 3).
    let stored: (String, String, String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT provider, event_id, event_type, session_id, outcome FROM payment_events",
    )
    .fetch_one(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        (
            "fake".into(),
            detail["event_id"].as_str().unwrap().to_string(),
            FAKE_CHECKOUT_COMPLETED.into(),
            Some(session.clone()),
            Some("applied".into())
        )
    );

    // Owner and member both see it paid.
    for who in [&f.owner, &f.member] {
        let v = ok_json(f.hub.get("/api/orders", Some(&who.cookie)).await).await;
        assert_eq!(v["orders"][0]["status"], "paid");
    }

    // A second pay, or an abandon, is refused and writes nothing.
    let before = f.audit_count().await;
    for action in ["pay", "abandon"] {
        assert_error(
            f.fake(&f.owner, &session, action).await,
            StatusCode::CONFLICT,
            MSG_CHECKOUT_ENDED,
        )
        .await;
    }
    assert_eq!(f.audit_count().await, before);
    assert_eq!(f.hub.audited("order.paid").await.len(), 1);
    assert_eq!(f.stored(&id).await["status"], "paid");
}

#[tokio::test]
async fn abandoning_expires_the_order() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station").await;
    let (order, session) = f.bought(&offering).await;
    let id = order["id"].as_str().unwrap().to_string();
    let v = ok_json(f.fake(&f.owner, &session, "abandon").await).await;
    assert_eq!(v["order"]["status"], "expired");
    assert!(v["order"]["expired_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(v["order"]["paid_at"], Value::Null);
    let audits = f.order_audits(&id).await;
    let actions: Vec<&str> = audits.iter().map(|a| a.0.as_str()).collect();
    assert_eq!(actions, ["order.created", "order.expired"]);
    assert_eq!(audits[1].1, "provider:fake");
    assert!(audits[1].2["event_id"]
        .as_str()
        .unwrap()
        .starts_with("fake_evt_"));
    assert_error(
        f.fake(&f.owner, &session, "pay").await,
        StatusCode::CONFLICT,
        MSG_CHECKOUT_ENDED,
    )
    .await;
    assert_eq!(f.stored(&id).await["status"], "expired");
}

#[tokio::test]
async fn concurrent_pays_give_one_200_and_one_409() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station").await;
    let (order, session) = f.bought(&offering).await;
    let (a, b) = tokio::join!(
        f.fake(&f.owner, &session, "pay"),
        f.fake(&f.owner, &session, "pay")
    );
    let mut codes = [a.status(), b.status()];
    codes.sort();
    assert_eq!(codes, [StatusCode::OK, StatusCode::CONFLICT]);
    let audits = f.order_audits(order["id"].as_str().unwrap()).await;
    let paid = audits.iter().filter(|a| a.0 == "order.paid").count();
    assert_eq!(paid, 1);
}

#[tokio::test]
async fn retiring_the_offering_leaves_a_paid_order_intact() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station").await;
    let (order, session) = f.bought(&offering).await;
    let id = order["id"].as_str().unwrap().to_string();
    ok_json(f.fake(&f.owner, &session, "pay").await).await;
    let row_before = f.stored(&id).await;
    let path = format!("/api/orders/{id}");
    let api_before = ok_json(f.hub.get(&path, Some(&f.owner.cookie)).await).await;

    // Rename it through the admin API (same terms), then retire it.
    let admin = bootstrap_admin(&f.hub).await;
    let renamed = f
        .hub
        .post(
            &format!("/api/admin/offerings/{offering}"),
            json!({
                "name": "Station renamed",
                "description": "A new description.",
                "price_minor": 120000,
                "currency": "USD",
                "interval": "annual",
                "grants": {
                    "scope": "machine",
                    "seats": 1,
                    "quota_erasures": null,
                    "features": ["hub_sync", "pdf_certificates"],
                    "term_months": 12
                }
            }),
            Some(&admin.cookie),
        )
        .await;
    assert_eq!(
        ok_json(renamed).await["offering"]["name"],
        "Station renamed"
    );
    sqlx::query("UPDATE offerings SET status = 'retired', retired_at = now() WHERE id = $1::uuid")
        .bind(&offering)
        .execute(&f.hub.db.pool)
        .await
        .unwrap();

    let row_after = f.stored(&id).await;
    assert_eq!(row_after["offering_name"], "Station");
    assert_eq!(row_after, row_before);
    let api_after = ok_json(f.hub.get(&path, Some(&f.owner.cookie)).await).await;
    assert_eq!(api_after["order"]["offering_name"], "Station");
    assert_eq!(api_after, api_before);
    // And it can no longer be bought.
    assert_error(
        f.checkout(&f.owner, &offering).await,
        StatusCode::NOT_FOUND,
        MSG_NOT_FOR_SALE,
    )
    .await;
}

#[tokio::test]
async fn checkout_waits_for_a_concurrent_retire() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station").await;
    let mut retire = f.hub.db.pool.begin().await.unwrap();
    sqlx::query("UPDATE offerings SET status = 'retired', retired_at = now() WHERE id = $1::uuid")
        .bind(&offering)
        .execute(&mut *retire)
        .await
        .unwrap();
    let commit_later = async {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        retire.commit().await.unwrap();
    };
    let (resp, ()) = tokio::join!(f.checkout(&f.owner, &offering), commit_later);
    assert_error(resp, StatusCode::NOT_FOUND, MSG_NOT_FOR_SALE).await;
    assert_eq!(f.hub.count("orders").await, 0);
}

#[tokio::test]
async fn removing_the_buyer_keeps_the_order() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station").await;
    let buyer = f.hub.add(&f.owner, "owner").await;
    let resp = f.checkout(&buyer, &offering).await;
    let code = resp.status();
    let v: Value = resp.json().await.unwrap();
    assert_eq!(code, StatusCode::CREATED, "{v}");
    let id = v["order"]["id"].as_str().unwrap().to_string();
    let session = session_of(v["checkout_url"].as_str().unwrap());
    ok_json(f.fake(&buyer, &session, "pay").await).await;
    let before = f.stored(&id).await;
    assert_eq!(before["created_by"], json!(buyer.id));

    let resp = f.hub.remove(&f.owner, &buyer.id).await;
    // The remove route answers 204 No Content on success.
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let after = f.stored(&id).await;
    assert_eq!(after["created_by"], Value::Null);
    for col in ["status", "paid_at", "updated_at"] {
        assert_eq!(after[col], before[col], "{col}");
    }
    assert_eq!(after["status"], "paid");
}

#[tokio::test]
async fn the_schema_refuses_illegal_order_changes() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station").await;
    let (order, session) = f.bought(&offering).await;
    let id = order["id"].as_str().unwrap().to_string();
    let pool = &f.hub.db.pool;
    for sql in [
        "UPDATE orders SET price_minor = 1 WHERE id = $1::uuid",
        "UPDATE orders SET provider_session_id = 'other' WHERE id = $1::uuid",
        "UPDATE orders SET status = 'paid' WHERE id = $1::uuid",
        "DELETE FROM orders WHERE id = $1::uuid",
    ] {
        sqlx::query(sql)
            .bind(&id)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    ok_json(f.fake(&f.owner, &session, "abandon").await).await;
    for sql in [
        "UPDATE orders SET status = 'paid', paid_at = now(), expired_at = NULL \
         WHERE id = $1::uuid",
        "UPDATE orders SET status = 'pending', expired_at = NULL WHERE id = $1::uuid",
    ] {
        sqlx::query(sql)
            .bind(&id)
            .execute(pool)
            .await
            .expect_err(sql);
    }
}

// ---------------------------------------------------------------------------
// apply_event, directly

#[tokio::test]
async fn events_that_do_not_apply_change_and_audit_nothing() {
    let f = fixture().await;
    let offering = f.hub.published_offering("Station").await;
    let (order, session) = f.bought(&offering).await;
    let fake = Fake::new(url::Url::parse("http://127.0.0.1:7900").unwrap());
    let verified = |event_type: &str, session: &str| {
        let (body, sig) = Fake::signed_event(event_type, session);
        fake.verify_event(&Fake::signature_headers(&sig), &body)
            .unwrap()
    };
    let ignored = {
        let body = json!({"id": "fake_evt_x", "type": "invoice.paid"}).to_string();
        fake.verify_event(
            &Fake::signature_headers(&Fake::sign(body.as_bytes())),
            body.as_bytes(),
        )
        .unwrap()
    };
    let before = f.audit_count().await;
    let mut conn = f.hub.db.pool.acquire().await.unwrap();
    let unmatched = wipe_hub::payments::apply_event(
        &mut conn,
        &verified(FAKE_CHECKOUT_COMPLETED, "fake_cs_unknown"),
    )
    .await
    .unwrap();
    assert_eq!(unmatched, ApplyOutcome::Unmatched);
    assert_eq!(
        wipe_hub::payments::apply_event(&mut conn, &ignored)
            .await
            .unwrap(),
        ApplyOutcome::Ignored
    );
    assert_eq!(f.audit_count().await, before);

    let ApplyOutcome::Applied(paid) =
        wipe_hub::payments::apply_event(&mut conn, &verified(FAKE_CHECKOUT_COMPLETED, &session))
            .await
            .unwrap()
    else {
        panic!("not applied");
    };
    assert_eq!(paid.status, "paid");
    assert_eq!(paid.id.to_string(), order["id"].as_str().unwrap());
    // `order.paid` and `entitlement.granted`.
    let after_pay = f.audit_count().await;
    assert_eq!(after_pay, before + 2);
    for kind in [FAKE_CHECKOUT_COMPLETED, FAKE_CHECKOUT_EXPIRED] {
        let again = wipe_hub::payments::apply_event(&mut conn, &verified(kind, &session))
            .await
            .unwrap();
        assert_eq!(again, ApplyOutcome::Unchanged(paid.clone()));
    }
    assert_eq!(f.audit_count().await, after_pay);
    // Applying directly stores no event (only `payment_events::receive`
    // does) and, for Fake, no billing customer.
    assert_eq!(f.hub.count("payment_events").await, 0);
    assert_eq!(f.hub.count("billing_customers").await, 0);
}

/// `src/*.rs` lines containing `needle`, as `file:line`.
fn source_lines_with(needle: &str) -> Vec<String> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let source = std::fs::read_to_string(&path).unwrap();
        for (i, line) in source.lines().enumerate() {
            if line.contains(needle) {
                found.push(format!(
                    "{}:{}",
                    path.file_name().unwrap().to_string_lossy(),
                    i + 1
                ));
            }
        }
    }
    found
}

/// The only write of `orders.status` is `orders::apply_event`.
#[test]
fn only_apply_event_updates_an_orders_status() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let found = source_lines_with("UPDATE orders");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("orders.rs:"), "{found:?}");
    let source = std::fs::read_to_string(format!("{dir}/orders.rs")).unwrap();
    let at = source.find("UPDATE orders SET status").unwrap();
    let apply = source.find("pub async fn apply_event").unwrap();
    assert!(at > apply, "the UPDATE is not inside apply_event");
    // The next function after apply_event starts after the UPDATE.
    let next_fn = source[apply + 1..]
        .find("\nfn ")
        .into_iter()
        .chain(source[apply + 1..].find("\npub fn "))
        .chain(source[apply + 1..].find("\nasync fn "))
        .chain(source[apply + 1..].find("\npub async fn "))
        .min()
        .map_or(source.len(), |i| apply + 1 + i);
    assert!(at < next_fn, "the UPDATE is not inside apply_event");
}

/// Entitlements are written only by `entitlements.rs` (story 4), and inserted
/// only by `entitlements::grant`.
#[test]
fn only_the_entitlements_module_writes_entitlements() {
    for needle in [
        "UPDATE entitlements",
        "INSERT INTO entitlements",
        "INSERT INTO entitlement_renewals",
    ] {
        let found = source_lines_with(needle);
        assert!(!found.is_empty(), "{needle}");
        assert!(
            found.iter().all(|f| f.starts_with("entitlements.rs:")),
            "{needle}: {found:?}"
        );
    }
    assert_eq!(source_lines_with("INSERT INTO entitlements ").len(), 1);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let source = std::fs::read_to_string(format!("{dir}/entitlements.rs")).unwrap();
    let at = source.find("INSERT INTO entitlements ").unwrap();
    let grant = source.find("pub(crate) async fn grant").unwrap();
    let next = source[grant + 1..].find("\npub(crate) async fn ").unwrap() + grant + 1;
    assert!(grant < at && at < next, "the INSERT is not inside grant");
    // `grant` is called only from `apply_event`.
    let calls = source_lines_with("entitlements::grant(");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].starts_with("orders.rs:"), "{calls:?}");
}

/// `apply_event` is called only from `payment_events::receive`, so every
/// applied event is stored first.
#[test]
fn only_payment_events_receive_calls_apply_event() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut calls = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let source = std::fs::read_to_string(&path).unwrap();
        for (i, line) in source.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") || code.contains("fn apply_event(") {
                continue;
            }
            if code.contains("apply_event(") {
                calls.push(format!(
                    "{}:{}",
                    path.file_name().unwrap().to_string_lossy(),
                    i + 1
                ));
            }
        }
    }
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].starts_with("payment_events.rs:"), "{calls:?}");
    let source = std::fs::read_to_string(format!("{dir}/payment_events.rs")).unwrap();
    let receive = source.find("pub async fn receive").unwrap();
    let call = source.find("apply_event(&mut").unwrap();
    assert!(call > receive, "apply_event is not called inside receive");
}

// ---------------------------------------------------------------------------
// Billing

#[tokio::test]
async fn owners_open_billing() {
    let f = fixture().await;
    let v = ok_json(
        f.hub
            .post_empty("/api/billing/portal", Some(&f.owner.cookie))
            .await,
    )
    .await;
    assert_eq!(v, json!({"url": "http://127.0.0.1:7900/billing/fake"}));
    let resp = f
        .hub
        .post_empty("/api/billing/portal", Some(&f.member.cookie))
        .await;
    assert_error(resp, StatusCode::FORBIDDEN, MSG_OWNER_ONLY_BILLING).await;
}
