//! Payment providers (ADR-0007 §4; offerings-and-payments stories 2 and 3).
//!
//! [`PaymentProvider`] is the hub's one seam to a payment service: it starts
//! a hosted checkout for an order, opens the provider's billing page for an
//! organization, and verifies a signed provider event. Two implementations
//! exist:
//!
//! - [`Fake`]: deterministic and in-process, for every automated test and
//!   local development. Its "hosted page" is the portal's `/checkout/fake`
//!   and its events are JSON signed with HMAC-SHA256 under the public
//!   [`FAKE_WEBHOOK_SECRET`]. Because that secret is public, it refuses to
//!   start a checkout, open billing or verify an event on a hub whose
//!   `--public-base-url` is not a local host ([`MSG_FAKE_LOCAL_ONLY`]).
//! - [`crate::stripe::Stripe`]: Stripe Checkout, the Stripe billing portal
//!   and `Stripe-Signature` verification.
//!
//! Subscriptions (story 4): an annual offering checks out as a subscription.
//! Its renewal payments, renewal changes, end and refunds arrive as events
//! too ([`EventKind::RenewalPaid`] and the others), and change entitlements
//! through the same path. The Fake provider simulates them with
//! [`FAKE_SUBSCRIPTION_RENEWED`] and [`FAKE_SUBSCRIPTION_CANCELLED`] (the
//! portal's Fake billing buttons), and [`FAKE_SUBSCRIPTION_RENEWAL_FAILED`]
//! and [`FAKE_SUBSCRIPTION_ENDED`] (posted to the webhook by tests and the
//! local demo, controlled-licensing story 5).
//!
//! An order changes status only by [`apply_event`] applying a
//! [`VerifiedEvent`], called only from [`crate::payment_events::receive`],
//! and only [`PaymentProvider::verify_event`] can make a `VerifiedEvent`: its
//! fields are private and it has no public constructor.

use std::sync::Arc;

use axum::async_trait;
use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use url::Url;
use uuid::Uuid;

use crate::config::{self, Config};

pub use crate::orders::apply_event;

/// The Fake provider's answer on a hub whose public base URL is not local.
pub const MSG_FAKE_LOCAL_ONLY: &str = "the fake payment provider works only on a local hub";

/// The Fake provider's event signing secret. Public on purpose: anyone can
/// sign a Fake event, which is why the Fake provider works only on a local
/// hub.
pub const FAKE_WEBHOOK_SECRET: &str = "wipehub-fake-webhook-secret";
/// The header carrying a Fake event's hex HMAC-SHA256 signature.
pub const FAKE_SIGNATURE_HEADER: &str = "fake-signature";
/// Fake event types.
pub const FAKE_CHECKOUT_COMPLETED: &str = "checkout.completed";
pub const FAKE_CHECKOUT_EXPIRED: &str = "checkout.expired";
/// A renewal payment of a Fake subscription.
pub const FAKE_SUBSCRIPTION_RENEWED: &str = "subscription.renewed";
/// A Fake subscription cancelled at period end.
pub const FAKE_SUBSCRIPTION_CANCELLED: &str = "subscription.cancelled";
/// A failed renewal charge of a Fake subscription (subscription and
/// invoice).
pub const FAKE_SUBSCRIPTION_RENEWAL_FAILED: &str = "subscription.renewal_failed";
/// A Fake subscription the provider ended (subscription).
pub const FAKE_SUBSCRIPTION_ENDED: &str = "subscription.ended";
/// Prefix of Fake subscription ids.
pub const FAKE_SUBSCRIPTION_PREFIX: &str = "fake_sub_";
/// Prefix of Fake invoice ids.
pub const FAKE_INVOICE_PREFIX: &str = "fake_in_";
/// Prefix of Fake checkout session ids.
pub const FAKE_SESSION_PREFIX: &str = "fake_cs_";
/// Prefix of Fake event ids.
pub const FAKE_EVENT_PREFIX: &str = "fake_evt_";

