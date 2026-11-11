//! Entitlements through the Fake provider (offerings-and-payments story 4,
//! CAP-4): fulfilment, the Fake "Renew now" and "Cancel" test actions, seats,
//! lapsed terms and isolation between organizations. One test per Fake row of
//! the story's I/O matrix, through HTTP, checking the response, the stored
//! rows and the audit rows. The Stripe rows are in `tests/stripe.rs`. Needs
//! `docker compose up -d`; fails, never skips, when it is down.

mod support;

use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{assert_error, spawn_hub, spawn_hub_args, Hub, Who};
use support::stripe_mock::{spawn_stripe_hub, StripeMock};
use uuid::Uuid;
use wipe_hub::order_routes::{MSG_NOT_RENEWING, MSG_NO_ENTITLEMENT, MSG_OWNER_ONLY_BILLING};
use wipe_hub::payments::{
    Fake, FAKE_SIGNATURE_HEADER, FAKE_SUBSCRIPTION_RENEWED, MSG_FAKE_LOCAL_ONLY,
};

const WEBHOOK: &str = "/api/payments/webhook";

struct Fixture {
    hub: Hub,
    owner: Who,
    member: Who,
    annual: String,
    one_off: String,
}

async fn fixture_on(hub: Hub) -> Fixture {
    let owner = hub.verified_owner("Acme Recyclers").await;
    let member = hub.add(&owner, "member").await;
    let annual = hub.published_offering("Station — annual, 1 seat").await;
    let one_off = hub
        .published_one_off_offering("Station — 2 years", 24)
        .await;
    Fixture {
        hub,
        owner,
        member,
        annual,
        one_off,
    }
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

/// One entitlement row: `(status, renews, months from term_start to
/// term_end, term_start = the order's paid_at, has a subscription)`.
type Row = (String, bool, i32, bool, bool);

impl Fixture {
    /// `who` buys `offering` and presses Pay on the Fake checkout page: the
    /// paid order.
    async fn buy(&self, who: &Who, offering: &str) -> Value {
        let resp = self
            .hub
            .post(
                "/api/checkout",
                json!({ "offering_id": offering }),
                Some(&who.cookie),
            )
            .await;
        let code = resp.status();
        let v: Value = resp.json().await.unwrap();
        assert_eq!(code, StatusCode::CREATED, "{v}");
        let url = url::Url::parse(v["checkout_url"].as_str().unwrap()).unwrap();
        let session = url
            .query_pairs()
            .find(|(k, _)| k == "session")
            .unwrap()
            .1
            .into_owned();
        let paid = ok_json(
            self.hub
                .post_empty(
                    &format!("/api/payments/fake/sessions/{session}/pay"),
                    Some(&who.cookie),
                )
                .await,
        )
        .await;
        assert_eq!(paid["order"]["status"], "paid");
        paid["order"].clone()
    }

    async fn seats(&self, who: &Who) -> Value {
        ok_json(self.hub.get("/api/entitlements", Some(&who.cookie)).await).await
    }

    async fn row(&self, entitlement: &str) -> Row {
        sqlx::query_as(
            "SELECT e.status, e.renews, \
                    (extract(year FROM age(e.term_end, e.term_start)) * 12 \
                     + extract(month FROM age(e.term_end, e.term_start)))::int, \
                    e.term_start = o.paid_at, e.subscription_id IS NOT NULL \
             FROM entitlements e JOIN orders o ON o.id = e.order_id WHERE e.id = $1::uuid",
        )
        .bind(entitlement)
        .fetch_one(&self.hub.db.pool)
        .await
        .unwrap()
    }

    async fn subscription_of(&self, entitlement: &str) -> String {
        sqlx::query_scalar("SELECT subscription_id FROM entitlements WHERE id = $1::uuid")
            .bind(entitlement)
            .fetch_one(&self.hub.db.pool)
            .await
            .unwrap()
    }

    async fn action(&self, who: &Who, entitlement: &str, action: &str) -> Response {
        self.hub
            .post_empty(
                &format!("/api/payments/fake/entitlements/{entitlement}/{action}"),
                Some(&who.cookie),
            )
            .await
    }

    /// `(action, actor, detail)` of every audit row about `entitlement`.
    async fn audits(&self, entitlement: &str) -> Vec<(String, String, Value)> {
        sqlx::query_as("SELECT action, actor, detail FROM audit_log WHERE subject = $1 ORDER BY id")
            .bind(format!("entitlement:{entitlement}"))
            .fetch_all(&self.hub.db.pool)
            .await
            .unwrap()
    }

    async fn renewal_count(&self) -> i64 {
        self.hub.count("entitlement_renewals").await
    }
}

fn first_id(seats: &Value) -> String {
    seats["entitlements"][0]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn buying_an_annual_offering_shows_one_available_seat() {
    let f = fixture().await;
    let before = f.seats(&f.owner).await;
    assert_eq!(before, json!({"entitlements": [], "available_seats": 0}));
    let order = f.buy(&f.owner, &f.annual).await;
    let order_id = order["id"].as_str().unwrap();

    // Every role sees it.
    let seats = f.seats(&f.member).await;
    assert_eq!(seats["available_seats"], 1);
    let list = seats["entitlements"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    let e = &list[0];
    let mut keys: Vec<&String> = e.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "cancelled_at",
            "credits",
            "current",
            "grants",
            "id",
            "interval",
            "offering_name",
            "order_id",
            "refunded_at",
            "renews",
            "status",
            "term_end",
            "term_start"
        ]
    );
    assert_eq!(e["order_id"], json!(order_id));
    assert_eq!(e["offering_name"], "Station — annual, 1 seat");
    assert_eq!(e["interval"], "annual");
    assert_eq!(e["grants"], order["grants"]);
    assert_eq!(e["status"], "active");
    assert_eq!(e["renews"], true);
    assert_eq!(e["current"], true);
    assert_eq!(e["term_start"], order["paid_at"]);
    assert!(e["term_end"].as_str().unwrap().ends_with('Z'));
    assert_eq!(e["cancelled_at"], Value::Null);
    assert_eq!(e["refunded_at"], Value::Null);
    // Unlimited: no credit pool.
    assert_eq!(e["credits"], Value::Null);

    let id = first_id(&seats);
    assert_eq!(f.row(&id).await, ("active".into(), true, 12, true, true));
    assert!(f.subscription_of(&id).await.starts_with("fake_sub_"));
    let audits = f.audits(&id).await;
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(audits[0].0, "entitlement.granted");
    assert_eq!(audits[0].1, "provider:fake");
    assert_eq!(
        audits[0].2,
        json!({"order_id": order_id, "term_end": e["term_end"]})
    );
    assert_eq!(f.hub.count("entitlements").await, 1);
}

#[tokio::test]
async fn entitlements_are_listed_newest_first() {
    let f = fixture().await;
    let annual = f.buy(&f.owner, &f.annual).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let once = f.buy(&f.owner, &f.one_off).await;
    let seats = f.seats(&f.member).await;
    let orders: Vec<&Value> = seats["entitlements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["order_id"])
        .collect();
    assert_eq!(orders, [&once["id"], &annual["id"]]);
}

#[tokio::test]
async fn a_one_off_purchase_does_not_renew() {
    let f = fixture().await;
    f.buy(&f.owner, &f.one_off).await;
    let seats = f.seats(&f.owner).await;
    assert_eq!(seats["available_seats"], 2);
    let id = first_id(&seats);
    assert_eq!(f.row(&id).await, ("active".into(), false, 24, true, false));
    assert_eq!(seats["entitlements"][0]["renews"], false);
    let before = f.hub.count("audit_log").await;
    for action in ["renew", "cancel"] {
        assert_error(
            f.action(&f.owner, &id, action).await,
            StatusCode::CONFLICT,
            MSG_NOT_RENEWING,
        )
        .await;
    }
    assert_eq!(f.hub.count("audit_log").await, before);
    assert_eq!(f.hub.count("payment_events").await, 1);
}

#[tokio::test]
async fn renew_now_extends_the_term_from_its_current_end() {
    let f = fixture().await;
    f.buy(&f.owner, &f.annual).await;
    let id = first_id(&f.seats(&f.owner).await);
    let first = ok_json(f.action(&f.owner, &id, "renew").await).await;
    assert_eq!(first["entitlement"]["id"], json!(id));
    assert_eq!(f.row(&id).await.2, 24);
    let second = ok_json(f.action(&f.owner, &id, "renew").await).await;
    assert_eq!(f.row(&id).await.2, 36);
    assert!(
        second["entitlement"]["term_end"].as_str().unwrap()
            > first["entitlement"]["term_end"].as_str().unwrap()
    );
    assert_eq!(f.renewal_count().await, 2);
    let invoices: Vec<String> = sqlx::query_scalar(
        "SELECT invoice_id FROM entitlement_renewals WHERE provider = 'fake' ORDER BY created_at",
    )
    .fetch_all(&f.hub.db.pool)
    .await
    .unwrap();
    assert!(invoices.iter().all(|i| i.starts_with("fake_in_")));
    assert_ne!(invoices[0], invoices[1]);
    let audits = f.audits(&id).await;
    let actions: Vec<&str> = audits.iter().map(|a| a.0.as_str()).collect();
    assert_eq!(
        actions,
        [
            "entitlement.granted",
            "entitlement.renewed",
            "entitlement.renewed"
        ]
    );
    assert_eq!(audits[1].1, "provider:fake");
    assert_eq!(audits[1].2["invoice_id"], json!(invoices[0]));
    assert_eq!(audits[1].2["previous_term_end"], audits[0].2["term_end"]);

    // A replayed renewal event changes nothing; a new event for an applied
    // invoice is unchanged.
    let sub = f.subscription_of(&id).await;
    let (body, sig) = Fake::signed_event_with(
        FAKE_SUBSCRIPTION_RENEWED,
        None,
        Some(&sub),
        Some("fake_in_replayed"),
    );
    for _ in 0..2 {
        let resp = f
            .hub
            .http
            .post(f.hub.url(WEBHOOK))
            .header(FAKE_SIGNATURE_HEADER, &sig)
            .body(body.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
    assert_eq!(f.row(&id).await.2, 48);
    let (body, sig) = Fake::signed_event_with(
        FAKE_SUBSCRIPTION_RENEWED,
        None,
        Some(&sub),
        Some("fake_in_replayed"),
    );
    let resp = f
        .hub
        .http
        .post(f.hub.url(WEBHOOK))
        .header(FAKE_SIGNATURE_HEADER, &sig)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(f.row(&id).await.2, 48);
    let outcomes: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT outcome FROM payment_events WHERE event_type = 'subscription.renewed' \
         ORDER BY received_at",
    )
    .fetch_all(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(
        outcomes,
        [
            Some("applied".to_string()),
            Some("applied".to_string()),
            Some("applied".to_string()),
            Some("unchanged".to_string())
        ]
    );
    assert_eq!(f.renewal_count().await, 3);
}

#[tokio::test]
async fn cancel_stops_renewal_and_keeps_the_seats() {
    let f = fixture().await;
    f.buy(&f.owner, &f.annual).await;
    let id = first_id(&f.seats(&f.owner).await);
    let v = ok_json(f.action(&f.owner, &id, "cancel").await).await;
    assert_eq!(v["entitlement"]["renews"], false);
    assert_eq!(v["entitlement"]["status"], "active");
    assert_eq!(f.row(&id).await, ("active".into(), false, 12, true, true));
    assert_eq!(f.seats(&f.member).await["available_seats"], 1);
    let audits = f.audits(&id).await;
    assert_eq!(audits.len(), 2);
    assert_eq!(
        (audits[1].0.as_str(), audits[1].1.as_str()),
        ("entitlement.renewal_stopped", "provider:fake")
    );
    assert!(audits[1].2["event_id"]
        .as_str()
        .unwrap()
        .starts_with("fake_evt_"));
    let before = f.hub.count("audit_log").await;
    for action in ["renew", "cancel"] {
        assert_error(
            f.action(&f.owner, &id, action).await,
            StatusCode::CONFLICT,
            MSG_NOT_RENEWING,
        )
        .await;
    }
    assert_eq!(f.hub.count("audit_log").await, before);
    assert_eq!(f.renewal_count().await, 0);
}

#[tokio::test]
async fn a_lapsed_term_is_not_current_and_has_no_seats() {
    let f = fixture().await;
    f.buy(&f.owner, &f.annual).await;
    f.buy(&f.owner, &f.one_off).await;
    assert_eq!(f.seats(&f.owner).await["available_seats"], 3);
    // Move the annual entitlement's whole term into the past (the trigger
    // refuses this to the hub, so it is lifted for the test).
    let pool = &f.hub.db.pool;
    sqlx::query("ALTER TABLE entitlements DISABLE TRIGGER entitlements_lifecycle")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE entitlements e SET term_start = e.term_start - interval '2 years', \
                term_end = e.term_end - interval '2 years' \
         FROM orders o WHERE o.id = e.order_id AND o.billing_interval = 'annual'",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE entitlements ENABLE TRIGGER entitlements_lifecycle")
        .execute(pool)
        .await
        .unwrap();
    let seats = f.seats(&f.owner).await;
    assert_eq!(seats["available_seats"], 2);
    let lapsed: Vec<&Value> = seats["entitlements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["interval"] == "annual")
        .collect();
    assert_eq!(lapsed.len(), 1);
    assert_eq!(lapsed[0]["current"], false);
    assert_eq!(lapsed[0]["status"], "active");
}

#[tokio::test]
async fn another_organization_sees_and_renews_only_its_own() {
    let f = fixture().await;
    f.buy(&f.owner, &f.annual).await;
    let a_id = first_id(&f.seats(&f.owner).await);
    let b_owner = f.hub.verified_owner("Bravo Holdings").await;
    let b_member = f.hub.add(&b_owner, "member").await;
    for who in [&b_owner, &b_member] {
        assert_eq!(
            f.seats(who).await,
            json!({"entitlements": [], "available_seats": 0})
        );
    }
    f.buy(&b_owner, &f.one_off).await;
    let b_seats = f.seats(&b_member).await;
    assert_eq!(b_seats["available_seats"], 2);
    assert_eq!(b_seats["entitlements"].as_array().unwrap().len(), 1);
    assert_ne!(first_id(&b_seats), a_id);
    let before = (
        f.hub.count("audit_log").await,
        f.hub.count("payment_events").await,
    );
    for action in ["renew", "cancel"] {
        assert_error(
            f.action(&b_owner, &a_id, action).await,
            StatusCode::NOT_FOUND,
            MSG_NO_ENTITLEMENT,
        )
        .await;
        // A member is refused before the entitlement is read.
        assert_error(
            f.action(&b_member, &a_id, action).await,
            StatusCode::FORBIDDEN,
            MSG_OWNER_ONLY_BILLING,
        )
        .await;
        assert_error(
            f.action(&f.member, &a_id, action).await,
            StatusCode::FORBIDDEN,
            MSG_OWNER_ONLY_BILLING,
        )
        .await;
        assert_error(
            f.action(&f.owner, &Uuid::new_v4().to_string(), action)
                .await,
            StatusCode::NOT_FOUND,
            MSG_NO_ENTITLEMENT,
        )
        .await;
        assert_error(
            f.action(&f.owner, "not-a-uuid", action).await,
            StatusCode::NOT_FOUND,
            MSG_NO_ENTITLEMENT,
        )
        .await;
    }
    assert_eq!(
        (
            f.hub.count("audit_log").await,
            f.hub.count("payment_events").await
        ),
        before
    );
    assert_eq!(f.row(&a_id).await, ("active".into(), true, 12, true, true));
}

#[tokio::test]
async fn fake_renewals_are_refused_off_a_local_fake_hub() {
    // Under Stripe the Fake routes do not exist for the caller.
    let mock = StripeMock::start().await;
    let stripe = fixture_on(spawn_stripe_hub(&mock).await).await;
    let id = Uuid::new_v4().to_string();
    assert_error(
        stripe.action(&stripe.owner, &id, "renew").await,
        StatusCode::NOT_FOUND,
        MSG_NO_ENTITLEMENT,
    )
    .await;
    // A Fake hub that is not local cannot sign events.
    let remote = spawn_hub_args(|a| a.public_base_url = "https://hub.example.com".into()).await;
    let owner = remote.verified_owner("Acme Recyclers").await;
    for action in ["renew", "cancel"] {
        assert_error(
            remote
                .post_empty(
                    &format!("/api/payments/fake/entitlements/{id}/{action}"),
                    Some(&owner.cookie),
                )
                .await,
            StatusCode::SERVICE_UNAVAILABLE,
            MSG_FAKE_LOCAL_ONLY,
        )
        .await;
    }
}
