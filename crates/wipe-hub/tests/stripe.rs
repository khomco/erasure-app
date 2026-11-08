//! The Stripe provider and the payment webhook (offerings-and-payments story
//! 3, CAP-3, CAP-5 and CAP-7). One test per row of the story's I/O matrix,
//! through HTTP: a hub on the Stripe provider pointed at a local mock of the
//! Stripe API (`support::stripe_mock`), and Stripe-shaped event fixtures from
//! `tests/fixtures/stripe/` signed in the test. Asserts the responses, the
//! stored order, the `payment_events` and `billing_customers` rows and the
//! audit rows. Nothing leaves 127.0.0.1. Needs `docker compose up -d`; fails,
//! never skips, when it is down.

mod support;

use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::hub::{assert_error, error_of, spawn_hub, spawn_hub_args, Hub, Who, BASE};
use support::stripe_mock::{
    spawn_stripe_hub, StripeMock, BILLING_URL, STRIPE_SECRET_KEY, STRIPE_WEBHOOK_SECRET,
};
use uuid::Uuid;
use wipe_hub::csrf::MSG_BAD_CSRF;
use wipe_hub::order_routes::{
    MSG_EVENT_NOT_READY, MSG_NO_BILLING_ACCOUNT, MSG_OWNER_ONLY_BILLING, MSG_PROVIDER_FAILED,
};
use wipe_hub::payments::{
    Fake, FAKE_CHECKOUT_COMPLETED, FAKE_SIGNATURE_HEADER, MSG_FAKE_LOCAL_ONLY,
};
use wipe_hub::stripe::{signature_header, STRIPE_API_VERSION, STRIPE_SIGNATURE_HEADER};

const WEBHOOK: &str = "/api/payments/webhook";
const PRICE: i64 = 120000;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// ---------------------------------------------------------------------------
// Fixtures

fn template(name: &str) -> &'static str {
    match name {
        "checkout.session.completed" => {
            include_str!("fixtures/stripe/checkout.session.completed.json")
        }
        "checkout.session.completed.unpaid" => {
            include_str!("fixtures/stripe/checkout.session.completed.unpaid.json")
        }
        "checkout.session.async_payment_succeeded" => {
            include_str!("fixtures/stripe/checkout.session.async_payment_succeeded.json")
        }
        "checkout.session.async_payment_failed" => {
            include_str!("fixtures/stripe/checkout.session.async_payment_failed.json")
        }
        "checkout.session.expired" => {
            include_str!("fixtures/stripe/checkout.session.expired.json")
        }
        "payment_intent.succeeded" => {
            include_str!("fixtures/stripe/payment_intent.succeeded.json")
        }
        other => panic!("no fixture {other}"),
    }
}