/// What a provider needs to start a checkout for one order. The order is
/// not stored yet: the provider is called first, so a provider error writes
/// nothing.
#[derive(Debug, Clone)]
pub struct CheckoutOrder {
    /// The id the order will be stored under.
    pub order_id: Uuid,
    pub organization_id: Uuid,
    pub offering_id: Uuid,
    pub offering_name: String,
    /// Integer minor units.
    pub price_minor: i64,
    pub currency: String,
    /// `one_off` or `annual`.
    pub interval: String,
    /// The organization's stored customer at this provider, if any.
    pub customer_id: Option<String>,
    /// The owner-approved auto-renewal notice, for an annual offering (which
    /// checks out as a subscription); `None` for a one-off.
    pub renewal_notice: Option<crate::entitlements::RenewalNotice>,
}

impl CheckoutOrder {
    /// Whether the order renews (an annual offering, bought as a
    /// subscription).
    pub fn is_annual(&self) -> bool {
        self.interval == "annual"
    }
}

/// A started hosted checkout: the provider's session id and where to send
/// the buyer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutSession {
    pub session_id: String,
    pub url: Url,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The provider cannot be used on this hub; the message is the 503 body.
    #[error("{0}")]
    Unavailable(&'static str),
    /// The provider failed (logged; a 502 to the client).
    #[error("payment provider failed: {0}")]
    Failed(String),
    /// An event's signature or payload is not acceptable.
    #[error("payment event rejected: {0}")]
    Rejected(&'static str),
    /// The organization has no customer at the provider yet (a 409).
    #[error("the organization has no billing account at the provider yet")]
    NoBillingAccount,
}

/// What a verified event says happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// The buyer paid for the checkout session.
    CheckoutCompleted { session_id: String },
    /// The checkout session ended without payment.
    CheckoutExpired { session_id: String },
    /// A subscription's renewal invoice was paid.
    RenewalPaid {
        subscription_id: String,
        invoice_id: String,
    },
    /// A subscription's renewal charge failed (the provider retries it).
    RenewalFailed {
        subscription_id: String,
        invoice_id: String,
    },
    /// Whether a subscription will renew changed (an owner cancelled at
    /// period end, or resumed).
    RenewalChanged {
        subscription_id: String,
        renews: bool,
    },
    /// The provider ended a subscription (after a cancellation at period
    /// end, or after failed renewal charges).
    SubscriptionEnded { subscription_id: String },
    /// A payment was refunded in full; its provider references (non-empty).
    Refunded { payment_refs: Vec<String> },
    /// An event type the hub does not act on.
    Ignored { event_type: String },
}

/// The optional fields of a [`VerifiedEvent`], for provider implementations.
#[derive(Debug, Clone, Default)]
pub(crate) struct EventFields {
    pub customer_id: Option<String>,
    pub amount: Option<(i64, String)>,
    pub order_ref: Option<Uuid>,
    pub subscription_id: Option<String>,
    pub payment_ref: Option<String>,
    pub occurred_at: Option<i64>,
}

/// A provider event whose signature has been verified. Only
/// [`PaymentProvider::verify_event`] implementations in this module can
/// construct one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedEvent {
    id: String,
    provider: &'static str,
    event_type: String,
    kind: EventKind,
    customer_id: Option<String>,
    amount: Option<(i64, String)>,
    order_ref: Option<Uuid>,
    subscription_id: Option<String>,
    payment_ref: Option<String>,
    occurred_at: Option<i64>,
}

/// The longest event id, event type or session id the hub stores.
pub const MAX_EVENT_FIELD: usize = 255;

