//! The Stripe payment provider (ADR-0007 §4; offerings-and-payments story 3).
//!
//! - **Checkout**: `POST /v1/checkout/sessions` in `mode=payment` for a
//!   one-off offering, or `mode=subscription` with a yearly price for an
//!   annual one (story 4, owner decision D15), one line item priced
//!   from the order's snapshot, `client_reference_id` and
//!   `metadata[wipehub_order_id]` naming the hub's order (only the metadata
//!   is read back from events, decision 9), `metadata[wipehub_hub]` naming
//!   this hub's public origin (decision 11), and either the
//!   organization's stored `customer` or, for a one-off,
//!   `customer_creation=always` (Stripe refuses it in subscription mode,
//!   where it always creates a customer). A subscription also carries
//!   `subscription_data[metadata]` (the same three values, so its invoices
//!   and subscription events name the order) and the owner-approved
//!   auto-renewal notice as `custom_text[submit][message]`. No email
//!   or other personal data is sent. It carries `Idempotency-Key:
//!   wipehub-checkout-<order id>`, so a repeated identical request returns
//!   the same session rather than a second one. The hub itself does not
//!   retry, with one exception: when Stripe answers that a stored customer
//!   is missing (`resource_missing` on `customer`, for example after the
//!   vendor deleted it), the request is sent once more with
//!   no `customer` (and `customer_creation=always` for a one-off) and the key
//!   `wipehub-checkout-<order id>-new-customer`. The paid event then names the
//!   new customer, which replaces the stored one.
//! - **Billing**: `POST /v1/billing_portal/sessions` for the stored customer,
//!   returning to `{public_base_url}/orders`. Without a stored customer, or
//!   when Stripe answers that the stored one is missing, it is
//!   [`ProviderError::NoBillingAccount`]: the hub learns the customer from
//!   the first paid event and never creates empty Stripe customers.
//! - **Events**: `Stripe-Signature` is verified with Stripe's documented
//!   scheme ([`verify_signature`]) before the body is parsed. Only `id`,
//!   `type`, `created` and a few `data.object` fields of a Checkout Session,
//!   an Invoice, a Subscription or a Charge are read ([`EventKind`] mapping
//!   in [`parse_event`]). An event names one of this hub's orders only when
//!   its `wipehub_hub` is this hub's origin, so several hubs can share one
//!   Stripe account.
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
    page_url, CheckoutOrder, CheckoutSession, EventFields, EventKind, HmacSha256, PaymentProvider,
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
pub const INVOICE_PAID: &str = "invoice.paid";
pub const CUSTOMER_SUBSCRIPTION_UPDATED: &str = "customer.subscription.updated";
pub const CUSTOMER_SUBSCRIPTION_DELETED: &str = "customer.subscription.deleted";
pub const CHARGE_REFUNDED: &str = "charge.refunded";
/// The `billing_reason` of a subscription's renewal invoice.
pub const BILLING_REASON_CYCLE: &str = "subscription_cycle";

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

    /// The Checkout Session form for `order`: `mode=payment` for a one-off,
    /// or `mode=subscription` with a yearly price, `subscription_data`
    /// metadata and the auto-renewal notice for an annual offering. With
    /// `customer`, it is sent; without, a one-off asks Stripe to create one
    /// (`customer_creation=always`, which Stripe refuses in subscription
    /// mode, where it always creates one).
    fn checkout_form(
        &self,
        order: &CheckoutOrder,
        customer: Option<&str>,
    ) -> Vec<(&'static str, String)> {
        let page = page_url(&self.public_base_url, &format!("orders/{}", order.order_id));
        let annual = order.is_annual();
        let mut form: Vec<(&'static str, String)> = vec![
            (
                "mode",
                if annual { "subscription" } else { "payment" }.into(),
            ),
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
        if annual {
            form.extend([
                (
                    "line_items[0][price_data][recurring][interval]",
                    "year".to_string(),
                ),
                (
                    "subscription_data[metadata][wipehub_order_id]",
                    order.order_id.to_string(),
                ),
                (
                    "subscription_data[metadata][wipehub_organization_id]",
                    order.organization_id.to_string(),
                ),
                (
                    "subscription_data[metadata][wipehub_hub]",
                    self.hub_origin.clone(),
                ),
            ]);
            if let Some(notice) = &order.renewal_notice {
                form.push(("custom_text[submit][message]", notice.one_line()));
            }
        }
        match customer {
            Some(customer) => form.push(("customer", customer.to_string())),
            None if !annual => form.push(("customer_creation", "always".into())),
            None => {}
        }
        form
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
        let new_customer = |customer: Option<&str>| self.checkout_form(order, customer);
        let reply = match &order.customer_id {
            None => {
                self.post(
                    PATH,
                    &new_customer(None),
                    Some(format!("wipehub-checkout-{}", order.order_id)),
                )
                .await?
            }
            Some(customer) => {
                match self
                    .post(
                        PATH,
                        &new_customer(Some(customer)),
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
                            &new_customer(None),
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

pub(crate) fn unix_now() -> i64 {
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
    created: Option<i64>,
    #[serde(default)]
    data: Option<StripeData>,
}

#[derive(Deserialize)]
struct StripeData {
    #[serde(default)]
    object: Value,
}

/// A non-blank string field, or the `id` of an expanded object in its place.
fn id_of(v: &Value) -> Option<String> {
    v.as_str()
        .or_else(|| v["id"].as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

/// The hub order a `metadata` object names: `wipehub_order_id`, when it is a
/// UUID and `wipehub_hub` is `hub_origin` (this hub).
/// `client_reference_id` is sent but never read back (decision 9):
/// another app on a shared Stripe account may put its own UUIDs there,
/// and reading them would make its paid events look like the hub's own
/// order that is not committed yet (503 and days of Stripe retries).
/// Another hub's session (decision 11) has the same problem, hence the
/// origin check.
fn order_ref_of(metadata: &Value, hub_origin: &str) -> Option<Uuid> {
    if metadata["wipehub_hub"].as_str() != Some(hub_origin) {
        return None;
    }
    metadata["wipehub_order_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
}

/// `(minor, upper-case currency)` from an amount field and `currency`.
fn amount_of(minor: &Value, currency: &Value) -> Option<(i64, String)> {
    Some((minor.as_i64()?, currency.as_str()?.to_ascii_uppercase()))
}

/// Map a verified Stripe event body to a [`VerifiedEvent`] for the hub whose
/// origin is `hub_origin`. Only the fields named here are read:
///
/// - `checkout.session.*` (the object must have an `id`): `completed` with
///   `payment_status=paid`, or `async_payment_succeeded`, is
///   [`EventKind::CheckoutCompleted`]; `expired` or `async_payment_failed` is
///   [`EventKind::CheckoutExpired`]; anything else is ignored. Also
///   `amount_subtotal`/`currency`, `customer`, `subscription`, and the
///   payment reference (`payment_intent`, else `invoice`).
/// - `invoice.paid` with `billing_reason=subscription_cycle` and a
///   subscription (`subscription`, else `parent.subscription_details
///   .subscription` in API versions after 2024-06-20, decision 9) is
///   [`EventKind::RenewalPaid`] (the invoice must have an `id`), with
///   `subtotal`/`currency` and the order from `subscription_details.metadata`,
///   else `parent.subscription_details.metadata`. A renewal invoice with
///   neither subscription is ignored and logged at error; any other
///   `invoice.paid` is ignored (the first invoice is paid through the
///   checkout).
/// - `customer.subscription.updated` is [`EventKind::RenewalChanged`], renewing
///   unless `cancel_at_period_end` or `cancel_at` is set, at the event's
///   `created`; `customer.subscription.deleted` is
///   [`EventKind::SubscriptionEnded`]. Both need the subscription's `id` and
///   read the order from its `metadata`.
/// - `charge.refunded` with `refunded=true` (a full refund) is
///   [`EventKind::Refunded`] naming the charge's `payment_intent` and
///   `invoice`; a full refund naming neither is ignored and logged at warn
///   (decision 11), and a partial refund is ignored.
/// - Every other type is ignored.
fn parse_event(body: &[u8], hub_origin: &str) -> Result<VerifiedEvent, ProviderError> {
    let event: StripeEvent = serde_json::from_slice(body)
        .map_err(|_| ProviderError::Rejected("the event is not a Stripe event"))?;
    let o = event.data.map(|d| d.object).unwrap_or(Value::Null);
    let mut fields = EventFields::default();
    let ignored = || EventKind::Ignored {
        event_type: event.event_type.clone(),
    };
    let kind = match event.event_type.as_str() {
        t if t.starts_with("checkout.session.") => {
            let Some(session_id) = id_of(&o["id"]) else {
                return Err(ProviderError::Rejected(
                    "the checkout session event has no data.object.id",
                ));
            };
            fields.amount = amount_of(&o["amount_subtotal"], &o["currency"]);
            fields.customer_id = id_of(&o["customer"]);
            fields.order_ref = order_ref_of(&o["metadata"], hub_origin);
            fields.subscription_id = id_of(&o["subscription"]);
            fields.payment_ref = id_of(&o["payment_intent"]).or_else(|| id_of(&o["invoice"]));
            match t {
                CHECKOUT_SESSION_COMPLETED if o["payment_status"].as_str() == Some("paid") => {
                    EventKind::CheckoutCompleted { session_id }
                }
                CHECKOUT_SESSION_ASYNC_SUCCEEDED => EventKind::CheckoutCompleted { session_id },
                CHECKOUT_SESSION_EXPIRED | CHECKOUT_SESSION_ASYNC_FAILED => {
                    EventKind::CheckoutExpired { session_id }
                }
                _ => ignored(),
            }
        }
        INVOICE_PAID if o["billing_reason"].as_str() == Some(BILLING_REASON_CYCLE) => {
            // API 2024-06-20 puts the subscription at the top level; newer
            // versions under `parent.subscription_details` (decision 9).
            let parent = &o["parent"]["subscription_details"];
            let Some(subscription_id) =
                id_of(&o["subscription"]).or_else(|| id_of(&parent["subscription"]))
            else {
                tracing::error!(
                    event_id = %event.id,
                    invoice_id = ?id_of(&o["id"]),
                    "a paid renewal invoice names no subscription (neither `subscription` nor \
                     `parent.subscription_details.subscription`); the renewal is ignored and \
                     its year not granted"
                );
                return VerifiedEvent::new(
                    event.id.clone(),
                    "stripe",
                    event.event_type.clone(),
                    ignored(),
                    fields,
                );
            };
            let Some(invoice_id) = id_of(&o["id"]) else {
                return Err(ProviderError::Rejected(
                    "the renewal invoice event has no data.object.id",
                ));
            };
            fields.amount = amount_of(&o["subtotal"], &o["currency"]);
            fields.customer_id = id_of(&o["customer"]);
            fields.order_ref = order_ref_of(&o["subscription_details"]["metadata"], hub_origin)
                .or_else(|| order_ref_of(&parent["metadata"], hub_origin));
            EventKind::RenewalPaid {
                subscription_id,
                invoice_id,
            }
        }
        CUSTOMER_SUBSCRIPTION_UPDATED | CUSTOMER_SUBSCRIPTION_DELETED => {
            let Some(subscription_id) = id_of(&o["id"]) else {
                return Err(ProviderError::Rejected(
                    "the subscription event has no data.object.id",
                ));
            };
            fields.order_ref = order_ref_of(&o["metadata"], hub_origin);
            fields.occurred_at = event.created;
            if event.event_type == CUSTOMER_SUBSCRIPTION_UPDATED {
                let at_period_end = o["cancel_at_period_end"].as_bool() == Some(true);
                let cancel_at = !o["cancel_at"].is_null();
                EventKind::RenewalChanged {
                    subscription_id,
                    renews: !(at_period_end || cancel_at),
                }
            } else {
                EventKind::SubscriptionEnded { subscription_id }
            }
        }
        CHARGE_REFUNDED if o["refunded"].as_bool() == Some(true) => {
            let payment_refs: Vec<String> = [id_of(&o["payment_intent"]), id_of(&o["invoice"])]
                .into_iter()
                .flatten()
                .collect();
            if payment_refs.is_empty() {
                // Every hub checkout produces a payment intent or an invoice,
                // so this is another app's legacy charge on a shared account:
                // acknowledge it rather than have Stripe retry (decision 11).
                tracing::warn!(
                    event_id = %event.id,
                    charge_id = ?id_of(&o["id"]),
                    "a fully refunded charge names no payment intent or invoice, so it \
                     cannot be one of this hub's payments; the refund is ignored"
                );
                ignored()
            } else {
                EventKind::Refunded { payment_refs }
            }
        }
        _ => ignored(),
    };
    VerifiedEvent::new(event.id, "stripe", event.event_type, kind, fields)
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
            renewal_notice: None,
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

    // -----------------------------------------------------------------------
    // Story 4: subscriptions, renewals and refunds

    fn event_at(
        event_type: &str,
        created: i64,
        object: Value,
    ) -> Result<VerifiedEvent, ProviderError> {
        let body = json!({
            "id": "evt_test_2",
            "object": "event",
            "type": event_type,
            "created": created,
            "data": {"object": object},
        })
        .to_string()
        .into_bytes();
        stripe().verify_event(&signed(&body), &body)
    }

    const ORDER: &str = "7f0c1e2a-0000-4000-8000-000000000001";

    fn ours() -> Value {
        json!({"wipehub_order_id": ORDER, "wipehub_hub": HUB, "wipehub_organization_id": "x"})
    }

    fn renewal_invoice() -> Value {
        json!({
            "id": "in_test_1",
            "object": "invoice",
            "billing_reason": "subscription_cycle",
            "subscription": "sub_test_1",
            "subtotal": 120000,
            "total": 120000,
            "currency": "usd",
            "customer": "cus_test_1",
            "subscription_details": {"metadata": ours()},
            "metadata": {},
        })
    }

    fn subscription(cancel_at_period_end: bool, cancel_at: Value) -> Value {
        json!({
            "id": "sub_test_1",
            "object": "subscription",
            "status": "active",
            "cancel_at_period_end": cancel_at_period_end,
            "cancel_at": cancel_at,
            "metadata": ours(),
        })
    }

    fn charge(refunded: bool, payment_intent: Value, invoice: Value) -> Value {
        json!({
            "id": "ch_test_1",
            "object": "charge",
            "amount": 120000,
            "amount_refunded": if refunded { 120000 } else { 100 },
            "refunded": refunded,
            "payment_intent": payment_intent,
            "invoice": invoice,
        })
    }

    #[test]
    fn a_subscription_checkout_carries_its_subscription_and_first_invoice() {
        let mut s = paid_session();
        s["mode"] = json!("subscription");
        s["subscription"] = json!("sub_test_1");
        s["invoice"] = json!("in_test_0");
        s["payment_intent"] = Value::Null;
        let ev = kind_of(CHECKOUT_SESSION_COMPLETED, s.clone()).unwrap();
        assert_eq!(ev.subscription_id(), Some("sub_test_1"));
        assert_eq!(ev.payment_ref(), Some("in_test_0"));
        // A one-off payment's reference is its payment intent.
        s["subscription"] = Value::Null;
        s["payment_intent"] = json!({"id": "pi_test_1", "object": "payment_intent"});
        let ev = kind_of(CHECKOUT_SESSION_ASYNC_SUCCEEDED, s).unwrap();
        assert_eq!(ev.subscription_id(), None);
        assert_eq!(ev.payment_ref(), Some("pi_test_1"));
        let ev = kind_of(CHECKOUT_SESSION_COMPLETED, paid_session()).unwrap();
        assert_eq!(ev.subscription_id(), None);
        assert_eq!(ev.payment_ref(), None);
    }

    #[test]
    fn a_renewal_invoice_is_renewal_paid() {
        let ev = event_at(INVOICE_PAID, 1_800_000_000, renewal_invoice()).unwrap();
        assert_eq!(
            ev.kind(),
            &EventKind::RenewalPaid {
                subscription_id: "sub_test_1".into(),
                invoice_id: "in_test_1".into()
            }
        );
        assert_eq!(ev.amount(), Some((120000, "USD")));
        assert_eq!(ev.order_ref(), Some(Uuid::parse_str(ORDER).unwrap()));
        assert_eq!(ev.session_id(), None);
        // An expanded subscription.
        let mut inv = renewal_invoice();
        inv["subscription"] = json!({"id": "sub_test_9", "object": "subscription"});
        assert!(matches!(
            event_at(INVOICE_PAID, 1, inv).unwrap().kind(),
            EventKind::RenewalPaid { subscription_id, .. } if subscription_id == "sub_test_9"
        ));
        // Another hub's subscription names no order of ours.
        let mut inv = renewal_invoice();
        inv["subscription_details"]["metadata"]["wipehub_hub"] = json!("http://127.0.0.1:7901");
        assert_eq!(event_at(INVOICE_PAID, 1, inv).unwrap().order_ref(), None);
        // The session-level `metadata` is not read for an invoice.
        let mut inv = renewal_invoice();
        inv["metadata"] = ours();
        inv["subscription_details"] = json!({"metadata": {}});
        assert_eq!(event_at(INVOICE_PAID, 1, inv).unwrap().order_ref(), None);
    }

    /// The renewal invoice in API versions after 2024-06-20: no top-level
    /// `subscription` or `subscription_details`, both under `parent`.
    fn parent_renewal_invoice() -> Value {
        let mut inv = renewal_invoice();
        let map = inv.as_object_mut().unwrap();
        map.remove("subscription");
        map.remove("subscription_details");
        inv["parent"] = json!({
            "type": "subscription_details",
            "quote_details": null,
            "subscription_details": {"subscription": "sub_test_1", "metadata": ours()},
        });
        inv
    }

    #[test]
    fn a_renewal_invoice_is_read_in_the_newer_parent_shape() {
        let ev = event_at(INVOICE_PAID, 1, parent_renewal_invoice()).unwrap();
        assert_eq!(
            ev.kind(),
            &EventKind::RenewalPaid {
                subscription_id: "sub_test_1".into(),
                invoice_id: "in_test_1".into()
            }
        );
        assert_eq!(ev.amount(), Some((120000, "USD")));
        assert_eq!(ev.order_ref(), Some(Uuid::parse_str(ORDER).unwrap()));
        // The parent's metadata names another hub: no order of ours.
        let mut inv = parent_renewal_invoice();
        inv["parent"]["subscription_details"]["metadata"]["wipehub_hub"] =
            json!("http://127.0.0.1:7901");
        assert_eq!(event_at(INVOICE_PAID, 1, inv).unwrap().order_ref(), None);
        // The top-level shape wins when both are present.
        let mut inv = renewal_invoice();
        inv["parent"] = json!({"subscription_details": {"subscription": "sub_other"}});
        assert!(matches!(
            event_at(INVOICE_PAID, 1, inv).unwrap().kind(),
            EventKind::RenewalPaid { subscription_id, .. } if subscription_id == "sub_test_1"
        ));
        // Neither shape names a subscription: ignored (and logged at error).
        let mut inv = parent_renewal_invoice();
        inv["parent"] = Value::Null;
        assert_eq!(
            event_at(INVOICE_PAID, 1, inv).unwrap().kind(),
            &EventKind::Ignored {
                event_type: INVOICE_PAID.into()
            }
        );
    }

    #[test]
    fn other_paid_invoices_are_ignored() {
        let ignored = EventKind::Ignored {
            event_type: INVOICE_PAID.into(),
        };
        for (field, value) in [
            ("billing_reason", json!("subscription_create")),
            ("billing_reason", json!("manual")),
            ("billing_reason", Value::Null),
            ("subscription", Value::Null),
            ("subscription", json!("")),
        ] {
            let mut inv = renewal_invoice();
            inv[field] = value.clone();
            assert_eq!(
                event_at(INVOICE_PAID, 1, inv).unwrap().kind(),
                &ignored,
                "{field} {value}"
            );
        }
    }

    #[test]
    fn subscription_changes_map_to_renews() {
        for (at_period_end, cancel_at, renews) in [
            (false, Value::Null, true),
            (true, Value::Null, false),
            (false, json!(1_900_000_000), false),
            (true, json!(1_900_000_000), false),
        ] {
            let ev = event_at(
                CUSTOMER_SUBSCRIPTION_UPDATED,
                1_800_000_123,
                subscription(at_period_end, cancel_at.clone()),
            )
            .unwrap();
            assert_eq!(
                ev.kind(),
                &EventKind::RenewalChanged {
                    subscription_id: "sub_test_1".into(),
                    renews
                },
                "{at_period_end} {cancel_at}"
            );
            assert_eq!(ev.occurred_at(), Some(1_800_000_123));
            assert_eq!(ev.order_ref(), Some(Uuid::parse_str(ORDER).unwrap()));
        }
        let ev = event_at(
            CUSTOMER_SUBSCRIPTION_DELETED,
            1_800_000_200,
            subscription(false, Value::Null),
        )
        .unwrap();
        assert_eq!(
            ev.kind(),
            &EventKind::SubscriptionEnded {
                subscription_id: "sub_test_1".into()
            }
        );
        assert_eq!(ev.order_ref(), Some(Uuid::parse_str(ORDER).unwrap()));
        // The hub check applies to subscription metadata too.
        let mut sub = subscription(true, Value::Null);
        sub["metadata"] = json!({"wipehub_order_id": ORDER});
        assert_eq!(
            event_at(CUSTOMER_SUBSCRIPTION_UPDATED, 1, sub)
                .unwrap()
                .order_ref(),
            None
        );
    }

    #[test]
    fn a_full_refund_names_its_payments_and_a_partial_one_is_ignored() {
        let refs = |ev: VerifiedEvent| match ev.kind() {
            EventKind::Refunded { payment_refs } => payment_refs.clone(),
            other => panic!("{other:?}"),
        };
        let ev = event_at(
            CHARGE_REFUNDED,
            1,
            charge(true, json!("pi_1"), json!("in_1")),
        )
        .unwrap();
        assert_eq!(refs(ev), ["pi_1", "in_1"]);
        let ev = event_at(CHARGE_REFUNDED, 1, charge(true, json!("pi_1"), Value::Null)).unwrap();
        assert_eq!(refs(ev), ["pi_1"]);
        let ev = event_at(CHARGE_REFUNDED, 1, charge(true, Value::Null, json!("in_1"))).unwrap();
        assert_eq!(refs(ev), ["in_1"]);
        let ev = event_at(
            CHARGE_REFUNDED,
            1,
            charge(false, json!("pi_1"), Value::Null),
        )
        .unwrap();
        assert_eq!(
            ev.kind(),
            &EventKind::Ignored {
                event_type: CHARGE_REFUNDED.into()
            }
        );
    }

    #[test]
    fn a_full_refund_naming_no_payment_is_acknowledged_as_ignored() {
        // A legacy Charges API charge from another app on a shared account
        // (decision 11): refusing it would make Stripe retry for days.
        let ev = event_at(CHARGE_REFUNDED, 1, charge(true, Value::Null, Value::Null)).unwrap();
        assert_eq!(
            ev.kind(),
            &EventKind::Ignored {
                event_type: CHARGE_REFUNDED.into()
            }
        );
    }

    #[test]
    fn mapped_types_without_their_ids_are_refused() {
        let mut inv = renewal_invoice();
        inv.as_object_mut().unwrap().remove("id");
        assert!(rejected(event_at(INVOICE_PAID, 1, inv)));
        for t in [CUSTOMER_SUBSCRIPTION_UPDATED, CUSTOMER_SUBSCRIPTION_DELETED] {
            let mut sub = subscription(false, Value::Null);
            sub["id"] = json!("");
            assert!(rejected(event_at(t, 1, sub)), "{t}");
            assert!(rejected(event_at(t, 1, Value::Null)), "{t}");
        }
        let mut inv = renewal_invoice();
        inv["id"] = json!("i".repeat(MAX_EVENT_FIELD + 1));
        assert!(rejected(event_at(INVOICE_PAID, 1, inv)));
    }

    fn checkout_order(interval: &str) -> CheckoutOrder {
        CheckoutOrder {
            order_id: Uuid::parse_str(ORDER).unwrap(),
            organization_id: Uuid::nil(),
            offering_id: Uuid::nil(),
            offering_name: "Station".into(),
            price_minor: 120000,
            currency: "USD".into(),
            interval: interval.into(),
            customer_id: None,
            renewal_notice: (interval == "annual").then(|| {
                crate::entitlements::renewal_notice(
                    120000,
                    "USD",
                    time::macros::date!(2026 - 11 - 08),
                )
            }),
        }
    }

    #[test]
    fn an_annual_checkout_is_a_yearly_subscription() {
        let p = stripe();
        let field = |form: &[(&str, String)], name: &str| {
            form.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone())
        };
        let form = p.checkout_form(&checkout_order("annual"), None);
        assert_eq!(field(&form, "mode").as_deref(), Some("subscription"));
        assert_eq!(
            field(&form, "line_items[0][price_data][recurring][interval]").as_deref(),
            Some("year")
        );
        for key in ["wipehub_order_id", "wipehub_organization_id", "wipehub_hub"] {
            assert_eq!(
                field(&form, &format!("subscription_data[metadata][{key}]")),
                field(&form, &format!("metadata[{key}]")),
                "{key}"
            );
        }
        assert_eq!(
            field(&form, "custom_text[submit][message]").as_deref(),
            Some(
                "Renews automatically every year at 1,200.00 USD until you cancel. Your first \
                 year runs to 8 November 2027. Cancel any time from Billing in your account: \
                 your current year still runs to its end, and you will not be charged again. \
                 We email you before each renewal."
            )
        );
        assert_eq!(field(&form, "customer_creation"), None);
        assert_eq!(field(&form, "customer"), None);
        let with = p.checkout_form(&checkout_order("annual"), Some("cus_1"));
        assert_eq!(field(&with, "customer").as_deref(), Some("cus_1"));
        assert_eq!(field(&with, "customer_creation"), None);

        let one_off = p.checkout_form(&checkout_order("one_off"), None);
        assert_eq!(field(&one_off, "mode").as_deref(), Some("payment"));
        assert_eq!(
            field(&one_off, "customer_creation").as_deref(),
            Some("always")
        );
        assert!(
            one_off
                .iter()
                .all(|(k, _)| !k.starts_with("subscription_data")
                    && !k.starts_with("custom_text")
                    && !k.contains("recurring")),
            "{one_off:?}"
        );
    }
}