/// A fixture with its placeholders filled: a fresh event id, `session`, the
/// order and organization ids, the spawned hub's origin (`BASE`, the
/// `metadata[wipehub_hub]` the hub sends), the offering's price, and `usd`
/// (Stripe sends currencies lower-case).
fn event(name: &str, session: &str, order: &str, org: &str) -> Value {
    let text = template(name)
        .replace(
            "__EVENT_ID__",
            &format!("evt_test_{}", Uuid::new_v4().simple()),
        )
        .replace("__SESSION_ID__", session)
        .replace("__ORDER_ID__", order)
        .replace("__ORGANIZATION_ID__", org)
        .replace("__HUB__", BASE)
        .replace("__AMOUNT__", &PRICE.to_string())
        .replace("__CURRENCY__", "usd");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

/// POST `body` to the webhook with `signature` (if any) as
/// `Stripe-Signature`, and no cookie or CSRF token.
async fn post_webhook(hub: &Hub, signature: Option<String>, body: &[u8]) -> Response {
    let mut req = hub
        .http
        .post(hub.url(WEBHOOK))
        .header("content-type", "application/json")
        .body(body.to_vec());
    if let Some(sig) = signature {
        req = req.header(STRIPE_SIGNATURE_HEADER, sig);
    }
    req.send().await.unwrap()
}

/// Deliver `body` signed now with the hub's webhook secret.
async fn deliver(hub: &Hub, body: &[u8]) -> Response {
    let sig = signature_header(STRIPE_WEBHOOK_SECRET, now(), body);
    post_webhook(hub, Some(sig), body).await
}

async fn received(resp: Response) {
    let code = resp.status();
    let v: Value = resp.json().await.unwrap();
    assert_eq!((code, v), (StatusCode::OK, json!({"received": true})));
}

// ---------------------------------------------------------------------------
// A hub on Stripe with one organization

struct Shop {
    hub: Hub,
    mock: StripeMock,
    owner: Who,
    member: Who,
    org: String,
    offering: String,
}

/// One stored payment event: `(event_id, event_type, session_id,
/// payload_sha256, outcome)`.
type EventRow = (String, String, Option<String>, String, Option<String>);

async fn org_of(hub: &Hub, who: &Who) -> String {
    let org: Uuid = sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1::uuid")
        .bind(&who.id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    org.to_string()
}

async fn shop() -> Shop {
    let mock = StripeMock::start().await;
    let hub = spawn_stripe_hub(&mock).await;
    let owner = hub.verified_owner("Acme Recyclers").await;
    let member = hub.add(&owner, "member").await;
    let org = org_of(&hub, &owner).await;
    let offering = hub.published_offering("Station annual").await;
    Shop {
        hub,
        mock,
        owner,
        member,
        org,
        offering,
    }
}

impl Shop {
    async fn checkout_resp(&self) -> Response {
        self.hub
            .post(
                "/api/checkout",
                json!({ "offering_id": self.offering }),
                Some(&self.owner.cookie),
            )
            .await
    }

    /// A pending order: `(response body, order id, session id)`.
    async fn checkout(&self) -> (Value, String, String) {
        let resp = self.checkout_resp().await;
        let code = resp.status();
        let v: Value = resp.json().await.unwrap();
        assert_eq!(code, StatusCode::CREATED, "{v}");
        let id = v["order"]["id"].as_str().unwrap().to_string();
        let session = self.session_of(&id).await;
        (v, id, session)
    }

    async fn session_of(&self, order: &str) -> String {
        sqlx::query_scalar("SELECT provider_session_id FROM orders WHERE id = $1::uuid")
            .bind(order)
            .fetch_one(&self.hub.db.pool)
            .await
            .unwrap()
    }

    /// The fixture `name` for this organization's `order` and `session`.
    fn event(&self, name: &str, session: &str, order: &str) -> Value {
        event(name, session, order, &self.org)
    }

    async fn status(&self, order: &str) -> String {
        sqlx::query_scalar("SELECT status FROM orders WHERE id = $1::uuid")
            .bind(order)
            .fetch_one(&self.hub.db.pool)
            .await
            .unwrap()
    }

    async fn events(&self) -> Vec<EventRow> {
        sqlx::query_as(
            "SELECT event_id, event_type, session_id, payload_sha256, outcome \
             FROM payment_events ORDER BY received_at, event_id",
        )
        .fetch_all(&self.hub.db.pool)
        .await
        .unwrap()
    }

    async fn outcome(&self, event: &Value) -> Option<String> {
        sqlx::query_scalar(
            "SELECT outcome FROM payment_events WHERE provider = 'stripe' AND event_id = $1",
        )
        .bind(event["id"].as_str().unwrap())
        .fetch_one(&self.hub.db.pool)
        .await
        .unwrap()
    }

    async fn customers(&self) -> Vec<(String, String, String)> {
        sqlx::query_as(
            "SELECT organization_id::text, provider, customer_id FROM billing_customers \
             ORDER BY organization_id",
        )
        .fetch_all(&self.hub.db.pool)
        .await
        .unwrap()
    }

    /// `(action, actor, detail)` of every audit row about order `id`.
    async fn order_audits(&self, id: &str) -> Vec<(String, String, Value)> {
        sqlx::query_as("SELECT action, actor, detail FROM audit_log WHERE subject = $1 ORDER BY id")
            .bind(format!("order:{id}"))
            .fetch_all(&self.hub.db.pool)
            .await
            .unwrap()
    }

    async fn billing(&self, who: &Who) -> Response {
        self.hub
            .post_empty("/api/billing/portal", Some(&who.cookie))
            .await
    }
}

// ---------------------------------------------------------------------------
// Checkout

#[tokio::test]
async fn stripe_checkout_sends_the_documented_request() {
    let s = shop().await;
    let (v, id, session) = s.checkout().await;
    assert_eq!(v["order"]["status"], "pending");
    assert_eq!(v["order"]["provider"], "stripe");
    assert!(session.starts_with("cs_test_"), "{session}");
    assert_eq!(
        v["checkout_url"],
        json!(format!("https://checkout.stripe.com/c/pay/{session}"))
    );

    let requests = s.mock.requests();
    assert_eq!(requests.len(), 1);
    let r = &requests[0];
    assert_eq!(
        (r.method.as_str(), r.path.as_str()),
        ("POST", "/v1/checkout/sessions")
    );
    assert_eq!(
        r.header("authorization"),
        Some(format!("Bearer {STRIPE_SECRET_KEY}").as_str())
    );
    assert_eq!(r.header("stripe-version"), Some(STRIPE_API_VERSION));
    assert_eq!(
        r.header("idempotency-key"),
        Some(format!("wipehub-checkout-{id}").as_str())
    );
    assert_eq!(
        r.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    let page = format!("{BASE}/orders/{id}");
    let mut want = vec![
        ("mode", "payment".to_string()),
        ("client_reference_id", id.clone()),
        ("success_url", page.clone()),
        ("cancel_url", page),
        ("line_items[0][quantity]", "1".into()),
        ("line_items[0][price_data][currency]", "usd".into()),
        ("line_items[0][price_data][unit_amount]", PRICE.to_string()),
        (
            "line_items[0][price_data][product_data][name]",
            "Station annual".into(),
        ),
        ("metadata[wipehub_order_id]", id.clone()),
        ("metadata[wipehub_organization_id]", s.org.clone()),
        ("metadata[wipehub_hub]", BASE.into()),
        ("customer_creation", "always".into()),
    ];
    let mut got: Vec<(String, String)> = r.form.clone();
    let mut want: Vec<(String, String)> = want.drain(..).map(|(k, v)| (k.to_string(), v)).collect();
    got.sort();
    want.sort();
    assert_eq!(got, want);
    // No personal data goes to Stripe.
    let form = format!("{:?}", r.form);
    assert!(!form.contains(&s.owner.email), "{form}");
    assert!(!form.contains("Ada Lovelace"), "{form}");

    let audits = s.order_audits(&id).await;
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(audits[0].0, "order.created");
    assert_eq!(audits[0].2["provider"], "stripe");
}

#[tokio::test]
async fn an_unusable_stripe_answer_is_502_and_writes_nothing() {
    let s = shop().await;
    let before = (s.hub.count("orders").await, s.hub.count("audit_log").await);
    for (status, reply) in [
        (
            500,
            json!({"error": {"type": "api_error", "message": "boom"}}),
        ),
        (402, json!({"error": {"type": "card_error"}})),
        (200, json!({"id": "cs_test_no_url"})),
        (
            200,
            json!({"id": "cs_test_bad_url", "url": "javascript:alert(1)"}),
        ),
        (200, json!({"url": "https://checkout.stripe.com/c/pay/x"})),
        (
            200,
            json!({"id": "", "url": "https://checkout.stripe.com/c/pay/x"}),
        ),
    ] {
        s.mock.reply_next(status, reply.clone());
        assert_error(
            s.checkout_resp().await,
            StatusCode::BAD_GATEWAY,
            MSG_PROVIDER_FAILED,
        )
        .await;
        let after = (s.hub.count("orders").await, s.hub.count("audit_log").await);
        assert_eq!(after, before, "{reply}");
    }
}

// ---------------------------------------------------------------------------
// Webhook: paid, replay, out of order

#[tokio::test]
async fn a_paid_webhook_marks_the_order_paid() {
    let s = shop().await;
    let (_, id, session) = s.checkout().await;
    let ev = s.event("checkout.session.completed", &session, &id);
    let body = bytes(&ev);
    received(deliver(&s.hub, &body).await).await;

    assert_eq!(s.status(&id).await, "paid");
    let audits = s.order_audits(&id).await;
    let actions: Vec<&str> = audits.iter().map(|a| a.0.as_str()).collect();
    assert_eq!(actions, ["order.created", "order.paid"]);
    assert_eq!(audits[1].1, "provider:stripe");
    assert_eq!(audits[1].2, json!({"event_id": ev["id"]}));

    let digest = hex::encode(Sha256::digest(&body));
    assert_eq!(
        s.events().await,
        vec![(
            ev["id"].as_str().unwrap().to_string(),
            "checkout.session.completed".to_string(),
            Some(session.clone()),
            digest,
            Some("applied".to_string()),
        )]
    );
    assert_eq!(
        s.customers().await,
        vec![(s.org.clone(), "stripe".into(), "cus_test_1".into())]
    );
    // The buyer's personal data in the payload is not stored anywhere.
    let dump: String = sqlx::query_scalar(
        "SELECT coalesce(string_agg(row_to_json(e)::text, ''), '') FROM payment_events e",
    )
    .fetch_one(&s.hub.db.pool)
    .await
    .unwrap();
    assert!(!dump.contains("buyer@example.com"), "{dump}");
    assert!(!dump.contains("Test Buyer"), "{dump}");
}

#[tokio::test]
async fn a_replayed_event_changes_nothing_sequentially_or_concurrently() {
    let s = shop().await;
    let (_, id, session) = s.checkout().await;
    let body = bytes(&s.event("checkout.session.completed", &session, &id));
    // Delivered twice at once, then again.
    let (a, b) = tokio::join!(deliver(&s.hub, &body), deliver(&s.hub, &body));
    received(a).await;
    received(b).await;
    received(deliver(&s.hub, &body).await).await;

    assert_eq!(s.status(&id).await, "paid");
    let audits = s.order_audits(&id).await;
    let actions: Vec<&str> = audits.iter().map(|a| a.0.as_str()).collect();
    assert_eq!(actions, ["order.created", "order.paid"]);
    assert_eq!(s.events().await.len(), 1);
    assert_eq!(s.customers().await.len(), 1);
}

#[tokio::test]
async fn out_of_order_events_are_handled() {
    let s = shop().await;

    // `expired` after `paid`: unchanged.
    let (_, id, session) = s.checkout().await;
    received(
        deliver(
            &s.hub,
            &bytes(&s.event("checkout.session.completed", &session, &id)),
        )
        .await,
    )
    .await;
    let late = s.event("checkout.session.expired", &session, &id);
    received(deliver(&s.hub, &bytes(&late)).await).await;
    assert_eq!(s.outcome(&late).await.as_deref(), Some("unchanged"));
    assert_eq!(s.status(&id).await, "paid");
    let actions: Vec<String> = s.order_audits(&id).await.into_iter().map(|a| a.0).collect();
    assert_eq!(actions, ["order.created", "order.paid"]);

    // `completed` but not yet paid, then `async_payment_succeeded`.
    let (_, id, session) = s.checkout().await;
    let unpaid = s.event("checkout.session.completed.unpaid", &session, &id);
    received(deliver(&s.hub, &bytes(&unpaid)).await).await;
    assert_eq!(s.outcome(&unpaid).await.as_deref(), Some("ignored"));
    assert_eq!(s.status(&id).await, "pending");
    let succeeded = s.event("checkout.session.async_payment_succeeded", &session, &id);
    received(deliver(&s.hub, &bytes(&succeeded)).await).await;
    assert_eq!(s.outcome(&succeeded).await.as_deref(), Some("applied"));
    assert_eq!(s.status(&id).await, "paid");
    let audits = s.order_audits(&id).await;
    assert_eq!(audits.len(), 2);
    assert_eq!(
        (audits[1].0.as_str(), audits[1].1.as_str()),
        ("order.paid", "provider:stripe")
    );
    assert_eq!(audits[1].2, json!({"event_id": succeeded["id"]}));

    // `async_payment_failed` on a pending order: expired.
    let (_, id, session) = s.checkout().await;
    let failed = s.event("checkout.session.async_payment_failed", &session, &id);
    received(deliver(&s.hub, &bytes(&failed)).await).await;
    assert_eq!(s.outcome(&failed).await.as_deref(), Some("applied"));
    assert_eq!(s.status(&id).await, "expired");
    let audits = s.order_audits(&id).await;
    assert_eq!(audits.len(), 2);
    assert_eq!(
        (audits[1].0.as_str(), audits[1].1.as_str()),
        ("order.expired", "provider:stripe")
    );
    assert_eq!(audits[1].2, json!({"event_id": failed["id"]}));
}

#[tokio::test]
async fn an_expired_order_stores_no_customer() {
    // A fresh organization with no stored customer, so storing one would show.
    let s = shop().await;
    assert!(s.customers().await.is_empty());
    let (_, id, session) = s.checkout().await;
    let failed = s.event("checkout.session.async_payment_failed", &session, &id);
    assert_eq!(failed["data"]["object"]["customer"], "cus_test_1");
    received(deliver(&s.hub, &bytes(&failed)).await).await;
    assert_eq!(s.outcome(&failed).await.as_deref(), Some("applied"));
    assert_eq!(s.status(&id).await, "expired");
    assert!(s.customers().await.is_empty());
    assert_error(
        s.billing(&s.owner).await,
        StatusCode::CONFLICT,
        MSG_NO_BILLING_ACCOUNT,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Webhook: events that do not apply

#[tokio::test]
async fn an_event_before_its_order_is_refused_until_the_order_exists() {
    let s = shop().await;
    s.mock.reply_next(
        200,
        json!({"id": "cs_test_early", "url": "https://checkout.stripe.com/c/pay/cs_test_early"}),
    );
    let (seen, release) = s.mock.hold_next();
    let driver = async {
        // Stripe has the checkout request; the hub has not stored the order.
        let request = seen.await.unwrap();
        let order = request.field("client_reference_id").unwrap().to_string();
        let ev = s.event("checkout.session.completed", "cs_test_early", &order);
        let body = bytes(&ev);
        assert_error(
            deliver(&s.hub, &body).await,
            StatusCode::SERVICE_UNAVAILABLE,
            MSG_EVENT_NOT_READY,
        )
        .await;
        assert_eq!(s.hub.count("payment_events").await, 0);
        release.send(()).unwrap();
        (order, body)
    };
    let (resp, (order, body)) = tokio::join!(s.checkout_resp(), driver);
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(s.status(&order).await, "pending");
    // Stripe retries the same event: now it applies.
    received(deliver(&s.hub, &body).await).await;
    assert_eq!(s.status(&order).await, "paid");
    let events = s.events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].4.as_deref(), Some("applied"));
}

#[tokio::test]
async fn foreign_and_other_events_are_stored_and_acknowledged() {
    let s = shop().await;
    let (_, id, session) = s.checkout().await;
    let other_org = Uuid::new_v4().to_string();

    // A paid session that is not the hub's (no metadata or reference).
    let mut foreign = event(
        "checkout.session.completed",
        "cs_test_foreign",
        "x",
        &other_org,
    );
    let object = foreign["data"]["object"].as_object_mut().unwrap();
    object.insert("metadata".into(), json!({}));
    object.insert("client_reference_id".into(), Value::Null);
    // An unknown session that expired, even one naming a hub order.
    let expired = event(
        "checkout.session.expired",
        "cs_test_unknown",
        &Uuid::new_v4().to_string(),
        &other_org,
    );
    let intent = s.event("payment_intent.succeeded", &session, &id);
    for (ev, outcome) in [
        (&foreign, "unmatched"),
        (&expired, "unmatched"),
        (&intent, "ignored"),
    ] {
        received(deliver(&s.hub, &bytes(ev)).await).await;
        assert_eq!(s.outcome(ev).await.as_deref(), Some(outcome), "{ev}");
    }
    let events = s.events().await;
    assert_eq!(events.len(), 3);
    let intent_row = events
        .iter()
        .find(|e| e.1 == "payment_intent.succeeded")
        .unwrap();
    assert_eq!(intent_row.2, None);
    assert_eq!(s.status(&id).await, "pending");
    assert_eq!(s.order_audits(&id).await.len(), 1);
    assert!(s.customers().await.is_empty());
}

#[tokio::test]
async fn a_foreign_uuid_reference_is_not_the_hubs_order() {
    // Another app on a shared Stripe account puts its own UUID in
    // `client_reference_id` and sets no hub metadata (decision 9).
    let s = shop().await;
    let (_, id, _) = s.checkout().await;
    let before = (s.hub.count("orders").await, s.hub.count("audit_log").await);
    let foreign_ref = Uuid::new_v4().to_string();
    let mut ev = event(
        "checkout.session.completed",
        "cs_test_other_app",
        &foreign_ref,
        &Uuid::new_v4().to_string(),
    );
    ev["data"]["object"]["metadata"] = json!({});
    assert_eq!(
        ev["data"]["object"]["client_reference_id"],
        json!(foreign_ref)
    );
    received(deliver(&s.hub, &bytes(&ev)).await).await;
    assert_eq!(s.outcome(&ev).await.as_deref(), Some("unmatched"));
    assert_eq!(s.events().await.len(), 1);
    assert_eq!(
        (s.hub.count("orders").await, s.hub.count("audit_log").await),
        before
    );
    assert_eq!(s.status(&id).await, "pending");
    assert!(s.customers().await.is_empty());
}

#[tokio::test]
async fn another_hubs_paid_event_is_unmatched_never_503() {
    // Several hubs share one Stripe test account (decision 11): every hub
    // receives every hub's events. Another hub's paid session names an
    // order this hub never had.
    let s = shop().await;
    let (_, id, _) = s.checkout().await;
    let before = (s.hub.count("orders").await, s.hub.count("audit_log").await);
    for hub in [
        json!("https://staging.wipehub.example"),
        json!("http://127.0.0.1:7901"),
        Value::Null,
    ] {
        let mut ev = event(
            "checkout.session.completed",
            &format!("cs_test_other_hub_{}", Uuid::new_v4().simple()),
            &Uuid::new_v4().to_string(),
            &Uuid::new_v4().to_string(),
        );
        let metadata = ev["data"]["object"]["metadata"].as_object_mut().unwrap();
        match hub {
            Value::Null => {
                metadata.remove("wipehub_hub");
            }
            other => {
                metadata.insert("wipehub_hub".into(), other);
            }
        }
        received(deliver(&s.hub, &bytes(&ev)).await).await;
        assert_eq!(
            s.outcome(&ev).await.as_deref(),
            Some("unmatched"),
            "{}",
            ev["data"]["object"]["metadata"]
        );
    }
    assert_eq!(s.events().await.len(), 3);
    assert_eq!(
        (s.hub.count("orders").await, s.hub.count("audit_log").await),
        before
    );
    assert_eq!(s.status(&id).await, "pending");
    assert!(s.customers().await.is_empty());
}

#[tokio::test]
async fn a_paid_event_that_disagrees_with_its_order_leaves_it_pending() {
    let s = shop().await;
    let (_, id, session) = s.checkout().await;
    let tweak = |field: &str, value: Value| {
        let mut ev = s.event("checkout.session.completed", &session, &id);
        let object = &mut ev["data"]["object"];
        match field {
            "order" => object["metadata"]["wipehub_order_id"] = value,
            f => object[f] = value,
        }
        ev
    };
    for ev in [
        tweak("amount_subtotal", json!(PRICE - 1)),
        tweak("currency", json!("eur")),
        tweak("order", json!(Uuid::new_v4().to_string())),
    ] {
        received(deliver(&s.hub, &bytes(&ev)).await).await;
        assert_eq!(s.outcome(&ev).await.as_deref(), Some("mismatch"), "{ev}");
        assert_eq!(s.status(&id).await, "pending");
    }
    assert_eq!(s.order_audits(&id).await.len(), 1);
    assert!(s.customers().await.is_empty());
    // The matching event still applies.
    let good = s.event("checkout.session.completed", &session, &id);
    received(deliver(&s.hub, &bytes(&good)).await).await;
    assert_eq!(s.status(&id).await, "paid");
}

// ---------------------------------------------------------------------------
// Webhook: signatures and CSRF

#[tokio::test]
async fn a_bad_signature_is_400_and_stores_nothing() {
    let s = shop().await;
    let (_, id, session) = s.checkout().await;
    let body = bytes(&s.event("checkout.session.completed", &session, &id));
    let t = now();
    let good = signature_header(STRIPE_WEBHOOK_SECRET, t, &body);
    let tampered = String::from_utf8(body.clone())
        .unwrap()
        .replace(&PRICE.to_string(), "1");
    let cases: Vec<(&str, Option<String>, Vec<u8>)> = vec![
        (
            "wrong secret",
            Some(signature_header("whsec_wrong", t, &body)),
            body.clone(),
        ),
        ("tampered body", Some(good.clone()), tampered.into_bytes()),
        (
            "stale",
            Some(signature_header(STRIPE_WEBHOOK_SECRET, t - 301, &body)),
            body.clone(),
        ),
        (
            "future",
            Some(signature_header(STRIPE_WEBHOOK_SECRET, t + 301, &body)),
            body.clone(),
        ),
        ("missing", None, body.clone()),
        ("malformed", Some("t=yesterday,v1=zz".into()), body.clone()),
        ("no v1", Some(format!("t={t}")), body.clone()),
    ];
    for (name, sig, payload) in cases {
        let (code, _) = error_of(post_webhook(&s.hub, sig, &payload).await).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{name}");
    }
    // Two signature headers, even with a good one.
    let resp = s
        .hub
        .http
        .post(s.hub.url(WEBHOOK))
        .header(STRIPE_SIGNATURE_HEADER, &good)
        .header(STRIPE_SIGNATURE_HEADER, &good)
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(error_of(resp).await.0, StatusCode::BAD_REQUEST);
    // A Fake-signed event is not a Stripe event.
    let (fake_body, fake_sig) = Fake::signed_event(FAKE_CHECKOUT_COMPLETED, &session);
    let resp = s
        .hub
        .http
        .post(s.hub.url(WEBHOOK))
        .header(FAKE_SIGNATURE_HEADER, fake_sig)
        .body(fake_body)
        .send()
        .await
        .unwrap();
    assert_eq!(error_of(resp).await.0, StatusCode::BAD_REQUEST);

    assert_eq!(s.hub.count("payment_events").await, 0);
    assert_eq!(s.status(&id).await, "pending");
}

#[tokio::test]
async fn the_webhook_needs_no_csrf_token_but_other_posts_do() {
    let s = shop().await;
    let (_, id, session) = s.checkout().await;
    // No cookie, no X-CSRF-Token.
    let body = bytes(&s.event("checkout.session.completed", &session, &id));
    received(deliver(&s.hub, &body).await).await;
    // Another POST without a token is still refused (the exemption's exact
    // method and path are unit-tested in `csrf.rs`).
    let resp = s
        .hub
        .http
        .post(s.hub.url("/api/checkout"))
        .header("cookie", &s.owner.cookie)
        .json(&json!({ "offering_id": s.offering }))
        .send()
        .await
        .unwrap();
    assert_error(resp, StatusCode::FORBIDDEN, MSG_BAD_CSRF).await;
    assert_eq!(s.hub.count("orders").await, 1);
}

// ---------------------------------------------------------------------------
// The Fake provider through the webhook

#[tokio::test]
async fn fake_events_through_the_webhook_apply_on_a_local_hub_only() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Acme Recyclers").await;
    let offering = hub.published_offering("Station").await;
    let resp = hub
        .post(
            "/api/checkout",
            json!({ "offering_id": offering }),
            Some(&owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = resp.json().await.unwrap();
    let id = v["order"]["id"].as_str().unwrap().to_string();
    let session: String =
        sqlx::query_scalar("SELECT provider_session_id FROM orders WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&hub.db.pool)
            .await
            .unwrap();
    let (body, sig) = Fake::signed_event(FAKE_CHECKOUT_COMPLETED, &session);
    for _ in 0..2 {
        let resp = hub
            .http
            .post(hub.url(WEBHOOK))
            .header(FAKE_SIGNATURE_HEADER, &sig)
            .body(body.clone())
            .send()
            .await
            .unwrap();
        received(resp).await;
    }
    let status: String = sqlx::query_scalar("SELECT status FROM orders WHERE id = $1::uuid")
        .bind(&id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    assert_eq!(status, "paid");
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT provider, outcome FROM payment_events")
            .fetch_all(&hub.db.pool)
            .await
            .unwrap();
    assert_eq!(rows, [("fake".to_string(), Some("applied".to_string()))]);
    assert_eq!(hub.audited("order.paid").await.len(), 1);

    // A hub that is not local refuses every Fake event.
    let remote = spawn_hub_args(|a| a.public_base_url = "https://hub.example.com".into()).await;
    let resp = remote
        .http
        .post(remote.url(WEBHOOK))
        .header(FAKE_SIGNATURE_HEADER, &sig)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_error(resp, StatusCode::SERVICE_UNAVAILABLE, MSG_FAKE_LOCAL_ONLY).await;
    assert_eq!(remote.count("payment_events").await, 0);
}

// ---------------------------------------------------------------------------
// Billing

#[tokio::test]
async fn billing_opens_after_the_first_payment() {
    let s = shop().await;
    assert_error(
        s.billing(&s.owner).await,
        StatusCode::CONFLICT,
        MSG_NO_BILLING_ACCOUNT,
    )
    .await;
    assert_error(
        s.billing(&s.member).await,
        StatusCode::FORBIDDEN,
        MSG_OWNER_ONLY_BILLING,
    )
    .await;
    // Neither asked Stripe anything.
    assert!(s.mock.requests().is_empty());

    let (_, id, session) = s.checkout().await;
    received(
        deliver(
            &s.hub,
            &bytes(&s.event("checkout.session.completed", &session, &id)),
        )
        .await,
    )
    .await;
    let resp = s.billing(&s.owner).await;
    let code = resp.status();
    let v: Value = resp.json().await.unwrap();
    assert_eq!((code, v), (StatusCode::OK, json!({ "url": BILLING_URL })));
    let requests = s.mock.requests();
    let r = requests.last().unwrap();
    assert_eq!(
        (r.method.as_str(), r.path.as_str()),
        ("POST", "/v1/billing_portal/sessions")
    );
    assert_eq!(
        r.header("authorization"),
        Some(format!("Bearer {STRIPE_SECRET_KEY}").as_str())
    );
    assert_eq!(r.header("stripe-version"), Some(STRIPE_API_VERSION));
    let mut form = r.form.clone();
    form.sort();
    assert_eq!(
        form,
        [
            ("customer".to_string(), "cus_test_1".to_string()),
            ("return_url".to_string(), format!("{BASE}/orders")),
        ]
    );
    assert_error(
        s.billing(&s.member).await,
        StatusCode::FORBIDDEN,
        MSG_OWNER_ONLY_BILLING,
    )
    .await;

    // The next checkout reuses the stored customer.
    s.checkout().await;
    let requests = s.mock.requests();
    let r = requests.last().unwrap();
    assert_eq!(r.path, "/v1/checkout/sessions");
    assert_eq!(r.field("customer"), Some("cus_test_1"));
    assert_eq!(r.field("customer_creation"), None);
}

#[tokio::test]
async fn a_stale_stored_customer_falls_back_to_a_new_one() {
    let s = shop().await;
    // The organization's stored customer was deleted at Stripe.
    sqlx::query(
        "INSERT INTO billing_customers (organization_id, provider, customer_id) \
         VALUES ($1::uuid, 'stripe', 'cus_old')",
    )
    .bind(&s.org)
    .execute(&s.hub.db.pool)
    .await
    .unwrap();
    s.mock.reply_next(
        400,
        json!({"error": {
            "type": "invalid_request_error",
            "code": "resource_missing",
            "param": "customer",
            "message": "No such customer: 'cus_old'",
        }}),
    );
    let (_, id, session) = s.checkout().await;
    let requests = s.mock.requests();
    assert_eq!(requests.len(), 2);
    let (first, second) = (&requests[0], &requests[1]);
    assert_eq!(first.field("customer"), Some("cus_old"));
    assert_eq!(first.field("customer_creation"), None);
    assert_eq!(
        first.header("idempotency-key"),
        Some(format!("wipehub-checkout-{id}").as_str())
    );
    assert_eq!(second.path, "/v1/checkout/sessions");
    assert_eq!(second.field("customer"), None);
    assert_eq!(second.field("customer_creation"), Some("always"));
    assert_eq!(
        second.header("idempotency-key"),
        Some(format!("wipehub-checkout-{id}-new-customer").as_str())
    );
    // Everything else is the same request.
    let rest = |r: &support::stripe_mock::Recorded| {
        let mut f: Vec<(String, String)> = r
            .form
            .iter()
            .filter(|(k, _)| k != "customer" && k != "customer_creation")
            .cloned()
            .collect();
        f.sort();
        f
    };
    assert_eq!(rest(first), rest(second));

    // The paid event names the new customer, which replaces the old one.
    let mut ev = s.event("checkout.session.completed", &session, &id);
    ev["data"]["object"]["customer"] = json!("cus_new");
    received(deliver(&s.hub, &bytes(&ev)).await).await;
    assert_eq!(s.status(&id).await, "paid");
    assert_eq!(
        s.customers().await,
        vec![(s.org.clone(), "stripe".into(), "cus_new".into())]
    );
    let resp = s.billing(&s.owner).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let requests = s.mock.requests();
    let r = requests.last().unwrap();
    assert_eq!(r.path, "/v1/billing_portal/sessions");
    assert_eq!(r.field("customer"), Some("cus_new"));

    // A repeat of the same customer writes nothing.
    let stamps = || async {
        sqlx::query_as::<_, (String, String)>(
            "SELECT created_at::text, updated_at::text FROM billing_customers",
        )
        .fetch_one(&s.hub.db.pool)
        .await
        .unwrap()
    };
    let before = stamps().await;
    let (_, id2, session2) = s.checkout().await;
    assert_eq!(
        s.mock.requests().last().unwrap().field("customer"),
        Some("cus_new")
    );
    let mut ev = s.event("checkout.session.completed", &session2, &id2);
    ev["data"]["object"]["customer"] = json!("cus_new");
    received(deliver(&s.hub, &bytes(&ev)).await).await;
    assert_eq!(s.status(&id2).await, "paid");
    assert_eq!(stamps().await, before);
}

#[tokio::test]
async fn billing_for_a_customer_stripe_deleted_is_409() {
    let s = shop().await;
    sqlx::query(
        "INSERT INTO billing_customers (organization_id, provider, customer_id) \
         VALUES ($1::uuid, 'stripe', 'cus_gone')",
    )
    .bind(&s.org)
    .execute(&s.hub.db.pool)
    .await
    .unwrap();
    s.mock.reply_next(
        400,
        json!({"error": {
            "type": "invalid_request_error",
            "code": "resource_missing",
            "param": "customer",
            "message": "No such customer: 'cus_gone'",
        }}),
    );
    let audits = s.hub.count("audit_log").await;
    assert_error(
        s.billing(&s.owner).await,
        StatusCode::CONFLICT,
        MSG_NO_BILLING_ACCOUNT,
    )
    .await;
    let requests = s.mock.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/billing_portal/sessions");
    assert_eq!(requests[0].field("customer"), Some("cus_gone"));
    // Nothing changes: the next paid event replaces the customer.
    assert_eq!(
        s.customers().await,
        vec![(s.org.clone(), "stripe".into(), "cus_gone".into())]
    );
    assert_eq!(s.hub.count("audit_log").await, audits);

    // Any other refusal is still 502, with no retry.
    for reply in [
        json!({"error": {"type": "invalid_request_error", "code": "resource_missing",
                         "param": "return_url"}}),
        json!({"error": {"type": "invalid_request_error", "code": "parameter_invalid_empty",
                         "param": "customer"}}),
    ] {
        let sent = s.mock.requests().len();
        s.mock.reply_next(400, reply.clone());
        assert_error(
            s.billing(&s.owner).await,
            StatusCode::BAD_GATEWAY,
            MSG_PROVIDER_FAILED,
        )
        .await;
        assert_eq!(s.mock.requests().len(), sent + 1, "{reply}");
    }
}

#[tokio::test]
async fn a_customer_stored_for_another_organization_is_left_alone() {
    let s = shop().await;
    let (_, id, session) = s.checkout().await;
    let paid = s.event("checkout.session.completed", &session, &id);
    received(deliver(&s.hub, &bytes(&paid)).await).await;
    // A second organization's paid event names the first one's customer.
    let rival = s.hub.verified_owner("Rival Inc").await;
    let rival_org = org_of(&s.hub, &rival).await;
    let resp = s
        .hub
        .post(
            "/api/checkout",
            json!({ "offering_id": s.offering }),
            Some(&rival.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = resp.json().await.unwrap();
    let rival_id = v["order"]["id"].as_str().unwrap().to_string();
    let rival_session = s.session_of(&rival_id).await;
    let ev = event(
        "checkout.session.completed",
        &rival_session,
        &rival_id,
        &rival_org,
    );
    assert_eq!(ev["data"]["object"]["customer"], "cus_test_1");
    received(deliver(&s.hub, &bytes(&ev)).await).await;
    assert_eq!(s.outcome(&ev).await.as_deref(), Some("applied"));
    assert_eq!(s.status(&rival_id).await, "paid");
    assert_eq!(
        s.customers().await,
        vec![(s.org.clone(), "stripe".into(), "cus_test_1".into())]
    );
}

#[tokio::test]
async fn any_other_stripe_refusal_of_a_stored_customer_is_502_without_a_retry() {
    let s = shop().await;
    sqlx::query(
        "INSERT INTO billing_customers (organization_id, provider, customer_id) \
         VALUES ($1::uuid, 'stripe', 'cus_old')",
    )
    .bind(&s.org)
    .execute(&s.hub.db.pool)
    .await
    .unwrap();
    let before = (s.hub.count("orders").await, s.hub.count("audit_log").await);
    for reply in [
        // Missing, but not the customer.
        json!({"error": {"type": "invalid_request_error", "code": "resource_missing",
                         "param": "line_items[0][price_data][currency]"}}),
        // The customer, but another problem.
        json!({"error": {"type": "invalid_request_error", "code": "parameter_invalid_empty",
                         "param": "customer"}}),
        json!({"error": {"type": "invalid_request_error"}}),
    ] {
        let sent = s.mock.requests().len();
        s.mock.reply_next(400, reply.clone());
        assert_error(
            s.checkout_resp().await,
            StatusCode::BAD_GATEWAY,
            MSG_PROVIDER_FAILED,
        )
        .await;
        assert_eq!(s.mock.requests().len(), sent + 1, "{reply}");
    }
    // A missing customer whose fallback also fails is 502 after one retry.
    let sent = s.mock.requests().len();
    s.mock.reply_next(
        400,
        json!({"error": {"type": "invalid_request_error", "code": "resource_missing",
                         "param": "customer"}}),
    );
    s.mock
        .reply_next(500, json!({"error": {"type": "api_error"}}));
    assert_error(
        s.checkout_resp().await,
        StatusCode::BAD_GATEWAY,
        MSG_PROVIDER_FAILED,
    )
    .await;
    assert_eq!(s.mock.requests().len(), sent + 2);
    assert_eq!(
        (s.hub.count("orders").await, s.hub.count("audit_log").await),
        before
    );
    assert_eq!(
        s.customers().await,
        vec![(s.org.clone(), "stripe".into(), "cus_old".into())]
    );
}

// ---------------------------------------------------------------------------
// Schema backstop (migration 0012)

#[tokio::test]
async fn the_schema_keeps_events_append_only_and_customers_replace_only() {
    let s = shop().await;
    let (_, id, session) = s.checkout().await;
    let ev = s.event("checkout.session.completed", &session, &id);
    received(deliver(&s.hub, &bytes(&ev)).await).await;
    let pool = &s.hub.db.pool;
    let event_id = ev["id"].as_str().unwrap();
    for sql in [
        "DELETE FROM payment_events WHERE event_id = $1",
        "UPDATE payment_events SET outcome = 'mismatch' WHERE event_id = $1",
        "UPDATE payment_events SET outcome = NULL WHERE event_id = $1",
        "UPDATE payment_events SET event_type = 'x' WHERE event_id = $1",
        "DELETE FROM billing_customers WHERE customer_id = 'cus_test_1' AND $1 <> ''",
        // Only a change of customer (with `updated_at`) is allowed.
        "UPDATE billing_customers SET updated_at = now() + interval '1 day' WHERE $1 <> ''",
        "UPDATE billing_customers SET customer_id = customer_id WHERE $1 <> ''",
        "UPDATE billing_customers SET customer_id = 'cus_x', provider = 'fake' WHERE $1 <> ''",
        "UPDATE billing_customers SET customer_id = 'cus_x', created_at = now() - interval '1 day' \
         WHERE $1 <> ''",
        "UPDATE billing_customers SET customer_id = 'cus_x', \
         organization_id = gen_random_uuid() WHERE $1 <> ''",
    ] {
        sqlx::query(sql)
            .bind(event_id)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    // A fresh row may gain its outcome once; bad values are refused.
    let insert = |event_id: &'static str, sha: &'static str| {
        sqlx::query(
            "INSERT INTO payment_events (provider, event_id, event_type, payload_sha256) \
             VALUES ('stripe', $1, 'invoice.paid', $2)",
        )
        .bind(event_id)
        .bind(sha)
        .execute(pool)
    };
    let sha = "0000000000000000000000000000000000000000000000000000000000000000";
    insert("evt_fresh", sha).await.unwrap();
    sqlx::query("UPDATE payment_events SET outcome = 'ignored' WHERE event_id = 'evt_fresh'")
        .execute(pool)
        .await
        .unwrap();
    insert("evt_bad_sha", "ABC").await.expect_err("bad sha");
    insert(" ", sha).await.expect_err("blank id");
    insert("evt_bad_outcome", sha).await.unwrap();
    sqlx::query("UPDATE payment_events SET outcome = 'paid' WHERE event_id = 'evt_bad_outcome'")
        .execute(pool)
        .await
        .expect_err("an outcome that is not in the list");
    // Another organization cannot take an existing Stripe customer.
    let other = s.hub.verified_owner("Rival Inc").await;
    let other_org = org_of(&s.hub, &other).await;
    sqlx::query(
        "INSERT INTO billing_customers (organization_id, provider, customer_id) \
         VALUES ($1::uuid, 'stripe', 'cus_test_1')",
    )
    .bind(&other_org)
    .execute(pool)
    .await
    .expect_err("customer shared between organizations");
    // Replacing the customer is allowed.
    sqlx::query(
        "UPDATE billing_customers SET customer_id = 'cus_other', updated_at = now() \
         WHERE organization_id = $1::uuid",
    )
    .bind(&s.org)
    .execute(pool)
    .await
    .unwrap();
}