impl VerifiedEvent {
    /// For provider implementations in this crate only: checks the fields
    /// every stored event needs (a non-blank id and type, and the non-blank
    /// session, subscription, invoice and payment ids its kind names, each at
    /// most [`MAX_EVENT_FIELD`] characters). An optional customer,
    /// subscription or payment reference that does not fit is dropped.
    pub(crate) fn new(
        id: String,
        provider: &'static str,
        event_type: String,
        kind: EventKind,
        fields: EventFields,
    ) -> Result<Self, ProviderError> {
        let fits = |s: &str| !s.trim().is_empty() && s.chars().count() <= MAX_EVENT_FIELD;
        if !fits(&id) {
            return Err(ProviderError::Rejected(
                "the event id is missing or too long",
            ));
        }
        if !fits(&event_type) {
            return Err(ProviderError::Rejected(
                "the event type is missing or too long",
            ));
        }
        match &kind {
            EventKind::CheckoutCompleted { session_id }
            | EventKind::CheckoutExpired { session_id } => {
                if !fits(session_id) {
                    return Err(ProviderError::Rejected(
                        "the checkout session id is missing or too long",
                    ));
                }
            }
            EventKind::RenewalPaid {
                subscription_id,
                invoice_id,
            }
            | EventKind::RenewalFailed {
                subscription_id,
                invoice_id,
            } => {
                if !fits(subscription_id) || !fits(invoice_id) {
                    return Err(ProviderError::Rejected(
                        "the subscription or invoice id is missing or too long",
                    ));
                }
            }
            EventKind::RenewalChanged {
                subscription_id, ..
            }
            | EventKind::SubscriptionEnded { subscription_id } => {
                if !fits(subscription_id) {
                    return Err(ProviderError::Rejected(
                        "the subscription id is missing or too long",
                    ));
                }
            }
            EventKind::Refunded { payment_refs } => {
                if payment_refs.is_empty() || !payment_refs.iter().all(|r| fits(r)) {
                    return Err(ProviderError::Rejected(
                        "the refunded payment's ids are missing or too long",
                    ));
                }
            }
            EventKind::Ignored { .. } => {}
        }
        let EventFields {
            customer_id,
            amount,
            order_ref,
            subscription_id,
            payment_ref,
            occurred_at,
        } = fields;
        Ok(Self {
            id,
            provider,
            event_type,
            kind,
            customer_id: customer_id.filter(|c| fits(c)),
            amount,
            order_ref,
            subscription_id: subscription_id.filter(|c| fits(c)),
            payment_ref: payment_ref.filter(|c| fits(c)),
            occurred_at,
        })
    }

    /// The provider's event id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The verifying provider's name (`fake` or `stripe`).
    pub fn provider(&self) -> &'static str {
        self.provider
    }

    pub fn kind(&self) -> &EventKind {
        &self.kind
    }

    /// The checkout session the event is about, if any.
    pub fn session_id(&self) -> Option<&str> {
        match &self.kind {
            EventKind::CheckoutCompleted { session_id }
            | EventKind::CheckoutExpired { session_id } => Some(session_id),
            _ => None,
        }
    }

    /// The provider's own name for the event's type.
    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    /// The buyer's customer id at the provider (Stripe's `cus_…`), if sent.
    pub fn customer_id(&self) -> Option<&str> {
        self.customer_id.as_deref()
    }

    /// The amount paid before discounts and tax, in minor units, with the
    /// upper-case currency, if sent.
    pub fn amount(&self) -> Option<(i64, &str)> {
        self.amount.as_ref().map(|(m, c)| (*m, c.as_str()))
    }

    /// The hub order the provider says this is for: Stripe's
    /// `metadata[wipehub_order_id]`, and only when the session's
    /// `metadata[wipehub_hub]` is this hub's origin (decisions 9 and 11).
    pub fn order_ref(&self) -> Option<Uuid> {
        self.order_ref
    }

    /// The subscription a completed checkout started (Stripe's `sub_…`, the
    /// Fake `fake_sub_…`), if any: the order renews.
    pub fn subscription_id(&self) -> Option<&str> {
        self.subscription_id.as_deref()
    }

    /// A completed checkout's payment reference (Stripe's payment intent, or
    /// the subscription's first invoice), which a refund names.
    pub fn payment_ref(&self) -> Option<&str> {
        self.payment_ref.as_deref()
    }

    /// When the provider says the event happened, in unix seconds, if sent
    /// (orders renewal changes that arrive out of order).
    pub fn occurred_at(&self) -> Option<i64> {
        self.occurred_at
    }
}

