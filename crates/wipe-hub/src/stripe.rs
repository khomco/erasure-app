//! The Stripe payment provider (ADR-0007 §4; offerings-and-payments story 3).
//!
//! - **Checkout**: `POST /v1/checkout/sessions` in `mode=payment` (annual
//!   offerings are charged once; renewal is story 4), one line item priced
//!   from the order's snapshot, `client_reference_id` and
//!   `metadata[wipehub_order_id]` naming the hub's order (only the metadata
//!   is read back from events, decision 9), `metadata[wipehub_hub]` naming
//!   this hub's public origin (decision 11), and either the
//!   organization's stored `customer` or `customer_creation=always`. No email
//!   or other personal data is sent. It carries `Idempotency-Key:
//!   wipehub-checkout-<order id>`, so a repeated identical request returns
//!   the same session rather than a second one. The hub itself does not
//!   retry, with one exception: when Stripe answers that a stored customer
//!   is missing (`resource_missing` on `customer`, for example after the
//!   vendor deleted it), the request is sent once more with
//!   `customer_creation=always` and the key
//!   `wipehub-checkout-<order id>-new-customer`. The paid event then names the
//!   new customer, which replaces the stored one.
//! - **Billing**: `POST /v1/billing_portal/sessions` for the stored customer,
//!   returning to `{public_base_url}/orders`. Without a stored customer, or
//!   when Stripe answers that the stored one is missing, it is
//!   [`ProviderError::NoBillingAccount`]: the hub learns the customer from
//!   the first paid event and never creates empty Stripe customers.
//! - **Events**: `Stripe-Signature` is verified with Stripe's documented
//!   scheme ([`verify_signature`]) before the body is parsed. Only `id`,
//!   `type` and a few `data.object` fields of a Checkout Session are read
//!   ([`EventKind`] mapping in [`Stripe::verify_event`]). An event names one
//!   of this hub's orders only when its `wipehub_hub` is this hub's origin,
//!   so several hubs can share one Stripe account.
//!
//! Every API call sends `Authorization: Bearer <key>`, `Stripe-Version:
//! 2024-06-20` and a form body, over one `reqwest` client with a 5 s connect
//! and 15 s total timeout. A non-2xx answer, a timeout, or a reply without a
//! usable id or http(s) URL is [`ProviderError::Failed`], whose message never
//! carries the key.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::async_trait;
use axum::http::HeaderMap;
use hmac::Mac;
use serde::Deserialize;
use serde_json::Value;
use url::Url;
use uuid::Uuid;

use crate::csrf::constant_time_eq;
use crate::payments::{
    page_url, CheckoutOrder, CheckoutSession, EventKind, HmacSha256, PaymentProvider,
    ProviderError, VerifiedEvent, MAX_EVENT_FIELD,
};

/// The Stripe API version every request pins.
pub const STRIPE_API_VERSION: &str = "2024-06-20";
/// The header carrying a Stripe event's signature.
pub const STRIPE_SIGNATURE_HEADER: &str = "stripe-signature";
/// How far, in seconds, a signature's `t` may be from now, either way.
pub const STRIPE_SIGNATURE_TOLERANCE: i64 = 300;
/// Connect timeout of every Stripe API call.
pub const STRIPE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Total timeout of every Stripe API call.
pub const STRIPE_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Stripe event types the hub acts on.
pub const CHECKOUT_SESSION_COMPLETED: &str = "checkout.session.completed";
pub const CHECKOUT_SESSION_ASYNC_SUCCEEDED: &str = "checkout.session.async_payment_succeeded";
pub const CHECKOUT_SESSION_ASYNC_FAILED: &str = "checkout.session.async_payment_failed";
pub const CHECKOUT_SESSION_EXPIRED: &str = "checkout.session.expired";

/// The Stripe provider.
pub struct Stripe {
    http: reqwest::Client,
    secret_key: String,
    webhook_secret: String,
    api_base: Url,
    public_base_url: Url,
    /// `public_base_url`'s origin (`scheme://host[:port]`), sent as
    /// `metadata[wipehub_hub]` and required back on events (decision 11).
    hub_origin: String,
}