/// The payment provider seam (ADR-0007 §4).
#[async_trait]
pub trait PaymentProvider: Send + Sync {
    /// `fake` or `stripe`: the `orders.provider` value.
    fn name(&self) -> &'static str;

    /// Start a hosted checkout for `order`.
    async fn create_checkout(
        &self,
        order: &CheckoutOrder,
    ) -> Result<CheckoutSession, ProviderError>;

    /// The provider's billing page (cards and payment history) for the organization,
    /// whose stored customer at this provider is `customer_id`.
    async fn billing_portal(
        &self,
        organization_id: Uuid,
        customer_id: Option<&str>,
    ) -> Result<Url, ProviderError>;

    /// Verify a signed event's headers and raw body. Anything not signed by
    /// the provider is an error.
    fn verify_event(
        &self,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<VerifiedEvent, ProviderError>;
}

impl config::PaymentProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            config::PaymentProvider::Fake => "fake",
            config::PaymentProvider::Stripe => "stripe",
        }
    }
}

/// The provider `config` selects.
pub fn provider_for(config: &Config) -> anyhow::Result<Arc<dyn PaymentProvider>> {
    Ok(match config.payment_provider {
        config::PaymentProvider::Fake => Arc::new(Fake::new(config.public_base_url.clone())),
        config::PaymentProvider::Stripe => {
            let (Some(secret_key), Some(webhook_secret)) = (
                config.stripe_secret_key.clone(),
                config.stripe_webhook_secret.clone(),
            ) else {
                anyhow::bail!("--payment-provider stripe needs both Stripe secrets");
            };
            Arc::new(crate::stripe::Stripe::new(
                secret_key,
                webhook_secret,
                config.stripe_api_base.clone(),
                config.public_base_url.clone(),
            )?)
        }
    })
}

/// `{base}/{page}`, keeping any base path, with no query or fragment.
pub(crate) fn page_url(base: &Url, page: &str) -> Url {
    let mut url = base.clone();
    let path = format!("{}/{}", base.path().trim_end_matches('/'), page);
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    url
}

// ---------------------------------------------------------------------------
// Fake

/// The deterministic, in-process provider.
#[derive(Debug, Clone)]
pub struct Fake {
    base: Url,
}

#[derive(Debug, Serialize, Deserialize)]
struct FakeEvent {
    id: String,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subscription_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invoice_id: Option<String>,
}

pub(crate) type HmacSha256 = Hmac<Sha256>;

fn fake_mac() -> HmacSha256 {
    HmacSha256::new_from_slice(FAKE_WEBHOOK_SECRET.as_bytes()).expect("HMAC takes any key length")
}

impl Fake {
    /// A Fake provider for a hub reached at `public_base_url`.
    pub fn new(public_base_url: Url) -> Self {
        Self {
            base: public_base_url,
        }
    }

    fn require_local(&self) -> Result<(), ProviderError> {
        if crate::auth::is_local_host(&self.base) {
            Ok(())
        } else {
            Err(ProviderError::Unavailable(MSG_FAKE_LOCAL_ONLY))
        }
    }