// Hand-written: the key and the webhook secret never reach a log line.
impl fmt::Debug for Stripe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stripe")
            .field("secret_key", &"<redacted>")
            .field("webhook_secret", &"<redacted>")
            .field("api_base", &self.api_base.as_str())
            .field("public_base_url", &self.public_base_url.as_str())
            .finish()
    }
}

impl Stripe {
    /// A provider with the standard timeouts.
    pub fn new(
        secret_key: String,
        webhook_secret: String,
        api_base: Url,
        public_base_url: Url,
    ) -> anyhow::Result<Self> {
        Self::with_timeouts(
            secret_key,
            webhook_secret,
            api_base,
            public_base_url,
            STRIPE_CONNECT_TIMEOUT,
            STRIPE_REQUEST_TIMEOUT,
        )
    }

    /// A provider with chosen timeouts (tests use short ones).
    pub fn with_timeouts(
        secret_key: String,
        webhook_secret: String,
        api_base: Url,
        public_base_url: Url,
        connect: Duration,
        total: Duration,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(connect)
            .timeout(total)
            .build()
            .map_err(|e| anyhow::anyhow!("cannot build the Stripe HTTP client: {e}"))?;
        let hub_origin = hub_origin(&public_base_url);
        Ok(Self {
            http,
            secret_key,
            webhook_secret,
            api_base,
            public_base_url,
            hub_origin,
        })
    }

    /// This hub's origin as sent in `metadata[wipehub_hub]`.
    pub fn hub_origin(&self) -> &str {
        &self.hub_origin
    }

    /// `{api_base}/{path}`, keeping any base path.
    fn api_url(&self, path: &str) -> Url {
        page_url(&self.api_base, path)
    }

    /// POST a form to the Stripe API and return its JSON reply.
    async fn post(
        &self,
        path: &str,
        form: &[(&str, String)],
        idempotency_key: Option<String>,
    ) -> Result<Value, PostError> {
        let mut req = self
            .http
            .post(self.api_url(path))
            .bearer_auth(&self.secret_key)
            .header("Stripe-Version", STRIPE_API_VERSION)
            .form(form);
        if let Some(key) = idempotency_key {
            req = req.header("Idempotency-Key", key);
        }
        // reqwest's errors name the URL, never the headers, so the key
        // cannot reach these messages.
        let resp = req.send().await.map_err(|e| PostError {
            message: format!("POST {path}: {e}"),
            status: None,
            code: None,
            param: None,
        })?;
        let status = resp.status();
        let body: Option<Value> = resp.json().await.ok();
        if !status.is_success() {
            // Stripe's `error.type`, `error.code` and `error.param` only; its
            // message can quote part of a key.
            let err = body.as_ref().map(|b| &b["error"]);
            let field = |name: &str| err.and_then(|e| e[name].as_str()).map(str::to_string);
            let (kind, code, param) = (field("type"), field("code"), field("param"));
            return Err(PostError {
                message: format!(
                    "POST {path} answered {status} (type {}, code {})",
                    kind.as_deref().unwrap_or("-"),
                    code.as_deref().unwrap_or("-"),
                ),
                status: Some(status.as_u16()),
                code,
                param,
            });
        }
        body.ok_or_else(|| PostError {
            message: format!("POST {path} answered no JSON"),
            status: Some(status.as_u16()),
            code: None,
            param: None,
        })
    }
}

/// A failed Stripe API call: a message that never carries the key or
/// Stripe's own error message, plus the parts of Stripe's error a caller
/// may act on.
#[derive(Debug)]
struct PostError {
    message: String,
    status: Option<u16>,
    code: Option<String>,
    param: Option<String>,
}

impl PostError {
    /// Stripe's answer that the `customer` sent does not exist.
    fn is_missing_customer(&self) -> bool {
        self.status == Some(400)
            && self.code.as_deref() == Some("resource_missing")
            && self.param.as_deref() == Some("customer")
    }
}

impl From<PostError> for ProviderError {
    fn from(e: PostError) -> Self {
        ProviderError::Failed(e.message)
    }
}

/// The origin (`scheme://host[:port]`, default port omitted) of a hub
/// reached at `public_base_url`: the `metadata[wipehub_hub]` value.
pub fn hub_origin(public_base_url: &Url) -> String {
    public_base_url.origin().ascii_serialization()
}

/// The `url` of a Stripe reply, if it is an http(s) URL.
fn reply_url(reply: &Value, path: &str) -> Result<Url, ProviderError> {
    reply["url"]
        .as_str()
        .and_then(|u| Url::parse(u).ok())
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .ok_or_else(|| ProviderError::Failed(format!("POST {path} answered no usable url")))
}

#[async_trait]
impl PaymentProvider for Stripe {
    fn name(&self) -> &'static str {
        "stripe"
    }

    async fn create_checkout(
        &self,
        order: &CheckoutOrder,
    ) -> Result<CheckoutSession, ProviderError> {
        const PATH: &str = "v1/checkout/sessions";
        let page = page_url(&self.public_base_url, &format!("orders/{}", order.order_id));
        let form: Vec<(&'static str, String)> = vec![
            ("mode", "payment".into()),
            ("client_reference_id", order.order_id.to_string()),
            ("success_url", page.to_string()),
            ("cancel_url", page.to_string()),
            ("line_items[0][quantity]", "1".into()),
            (
                "line_items[0][price_data][currency]",
                order.currency.to_ascii_lowercase(),
            ),
            (
                "line_items[0][price_data][unit_amount]",
                order.price_minor.to_string(),
            ),
            (
                "line_items[0][price_data][product_data][name]",
                order.offering_name.clone(),
            ),
            ("metadata[wipehub_order_id]", order.order_id.to_string()),
            (
                "metadata[wipehub_organization_id]",
                order.organization_id.to_string(),
            ),
            ("metadata[wipehub_hub]", self.hub_origin.clone()),
        ];
        let new_customer = |mut form: Vec<(&'static str, String)>| {
            form.push(("customer_creation", "always".into()));
            form
        };
        let reply = match &order.customer_id {
            None => {
                self.post(
                    PATH,
                    &new_customer(form),
                    Some(format!("wipehub-checkout-{}", order.order_id)),
                )
                .await?
            }
            Some(customer) => {
                let mut with_customer = form.clone();
                with_customer.push(("customer", customer.clone()));
                match self
                    .post(
                        PATH,
                        &with_customer,
                        Some(format!("wipehub-checkout-{}", order.order_id)),
                    )
                    .await
                {
                    Ok(reply) => reply,
                    Err(e) if e.is_missing_customer() => {
                        // Decision 10: the stored customer is gone at
                        // Stripe; buy as a new customer, whom the paid
                        // event then stores in its place.
                        tracing::warn!(
                            order_id = %order.order_id,
                            organization_id = %order.organization_id,
                            stale_customer = %customer,
                            "Stripe no longer knows the stored customer; checking out \
                             as a new customer"
                        );
                        self.post(
                            PATH,
                            &new_customer(form),
                            Some(format!("wipehub-checkout-{}-new-customer", order.order_id)),
                        )
                        .await?
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        };
        let session_id = reply["id"]
            .as_str()
            .filter(|id| !id.trim().is_empty() && id.chars().count() <= MAX_EVENT_FIELD)
            .ok_or_else(|| ProviderError::Failed(format!("POST {PATH} answered no usable id")))?
            .to_string();
        let url = reply_url(&reply, PATH)?;
        Ok(CheckoutSession { session_id, url })
    }

    async fn billing_portal(
        &self,
        _organization_id: Uuid,
        customer_id: Option<&str>,
    ) -> Result<Url, ProviderError> {
        const PATH: &str = "v1/billing_portal/sessions";
        let customer = customer_id.ok_or(ProviderError::NoBillingAccount)?;
        let form = [
            ("customer", customer.to_string()),
            (
                "return_url",
                page_url(&self.public_base_url, "orders").to_string(),
            ),
        ];
        let reply = match self.post(PATH, &form, None).await {
            Ok(reply) => reply,
            // Decision 10: a customer Stripe no longer knows is the same as
            // none; the next paid checkout stores a new one.
            Err(e) if e.is_missing_customer() => {
                tracing::warn!(
                    stale_customer = %customer,
                    "Stripe no longer knows the stored customer; billing opens \
                     after the next payment"
                );
                return Err(ProviderError::NoBillingAccount);
            }
            Err(e) => return Err(e.into()),
        };
        reply_url(&reply, PATH)
    }

    fn verify_event(
        &self,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<VerifiedEvent, ProviderError> {
        let mut values = headers.get_all(STRIPE_SIGNATURE_HEADER).iter();
        let (Some(value), None) = (values.next(), values.next()) else {
            return Err(ProviderError::Rejected(
                "one Stripe-Signature header is required",
            ));
        };
        let header = value
            .to_str()
            .map_err(|_| ProviderError::Rejected("the Stripe-Signature header is malformed"))?;
        verify_signature(header, body, &self.webhook_secret, unix_now())?;
        parse_event(body, &self.hub_origin)
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn mac_of(secret: &str, t: &str, body: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(t.as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.finalize().into_bytes().to_vec()
}

/// A `Stripe-Signature` value for `body` signed at unix time `t` with
/// `secret` (the whole `whsec_…` string), as Stripe sends it. For tests and
/// local tooling.
pub fn signature_header(secret: &str, t: i64, body: &[u8]) -> String {
    let t = t.to_string();
    format!("t={t},v1={}", hex::encode(mac_of(secret, &t, body)))
}

/// Verify a `Stripe-Signature` header value against `body` at unix time
/// `now`: exactly one `t=<unix>` and at least one `v1=<hex>`; the expected
/// `v1` is the hex HMAC-SHA256 of `"{t}.{body}"` keyed with the whole
/// `secret`; any `v1` may match (compared in constant time); and `t` must be
/// within [`STRIPE_SIGNATURE_TOLERANCE`] seconds of `now`. Other schemes
/// (`v0`) are ignored. Anything else is [`ProviderError::Rejected`].
pub fn verify_signature(
    header: &str,
    body: &[u8],
    secret: &str,
    now: i64,
) -> Result<(), ProviderError> {
    const MALFORMED: ProviderError =
        ProviderError::Rejected("the Stripe-Signature header is malformed");
    let mut t: Option<(&str, i64)> = None;
    let mut candidates = Vec::new();
    for item in header.split(',') {
        let (key, value) = item.trim().split_once('=').ok_or(MALFORMED)?;
        match key {
            "t" => {
                let parsed = value.parse::<i64>().map_err(|_| MALFORMED)?;
                if t.replace((value, parsed)).is_some() {
                    return Err(MALFORMED);
                }
            }
            "v1" => candidates.push(hex::decode(value).map_err(|_| MALFORMED)?),
            _ => {}
        }
    }
    let (t_raw, t) = t.ok_or(MALFORMED)?;
    if candidates.is_empty() {
        return Err(MALFORMED);
    }
    let expected = mac_of(secret, t_raw, body);
    let matched = candidates
        .iter()
        .fold(false, |acc, c| constant_time_eq(&expected, c) | acc);
    if !matched {
        return Err(ProviderError::Rejected("the signature does not match"));
    }
    if now
        .checked_sub(t)
        .map_or(true, |d| d.abs() > STRIPE_SIGNATURE_TOLERANCE)
    {
        return Err(ProviderError::Rejected(
            "the signature timestamp is outside the tolerance",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct StripeEvent {
    id: String,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    data: Option<StripeData>,
}

#[derive(Deserialize)]
struct StripeData {
    #[serde(default)]
    object: Value,
}

/// The `data.object` fields of a Checkout Session the hub reads; anything
/// else in it is ignored.
struct Session {
    id: Option<String>,
    payment_status: Option<String>,
    amount_subtotal: Option<i64>,
    currency: Option<String>,
    customer: Option<String>,
    metadata_order_id: Option<String>,
    metadata_hub: Option<String>,
}

impl Session {
    fn from_object(o: &Value) -> Self {
        let text = |v: &Value| v.as_str().map(str::to_string);
        Self {
            id: text(&o["id"]).filter(|s| !s.trim().is_empty()),
            payment_status: text(&o["payment_status"]),
            amount_subtotal: o["amount_subtotal"].as_i64(),
            currency: text(&o["currency"]),
            // A string id; an expanded customer object carries it as `id`.
            customer: text(&o["customer"]).or_else(|| text(&o["customer"]["id"])),
            metadata_order_id: text(&o["metadata"]["wipehub_order_id"]),
            metadata_hub: text(&o["metadata"]["wipehub_hub"]),
        }
    }

    /// The hub order named by `metadata[wipehub_order_id]`, when it is a
    /// UUID and `metadata[wipehub_hub]` is `hub_origin` (this hub).
    /// `client_reference_id` is sent but never read back (decision 9):
    /// another app on a shared Stripe account may put its own UUIDs there,
    /// and reading them would make its paid events look like the hub's own
    /// order that is not committed yet (503 and days of Stripe retries).
    /// Another hub's session (decision 11) has the same problem, hence the
    /// origin check.
    fn order_ref(&self, hub_origin: &str) -> Option<Uuid> {
        if self.metadata_hub.as_deref() != Some(hub_origin) {
            return None;
        }
        self.metadata_order_id
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok())
    }
}

/// Map a verified Stripe event body to a [`VerifiedEvent`] for the hub whose
/// origin is `hub_origin`.
fn parse_event(body: &[u8], hub_origin: &str) -> Result<VerifiedEvent, ProviderError> {
    let event: StripeEvent = serde_json::from_slice(body)
        .map_err(|_| ProviderError::Rejected("the event is not a Stripe event"))?;
    if !event.event_type.starts_with("checkout.session.") {
        let kind = EventKind::Ignored {
            event_type: event.event_type.clone(),
        };
        return VerifiedEvent::new(event.id, "stripe", event.event_type, kind, None, None, None);
    }
    let object = event.data.map(|d| d.object).unwrap_or(Value::Null);
    let session = Session::from_object(&object);
    let Some(session_id) = session.id.clone() else {
        return Err(ProviderError::Rejected(
            "the checkout session event has no data.object.id",
        ));
    };
    let kind = match event.event_type.as_str() {
        CHECKOUT_SESSION_COMPLETED if session.payment_status.as_deref() == Some("paid") => {
            EventKind::CheckoutCompleted { session_id }
        }
        CHECKOUT_SESSION_ASYNC_SUCCEEDED => EventKind::CheckoutCompleted { session_id },
        CHECKOUT_SESSION_EXPIRED | CHECKOUT_SESSION_ASYNC_FAILED => {
            EventKind::CheckoutExpired { session_id }
        }
        other => EventKind::Ignored {
            event_type: other.to_string(),
        },
    };
    let amount = match (session.amount_subtotal, &session.currency) {
        (Some(minor), Some(currency)) => Some((minor, currency.to_ascii_uppercase())),
        _ => None,
    };
    let order_ref = session.order_ref(hub_origin);
    VerifiedEvent::new(
        event.id,
        "stripe",
        event.event_type,
        kind,
        session.customer,
        amount,
        order_ref,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SECRET: &str = "whsec_test_secret";
    const HUB: &str = "http://127.0.0.1:7900";

    fn stripe() -> Stripe {
        Stripe::new(
            "sk_test_key".into(),
            SECRET.into(),
            Url::parse("http://127.0.0.1:9").unwrap(),
            Url::parse(HUB).unwrap(),
        )
        .unwrap()
    }

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(STRIPE_SIGNATURE_HEADER, value.parse().unwrap());
        h
    }

    fn signed(body: &[u8]) -> HeaderMap {
        headers(&signature_header(SECRET, unix_now(), body))
    }

    fn session_event(event_type: &str, object: Value) -> Vec<u8> {
        json!({
            "id": "evt_test_1",
            "object": "event",
            "type": event_type,
            "data": {"object": object},
            "livemode": false,
        })
        .to_string()
        .into_bytes()
    }

    fn paid_session() -> Value {
        json!({
            "id": "cs_test_1",
            "object": "checkout.session",
            "payment_status": "paid",
            "amount_subtotal": 120000,
            "currency": "usd",
            "customer": "cus_test_1",
            "client_reference_id": "7f0c1e2a-0000-4000-8000-000000000001",
            "metadata": {
                "wipehub_order_id": "7f0c1e2a-0000-4000-8000-000000000001",
                "wipehub_hub": HUB,
            },
            "unknown_field": {"nested": [1, 2, 3]},
        })
    }

    fn rejected(r: Result<impl fmt::Debug, ProviderError>) -> bool {
        matches!(r, Err(ProviderError::Rejected(_)))
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let body = session_event(CHECKOUT_SESSION_COMPLETED, paid_session());
        let ev = stripe().verify_event(&signed(&body), &body).unwrap();
        assert_eq!(ev.id(), "evt_test_1");
        assert_eq!(ev.provider(), "stripe");
        assert_eq!(ev.event_type(), CHECKOUT_SESSION_COMPLETED);
        assert_eq!(
            ev.kind(),
            &EventKind::CheckoutCompleted {
                session_id: "cs_test_1".into()
            }
        );
        assert_eq!(ev.customer_id(), Some("cus_test_1"));
        assert_eq!(ev.amount(), Some((120000, "USD")));
        assert_eq!(
            ev.order_ref(),
            Some(Uuid::parse_str("7f0c1e2a-0000-4000-8000-000000000001").unwrap())
        );
    }

    #[test]
    fn a_known_answer_signature_verifies() {
        // Computed independently of this code:
        // printf '%s' '1700000000.{"id":"evt_test"}' \
        //   | openssl dgst -sha256 -hmac whsec_test_secret
        let header =
            "t=1700000000,v1=13941114bb88ac44a76abcfddea5b92aa6182a4b63d8be3aae908a616083bd7e";
        let body = br#"{"id":"evt_test"}"#;
        assert!(verify_signature(header, body, "whsec_test_secret", 1700000000).is_ok());
        assert_eq!(
            signature_header("whsec_test_secret", 1700000000, body),
            header
        );
        assert!(rejected(verify_signature(
            header,
            body,
            "whsec_other",
            1700000000
        )));
    }

    #[test]
    fn any_of_several_v1_signatures_matches() {
        let body = b"{}";
        let now = 1_700_000_000;
        let good = signature_header(SECRET, now, body);
        let good_v1 = good.split_once(",v1=").unwrap().1;
        let other = signature_header("whsec_other", now, body);
        let other_v1 = other.split_once(",v1=").unwrap().1;
        let header = format!("t={now},v1={other_v1},v0=abc,v1={good_v1}");
        assert!(verify_signature(&header, body, SECRET, now).is_ok());
        let header = format!("t={now}, v1={good_v1}, v1={other_v1}");
        assert!(verify_signature(&header, body, SECRET, now).is_ok());
    }

    #[test]
    fn a_wrong_secret_or_tampered_body_is_refused() {
        let body = session_event(CHECKOUT_SESSION_COMPLETED, paid_session());
        let wrong = headers(&signature_header("whsec_wrong", unix_now(), &body));
        assert!(rejected(stripe().verify_event(&wrong, &body)));
        let tampered = String::from_utf8(body.clone())
            .unwrap()
            .replace("120000", "1");
        assert!(rejected(
            stripe().verify_event(&signed(&body), tampered.as_bytes())
        ));
    }

    #[test]
    fn stale_and_future_timestamps_are_refused() {
        let body = b"{}";
        let now = 1_700_000_000;
        for t in [now - 301, now + 301, 0, i64::MIN, i64::MAX] {
            let header = signature_header(SECRET, t, body);
            assert!(
                rejected(verify_signature(&header, body, SECRET, now)),
                "{t}"
            );
        }
        for t in [now - 300, now + 300, now] {
            let header = signature_header(SECRET, t, body);
            assert!(verify_signature(&header, body, SECRET, now).is_ok(), "{t}");
        }
    }

    #[test]
    fn missing_duplicated_or_malformed_headers_are_refused() {
        let body = session_event(CHECKOUT_SESSION_COMPLETED, paid_session());
        assert!(rejected(stripe().verify_event(&HeaderMap::new(), &body)));
        let good = signature_header(SECRET, unix_now(), &body);
        let mut twice = headers(&good);
        twice.append(STRIPE_SIGNATURE_HEADER, good.parse().unwrap());
        assert!(rejected(stripe().verify_event(&twice, &body)));
        let now = unix_now();
        let v1 = good.split_once(",v1=").unwrap().1.to_string();
        for bad in [
            String::new(),
            "garbage".into(),
            format!("v1={v1}"),
            format!("t={now}"),
            format!("t=abc,v1={v1}"),
            format!("t={now},t={now},v1={v1}"),
            format!("t={now},v1=zz"),
            format!("t={now},v1={v1},junk"),
        ] {
            assert!(
                rejected(stripe().verify_event(&headers(&bad), &body)),
                "{bad}"
            );
        }
    }

    fn kind_of(event_type: &str, object: Value) -> Result<VerifiedEvent, ProviderError> {
        let body = session_event(event_type, object);
        stripe().verify_event(&signed(&body), &body)
    }

    #[test]
    fn event_types_map_to_kinds() {
        let completed = |s: &str| EventKind::CheckoutCompleted {
            session_id: s.into(),
        };
        let expired = |s: &str| EventKind::CheckoutExpired {
            session_id: s.into(),
        };
        let mut unpaid = paid_session();
        unpaid["payment_status"] = json!("unpaid");
        for (event_type, object, want) in [
            (
                CHECKOUT_SESSION_COMPLETED,
                paid_session(),
                completed("cs_test_1"),
            ),
            (
                CHECKOUT_SESSION_ASYNC_SUCCEEDED,
                paid_session(),
                completed("cs_test_1"),
            ),
            (
                CHECKOUT_SESSION_EXPIRED,
                paid_session(),
                expired("cs_test_1"),
            ),
            (
                CHECKOUT_SESSION_ASYNC_FAILED,
                paid_session(),
                expired("cs_test_1"),
            ),
            (
                CHECKOUT_SESSION_COMPLETED,
                unpaid,
                EventKind::Ignored {
                    event_type: CHECKOUT_SESSION_COMPLETED.into(),
                },
            ),
            (
                "payment_intent.succeeded",
                json!({"id": "pi_1", "object": "payment_intent", "amount": "odd"}),
                EventKind::Ignored {
                    event_type: "payment_intent.succeeded".into(),
                },
            ),
            (
                "invoice.paid",
                json!(null),
                EventKind::Ignored {
                    event_type: "invoice.paid".into(),
                },
            ),
        ] {
            let ev = kind_of(event_type, object).unwrap();
            assert_eq!(ev.kind(), &want, "{event_type}");
            assert_eq!(ev.event_type(), event_type);
        }
    }

    #[test]
    fn a_checkout_session_event_without_an_id_is_refused() {
        for event_type in [
            CHECKOUT_SESSION_COMPLETED,
            CHECKOUT_SESSION_EXPIRED,
            "checkout.session.something_new",
        ] {
            let mut object = paid_session();
            object.as_object_mut().unwrap().remove("id");
            assert!(rejected(kind_of(event_type, object)), "{event_type}");
            assert!(rejected(kind_of(event_type, json!({"id": ""}))));
        }
        let body = json!({"id": "evt_1", "type": CHECKOUT_SESSION_COMPLETED}).to_string();
        assert!(rejected(
            stripe().verify_event(&signed(body.as_bytes()), body.as_bytes())
        ));
        // Not an event at all.
        let body = b"[1,2]";
        assert!(rejected(stripe().verify_event(&signed(body), body)));
    }

    #[test]
    fn the_order_ref_reads_only_the_metadata_and_ignores_non_uuids() {
        let a = "7f0c1e2a-0000-4000-8000-00000000000a";
        let b = "7f0c1e2a-0000-4000-8000-00000000000b";
        let with = |client: Value, metadata: Value| {
            let mut s = paid_session();
            s["client_reference_id"] = client;
            s["metadata"] = metadata;
            kind_of(CHECKOUT_SESSION_COMPLETED, s).unwrap().order_ref()
        };
        let uuid = |s: &str| Some(Uuid::parse_str(s).unwrap());
        let ours = |id: &str| json!({"wipehub_order_id": id, "wipehub_hub": HUB});
        assert_eq!(with(json!(b), ours(a)), uuid(a));
        assert_eq!(with(json!(null), ours(a)), uuid(a));
        // A UUID `client_reference_id` alone (another app on a shared
        // Stripe account) is not the hub's order.
        assert_eq!(with(json!(b), json!({"wipehub_hub": HUB})), None);
        assert_eq!(with(json!(b), json!({})), None);
        assert_eq!(with(json!(b), json!(null)), None);
        assert_eq!(with(json!(null), json!({})), None);
        assert_eq!(with(json!(b), ours("x")), None);
    }

    #[test]
    fn the_order_ref_requires_this_hubs_origin() {
        let a = "7f0c1e2a-0000-4000-8000-00000000000a";
        let with = |metadata: Value| {
            let mut s = paid_session();
            s["metadata"] = metadata;
            kind_of(CHECKOUT_SESSION_COMPLETED, s).unwrap().order_ref()
        };
        assert_eq!(
            with(json!({"wipehub_order_id": a, "wipehub_hub": HUB})),
            Some(Uuid::parse_str(a).unwrap())
        );
        // Another hub on the same Stripe account (decision 11).
        for other in [
            json!("http://127.0.0.1:7901"),
            json!("https://staging.example.com"),
            json!("http://127.0.0.1:7900/"),
            json!(""),
            json!(null),
            json!(42),
        ] {
            assert_eq!(
                with(json!({"wipehub_order_id": a, "wipehub_hub": other})),
                None,
                "{other}"
            );
        }
        assert_eq!(with(json!({"wipehub_order_id": a})), None);
    }

    #[test]
    fn the_hub_origin_is_scheme_host_and_port() {
        let origin = |u: &str| hub_origin(&Url::parse(u).unwrap());
        assert_eq!(origin("http://127.0.0.1:7900"), "http://127.0.0.1:7900");
        assert_eq!(
            origin("https://hub.example.com/portal/"),
            "https://hub.example.com"
        );
        assert_eq!(
            origin("https://hub.example.com:443/"),
            "https://hub.example.com"
        );
        assert_eq!(
            origin("https://hub.example.com:8443/x"),
            "https://hub.example.com:8443"
        );
        assert_eq!(stripe().hub_origin(), HUB);
    }

    #[test]
    fn a_blank_or_oversized_customer_still_verifies_without_one() {
        for customer in [
            json!(""),
            json!("   "),
            json!("c".repeat(MAX_EVENT_FIELD + 1)),
        ] {
            let mut s = paid_session();
            s["customer"] = customer.clone();
            let ev = kind_of(CHECKOUT_SESSION_COMPLETED, s).unwrap();
            assert_eq!(ev.customer_id(), None, "{customer}");
            assert_eq!(
                ev.kind(),
                &EventKind::CheckoutCompleted {
                    session_id: "cs_test_1".into()
                }
            );
        }
        // An expanded customer object carries its id as `customer.id`.
        let mut s = paid_session();
        s["customer"] = json!({"id": "cus_expanded", "object": "customer"});
        let ev = kind_of(CHECKOUT_SESSION_COMPLETED, s).unwrap();
        assert_eq!(ev.customer_id(), Some("cus_expanded"));
        let mut s = paid_session();
        s["customer"] = json!("c".repeat(MAX_EVENT_FIELD));
        let ev = kind_of(CHECKOUT_SESSION_COMPLETED, s).unwrap();
        assert_eq!(ev.customer_id().map(str::len), Some(MAX_EVENT_FIELD));
    }

    #[test]
    fn debug_redacts_the_secrets() {
        let debug = format!("{:?}", stripe());
        assert!(!debug.contains("sk_test_key"), "{debug}");
        assert!(!debug.contains(SECRET), "{debug}");
    }

    #[tokio::test]
    async fn billing_without_a_customer_is_no_billing_account() {
        assert!(matches!(
            stripe().billing_portal(Uuid::nil(), None).await,
            Err(ProviderError::NoBillingAccount)
        ));
    }

    #[tokio::test]
    async fn a_silent_api_times_out_as_failed() {
        // Accepts connections (the OS backlog) but never answers.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}", silent.local_addr().unwrap())).unwrap();
        let p = Stripe::with_timeouts(
            "sk_test_key".into(),
            SECRET.into(),
            base,
            Url::parse("http://127.0.0.1:7900").unwrap(),
            Duration::from_millis(200),
            Duration::from_millis(300),
        )
        .unwrap();
        let order = CheckoutOrder {
            order_id: Uuid::new_v4(),
            organization_id: Uuid::new_v4(),
            offering_id: Uuid::new_v4(),
            offering_name: "Station".into(),
            price_minor: 120000,
            currency: "USD".into(),
            interval: "annual".into(),
            customer_id: None,
        };
        let started = std::time::Instant::now();
        match p.create_checkout(&order).await {
            Err(ProviderError::Failed(m)) => assert!(!m.contains("sk_test_key"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(matches!(
            p.billing_portal(Uuid::nil(), Some("cus_1")).await,
            Err(ProviderError::Failed(_))
        ));
        drop(silent);
    }
}