    /// The hex HMAC-SHA256 of `body` under [`FAKE_WEBHOOK_SECRET`].
    pub fn sign(body: &[u8]) -> String {
        let mut mac = fake_mac();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    /// A new Fake event of `event_type` for `session_id`: its JSON body and
    /// its signature. The event id is `fake_evt_<uuid>`.
    pub fn signed_event(event_type: &str, session_id: &str) -> (Vec<u8>, String) {
        Self::signed_event_with(event_type, Some(session_id), None, None)
    }

    /// A new Fake event of `event_type` with any of a checkout session, a
    /// subscription and an invoice: its JSON body and its signature.
    pub fn signed_event_with(
        event_type: &str,
        session_id: Option<&str>,
        subscription_id: Option<&str>,
        invoice_id: Option<&str>,
    ) -> (Vec<u8>, String) {
        let event = FakeEvent {
            id: format!("{FAKE_EVENT_PREFIX}{}", Uuid::new_v4().simple()),
            event_type: event_type.to_string(),
            session_id: session_id.map(str::to_string),
            subscription_id: subscription_id.map(str::to_string),
            invoice_id: invoice_id.map(str::to_string),
        };
        let body = serde_json::to_vec(&event).expect("a Fake event serializes");
        let signature = Self::sign(&body);
        (body, signature)
    }

    /// Headers carrying `signature` as a Fake event's.
    pub fn signature_headers(signature: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Ok(v) = signature.parse() {
            headers.insert(FAKE_SIGNATURE_HEADER, v);
        }
        headers
    }
}

#[async_trait]
impl PaymentProvider for Fake {
    fn name(&self) -> &'static str {
        "fake"
    }

    async fn create_checkout(
        &self,
        _order: &CheckoutOrder,
    ) -> Result<CheckoutSession, ProviderError> {
        self.require_local()?;
        let session_id = format!("{FAKE_SESSION_PREFIX}{}", Uuid::new_v4().simple());
        let mut url = page_url(&self.base, "checkout/fake");
        url.query_pairs_mut().append_pair("session", &session_id);
        Ok(CheckoutSession { session_id, url })
    }

    async fn billing_portal(
        &self,
        _organization_id: Uuid,
        _customer_id: Option<&str>,
    ) -> Result<Url, ProviderError> {
        self.require_local()?;
        Ok(page_url(&self.base, "billing/fake"))
    }

    fn verify_event(
        &self,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<VerifiedEvent, ProviderError> {
        self.require_local()?;
        let mut values = headers.get_all(FAKE_SIGNATURE_HEADER).iter();
        let (Some(value), None) = (values.next(), values.next()) else {
            return Err(ProviderError::Rejected(
                "one fake-signature header is required",
            ));
        };
        let signature = value
            .to_str()
            .ok()
            .and_then(|s| hex::decode(s.trim()).ok())
            .ok_or(ProviderError::Rejected("the signature is not hex"))?;
        let mut mac = fake_mac();
        mac.update(body);
        // Constant-time comparison.
        mac.verify_slice(&signature)
            .map_err(|_| ProviderError::Rejected("the signature does not match"))?;
        let event: FakeEvent = serde_json::from_slice(body)
            .map_err(|_| ProviderError::Rejected("the event is not valid JSON"))?;
        let required = |s: Option<String>, missing: &'static str| {
            s.filter(|s| !s.trim().is_empty())
                .ok_or(ProviderError::Rejected(missing))
        };
        const NO_SESSION: &str = "the event has no session_id";
        const NO_SUBSCRIPTION: &str = "the event has no subscription_id";
        let mut fields = EventFields::default();
        let kind = match event.event_type.as_str() {
            FAKE_CHECKOUT_COMPLETED => {
                fields.subscription_id = event.subscription_id.clone();
                EventKind::CheckoutCompleted {
                    session_id: required(event.session_id, NO_SESSION)?,
                }
            }
            FAKE_CHECKOUT_EXPIRED => EventKind::CheckoutExpired {
                session_id: required(event.session_id, NO_SESSION)?,
            },
            FAKE_SUBSCRIPTION_RENEWED => EventKind::RenewalPaid {
                subscription_id: required(event.subscription_id, NO_SUBSCRIPTION)?,
                invoice_id: required(event.invoice_id, "the event has no invoice_id")?,
            },
            FAKE_SUBSCRIPTION_CANCELLED => {
                fields.occurred_at = Some(crate::stripe::unix_now());
                EventKind::RenewalChanged {
                    subscription_id: required(event.subscription_id, NO_SUBSCRIPTION)?,
                    renews: false,
                }
            }
            FAKE_SUBSCRIPTION_RENEWAL_FAILED => {
                fields.occurred_at = Some(crate::stripe::unix_now());
                EventKind::RenewalFailed {
                    subscription_id: required(event.subscription_id, NO_SUBSCRIPTION)?,
                    invoice_id: required(event.invoice_id, "the event has no invoice_id")?,
                }
            }
            FAKE_SUBSCRIPTION_ENDED => {
                fields.occurred_at = Some(crate::stripe::unix_now());
                EventKind::SubscriptionEnded {
                    subscription_id: required(event.subscription_id, NO_SUBSCRIPTION)?,
                }
            }
            _ => EventKind::Ignored {
                event_type: event.event_type.clone(),
            },
        };
        VerifiedEvent::new(event.id, self.name(), event.event_type, kind, fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn local() -> Fake {
        Fake::new(Url::parse("http://127.0.0.1:7900").unwrap())
    }

    fn order() -> CheckoutOrder {
        CheckoutOrder {
            order_id: Uuid::new_v4(),
            organization_id: Uuid::new_v4(),
            offering_id: Uuid::new_v4(),
            offering_name: "Station".into(),
            price_minor: 120000,
            currency: "USD".into(),
            interval: "annual".into(),
            customer_id: None,
            renewal_notice: None,
        }
    }

    fn verify(body: &[u8], signature: Option<&str>) -> Result<VerifiedEvent, ProviderError> {
        let headers = signature.map(Fake::signature_headers).unwrap_or_default();
        local().verify_event(&headers, body)
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let (body, sig) = Fake::signed_event(FAKE_CHECKOUT_COMPLETED, "fake_cs_1");
        let ev = verify(&body, Some(&sig)).unwrap();
        assert!(ev.id().starts_with(FAKE_EVENT_PREFIX), "{}", ev.id());
        assert_eq!(ev.provider(), "fake");
        assert_eq!(
            ev.kind(),
            &EventKind::CheckoutCompleted {
                session_id: "fake_cs_1".into()
            }
        );
        let (body, sig) = Fake::signed_event(FAKE_CHECKOUT_EXPIRED, "fake_cs_2");
        assert_eq!(
            verify(&body, Some(&sig)).unwrap().kind(),
            &EventKind::CheckoutExpired {
                session_id: "fake_cs_2".into()
            }
        );
        // Upper-case hex is the same signature.
        assert!(verify(&body, Some(&sig.to_uppercase())).is_ok());
    }

    #[test]
    fn a_tampered_body_is_refused() {
        let (body, sig) = Fake::signed_event(FAKE_CHECKOUT_COMPLETED, "fake_cs_1");
        let tampered = String::from_utf8(body)
            .unwrap()
            .replace("fake_cs_1", "fake_cs_2");
        assert!(matches!(
            verify(tampered.as_bytes(), Some(&sig)),
            Err(ProviderError::Rejected(_))
        ));
    }

    #[test]
    fn a_wrong_or_missing_signature_is_refused() {
        let (body, sig) = Fake::signed_event(FAKE_CHECKOUT_COMPLETED, "fake_cs_1");
        let other = Fake::sign(b"something else");
        for bad in [other.as_str(), "", "zz", &sig[..sig.len() - 2]] {
            assert!(
                matches!(verify(&body, Some(bad)), Err(ProviderError::Rejected(_))),
                "{bad:?}"
            );
        }
        assert!(matches!(
            verify(&body, None),
            Err(ProviderError::Rejected(_))
        ));
        // Two signature headers are refused, even if one is right.
        let mut headers = Fake::signature_headers(&sig);
        headers.append(FAKE_SIGNATURE_HEADER, other.parse().unwrap());
        assert!(local().verify_event(&headers, &body).is_err());
        // A signed body that is not an event.
        let junk = b"not json";
        assert!(verify(junk, Some(&Fake::sign(junk))).is_err());
        // A completed event without a session.
        let no_session = json!({"id": "fake_evt_1", "type": FAKE_CHECKOUT_COMPLETED}).to_string();
        assert!(verify(
            no_session.as_bytes(),
            Some(&Fake::sign(no_session.as_bytes()))
        )
        .is_err());
    }

    #[test]
    fn an_unknown_type_is_ignored() {
        let body = json!({"id": "fake_evt_9", "type": "invoice.paid"}).to_string();
        let ev = verify(body.as_bytes(), Some(&Fake::sign(body.as_bytes()))).unwrap();
        assert_eq!(ev.id(), "fake_evt_9");
        assert_eq!(
            ev.kind(),
            &EventKind::Ignored {
                event_type: "invoice.paid".into()
            }
        );
    }

    #[tokio::test]
    async fn fake_urls_keep_the_base_path() {
        let fake = Fake::new(Url::parse("http://localhost:5174/portal/").unwrap());
        let s = fake.create_checkout(&order()).await.unwrap();
        assert!(s.session_id.starts_with(FAKE_SESSION_PREFIX));
        assert_eq!(s.session_id.len(), FAKE_SESSION_PREFIX.len() + 32);
        assert_eq!(
            s.url.as_str(),
            format!(
                "http://localhost:5174/portal/checkout/fake?session={}",
                s.session_id
            )
        );
        assert_eq!(
            fake.billing_portal(Uuid::nil(), Some("cus_ignored"))
                .await
                .unwrap()
                .as_str(),
            "http://localhost:5174/portal/billing/fake"
        );
        let other = fake.create_checkout(&order()).await.unwrap();
        assert_ne!(other.session_id, s.session_id);
    }

    #[tokio::test]
    async fn fake_refuses_a_non_local_hub() {
        let fake = Fake::new(Url::parse("https://hub.example.com").unwrap());
        let unavailable = |e: ProviderError| matches!(e, ProviderError::Unavailable(m) if m == MSG_FAKE_LOCAL_ONLY);
        assert!(unavailable(
            fake.create_checkout(&order()).await.unwrap_err()
        ));
        assert!(unavailable(
            fake.billing_portal(Uuid::nil(), None).await.unwrap_err()
        ));
        let (body, sig) = Fake::signed_event(FAKE_CHECKOUT_COMPLETED, "fake_cs_1");
        assert!(unavailable(
            fake.verify_event(&Fake::signature_headers(&sig), &body)
                .unwrap_err()
        ));
    }

    #[test]
    fn fake_events_carry_no_stripe_fields() {
        let (body, sig) = Fake::signed_event(FAKE_CHECKOUT_COMPLETED, "fake_cs_1");
        let ev = verify(&body, Some(&sig)).unwrap();
        assert_eq!(ev.customer_id(), None);
        assert_eq!(ev.amount(), None);
        assert_eq!(ev.order_ref(), None);
        assert_eq!(ev.session_id(), Some("fake_cs_1"));
        assert_eq!(ev.event_type(), FAKE_CHECKOUT_COMPLETED);
    }

    #[test]
    fn oversized_or_blank_event_fields_are_refused() {
        let long = "x".repeat(MAX_EVENT_FIELD + 1);
        for body in [
            json!({"id": long, "type": FAKE_CHECKOUT_COMPLETED, "session_id": "fake_cs_1"}),
            json!({"id": " ", "type": FAKE_CHECKOUT_COMPLETED, "session_id": "fake_cs_1"}),
            json!({"id": "fake_evt_1", "type": FAKE_CHECKOUT_COMPLETED, "session_id": long}),
            json!({"id": "fake_evt_1", "type": long}),
            json!({"id": "fake_evt_1", "type": ""}),
        ] {
            let body = body.to_string();
            assert!(
                matches!(
                    verify(body.as_bytes(), Some(&Fake::sign(body.as_bytes()))),
                    Err(ProviderError::Rejected(_))
                ),
                "{body}"
            );
        }
    }

    #[test]
    fn fake_subscription_events_map_to_kinds() {
        let (body, sig) = Fake::signed_event_with(
            FAKE_CHECKOUT_COMPLETED,
            Some("fake_cs_1"),
            Some("fake_sub_1"),
            None,
        );
        let ev = verify(&body, Some(&sig)).unwrap();
        assert_eq!(ev.subscription_id(), Some("fake_sub_1"));
        assert_eq!(ev.payment_ref(), None);
        let (body, sig) = Fake::signed_event_with(
            FAKE_SUBSCRIPTION_RENEWED,
            None,
            Some("fake_sub_1"),
            Some("fake_in_1"),
        );
        let ev = verify(&body, Some(&sig)).unwrap();
        assert_eq!(
            ev.kind(),
            &EventKind::RenewalPaid {
                subscription_id: "fake_sub_1".into(),
                invoice_id: "fake_in_1".into()
            }
        );
        assert_eq!(ev.session_id(), None);
        let (body, sig) =
            Fake::signed_event_with(FAKE_SUBSCRIPTION_CANCELLED, None, Some("fake_sub_1"), None);
        let ev = verify(&body, Some(&sig)).unwrap();
        assert_eq!(
            ev.kind(),
            &EventKind::RenewalChanged {
                subscription_id: "fake_sub_1".into(),
                renews: false
            }
        );
        assert!(ev.occurred_at().is_some());
        let (body, sig) = Fake::signed_event_with(
            FAKE_SUBSCRIPTION_RENEWAL_FAILED,
            None,
            Some("fake_sub_1"),
            Some("fake_in_2"),
        );
        let ev = verify(&body, Some(&sig)).unwrap();
        assert_eq!(
            ev.kind(),
            &EventKind::RenewalFailed {
                subscription_id: "fake_sub_1".into(),
                invoice_id: "fake_in_2".into()
            }
        );
        assert!(ev.occurred_at().is_some());
        let (body, sig) =
            Fake::signed_event_with(FAKE_SUBSCRIPTION_ENDED, None, Some("fake_sub_1"), None);
        let ev = verify(&body, Some(&sig)).unwrap();
        assert_eq!(
            ev.kind(),
            &EventKind::SubscriptionEnded {
                subscription_id: "fake_sub_1".into()
            }
        );
        assert!(ev.occurred_at().is_some());
        // Missing ids are refused.
        for (event_type, sub, invoice) in [
            (FAKE_SUBSCRIPTION_RENEWED, None, Some("fake_in_1")),
            (FAKE_SUBSCRIPTION_RENEWED, Some("fake_sub_1"), None),
            (FAKE_SUBSCRIPTION_RENEWED, Some(" "), Some("fake_in_1")),
            (FAKE_SUBSCRIPTION_CANCELLED, None, None),
            (FAKE_SUBSCRIPTION_RENEWAL_FAILED, Some("fake_sub_1"), None),
            (FAKE_SUBSCRIPTION_RENEWAL_FAILED, None, Some("fake_in_1")),
            (FAKE_SUBSCRIPTION_ENDED, None, None),
        ] {
            let (body, sig) = Fake::signed_event_with(event_type, None, sub, invoice);
            assert!(
                matches!(verify(&body, Some(&sig)), Err(ProviderError::Rejected(_))),
                "{event_type} {sub:?} {invoice:?}"
            );
        }
    }
}
