//! Orders and the customer catalog (offerings-and-payments stories 2, 3 and
//! 4, CAP-2 to CAP-4): the data layer behind [`crate::order_routes`].
//!
//! - **Catalog**: public, no session (story 5), published offerings only,
//!   by lower-cased name, then id. An annual offering carries the
//!   owner-approved auto-renewal notice
//!   ([`crate::entitlements::renewal_notice`]); a one-off carries none.
//! - **An order snapshots** the offering's name, price, currency, interval
//!   and grants, so retiring the offering never changes what was bought.
//! - **Lifecycle** `pending → paid` or `pending → expired`, and `paid →
//!   refunded` (a full refund, story 4); `expired` and `refunded` are
//!   terminal. [`apply_event`], fed a [`VerifiedEvent`] that only a
//!   provider's `verify_event` can make, is the only code that changes an
//!   order's status, and only [`crate::payment_events::receive`] calls it.
//! - **Fulfilment**: the paid transition creates the order's one
//!   entitlement in the same transaction ([`crate::entitlements::grant`]).
//!   Subscription events (renewal, failed renewal, renewal change, end) go
//!   to [`crate::entitlements`]; a refund marks the entitlement and the order
//!   `refunded`. None of them touches an issued licence here; a stopped or
//!   ended subscription's renewal grace is withdrawn by
//!   [`crate::payment_events::receive`] after this applies it.
//! - **A paid event is checked against the order's snapshot**: a different
//!   amount, currency or order reference leaves the order pending
//!   ([`ApplyOutcome::Mismatch`]) for a human to look at.
//! - **Billing customers**: a paid event that names the buyer's customer at
//!   the provider stores it in `billing_customers`, replacing a different
//!   stored one (decision 10: the vendor may have deleted the old customer at
//!   Stripe), and checkout and billing reuse it ([`billing_customer`]).
//! - **Free trials** ([`take_trial`], ADR-0013 §2): a verified owner takes a
//!   published trial offering without any payment provider. The order is
//!   written already `paid` (provider `trial`, price 0, session
//!   `trial:<order id>`) with its entitlement, once per organization per
//!   trial offering (migration 0020's partial unique index). A checkout of a
//!   trial is refused before the provider is called.
//! - **Every state change writes one audit row** in its transaction:
//!   `order.created` (actor `user:<id>`), then `order.paid` or
//!   `order.expired`, later `order.refunded` (actor `provider:<name>`,
//!   detail `{event_id}`); the subject is `order:<id>`. A free trial,
//!   written already paid, writes `order.trial_taken {offering_id}` (actor
//!   `user:<id>`) instead of `order.created` and `order.paid`.
//! - **Every customer query takes the organization id first**; another
//!   organization's order or session is the same `None` as an unknown one.
//! - Money is integer minor units; timestamps are RFC 3339 UTC strings
//!   formatted in SQL.
//!
//! Migration 0011 repeats the lifecycle and immutability rules as CHECKs
//! and a trigger.

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::admin::RFC3339_UTC;
use crate::audit::{self, AuditEntry, AuditError};
use crate::auth::actor;
use crate::entitlements::{self, RenewalNotice};
use crate::offering_admin::Grants;
use crate::payments::{CheckoutOrder, EventKind, PaymentProvider, ProviderError, VerifiedEvent};

/// Audit subject of one order.
pub fn subject(id: Uuid) -> String {
    format!("order:{id}")
}

// ---------------------------------------------------------------------------
// Shapes

/// A published offering as anyone sees it (the catalog is public).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogOffering {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub price_minor: i64,
    pub currency: String,
    /// `one_off` or `annual`.
    pub interval: String,
    /// A free trial (price 0, one-off): taken with `POST /api/trials`, never
    /// bought.
    pub trial: bool,
    pub grants: Grants,
    /// The auto-renewal notice for an annual offering, dated today; `null`
    /// for a one-off.
    pub renewal_notice: Option<RenewalNotice>,
}

/// An order as its organization sees it. The provider's session id is not
/// part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    pub id: Uuid,
    pub offering_id: Uuid,
    /// The offering's name when the order was created.
    pub offering_name: String,
    pub price_minor: i64,
    pub currency: String,
    pub interval: String,
    pub grants: Grants,
    /// `pending`, `paid`, `expired` or `refunded`.
    pub status: String,
    /// `fake`, `stripe`, or `trial` for a free trial taken without payment.
    pub provider: String,
    /// RFC 3339, UTC.
    pub created_at: String,
    pub updated_at: String,
    pub paid_at: Option<String>,
    pub expired_at: Option<String>,
    pub refunded_at: Option<String>,
}

#[derive(sqlx::FromRow)]
struct OfferingRow {
    id: Uuid,
    name: String,
    description: String,
    price_minor: i64,
    currency: String,
    billing_interval: String,
    trial: bool,
    scope: String,
    seats: i32,
    quota_erasures: Option<i64>,
    features: Vec<String>,
    term_months: i32,
}

impl From<OfferingRow> for CatalogOffering {
    fn from(r: OfferingRow) -> Self {
        let renewal_notice =
            entitlements::renewal_notice_now(&r.billing_interval, r.price_minor, &r.currency);
        CatalogOffering {
            renewal_notice,
            id: r.id,
            name: r.name,
            description: r.description,
            price_minor: r.price_minor,
            currency: r.currency,
            interval: r.billing_interval,
            trial: r.trial,
            grants: Grants {
                scope: r.scope,
                seats: r.seats,
                quota_erasures: r.quota_erasures,
                features: r.features,
                term_months: r.term_months,
            },
        }
    }
}

#[derive(sqlx::FromRow)]
struct OrderRow {
    id: Uuid,
    offering_id: Uuid,
    offering_name: String,
    price_minor: i64,
    currency: String,
    billing_interval: String,
    scope: String,
    seats: i32,
    quota_erasures: Option<i64>,
    features: Vec<String>,
    term_months: i32,
    status: String,
    provider: String,
    created_at: String,
    updated_at: String,
    paid_at: Option<String>,
    expired_at: Option<String>,
    refunded_at: Option<String>,
}

impl From<OrderRow> for Order {
    fn from(r: OrderRow) -> Self {
        Order {
            id: r.id,
            offering_id: r.offering_id,
            offering_name: r.offering_name,
            price_minor: r.price_minor,
            currency: r.currency,
            interval: r.billing_interval,
            grants: Grants {
                scope: r.scope,
                seats: r.seats,
                quota_erasures: r.quota_erasures,
                features: r.features,
                term_months: r.term_months,
            },
            status: r.status,
            provider: r.provider,
            created_at: r.created_at,
            updated_at: r.updated_at,
            paid_at: r.paid_at,
            expired_at: r.expired_at,
            refunded_at: r.refunded_at,
        }
    }
}

/// A refused checkout, or a failure.
#[derive(Debug, thiserror::Error)]
pub enum OrderError {
    /// The offering is unknown, a draft or retired.
    #[error("the offering is not for sale")]
    NotForSale,
    /// A checkout of a free trial offering.
    #[error("the offering is a free trial")]
    IsTrial,
    /// `POST /api/trials` for an offering that is not a free trial.
    #[error("the offering is not a free trial")]
    NotTrial,
    /// The organization has already taken this free trial.
    #[error("the organization has already taken this free trial")]
    TrialTaken,
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// As [`ApplyError::Licence`].
    #[error("withdrawing renewal grace failed: {0}")]
    Licence(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

impl From<ApplyError> for OrderError {
    fn from(e: ApplyError) -> Self {
        match e {
            ApplyError::Db(e) => Self::Db(e),
            ApplyError::Audit(e) => Self::Audit(e),
            ApplyError::Licence(why) => Self::Licence(why),
        }
    }
}

/// What [`apply_event`] did with an event. The names are the
/// `payment_events.outcome` values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The order moved to `paid`, `expired` or `refunded`, or its
    /// entitlement was renewed, changed or ended; the order after the change.
    Applied(Order),
    /// The order (or its entitlement) is already past the event; it is
    /// unchanged.
    Unchanged(Order),
    /// An event type the hub does not act on.
    Ignored,
    /// No order has the event's checkout session, no entitlement its
    /// subscription, or none the refunded payment.
    Unmatched,
    /// A paid event or paid renewal disagrees with the order's snapshot
    /// (amount, currency or order reference); nothing changes.
    Mismatch(Order),
}

impl ApplyOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApplyOutcome::Applied(_) => "applied",
            ApplyOutcome::Unchanged(_) => "unchanged",
            ApplyOutcome::Ignored => "ignored",
            ApplyOutcome::Unmatched => "unmatched",
            ApplyOutcome::Mismatch(_) => "mismatch",
        }
    }
}

/// A failure applying an event.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
    /// Withdrawing renewal grace from a station's licence failed (signing,
    /// or stored data unusable): the event is not stored, and the provider
    /// retries it.
    #[error("withdrawing renewal grace failed: {0}")]
    Licence(String),
}

impl From<crate::stations::StationError> for ApplyError {
    fn from(e: crate::stations::StationError) -> Self {
        use crate::stations::StationError;
        match e {
            StationError::Db(e) => Self::Db(e),
            StationError::Audit(e) => Self::Audit(e),
            other => Self::Licence(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Queries

const OFFERING_COLUMNS: &str = "id, name, description, price_minor, currency, billing_interval, \
     trial, scope, seats, quota_erasures, features, term_months";

/// The provider name of an order or entitlement taken as a free trial.
pub const TRIAL_PROVIDER: &str = "trial";

/// Migration 0020's partial unique index: one free trial order per
/// organization per offering.
const TRIAL_ONCE_INDEX: &str = "orders_trial_once_key";

/// `SELECT … FROM orders o`, every column of [`OrderRow`]; callers add the
/// `WHERE` (and qualify columns with `o.`).
fn select_orders() -> String {
    format!(
        "SELECT o.id, o.offering_id, o.offering_name, o.price_minor, o.currency, \
                o.billing_interval, o.scope, o.seats, o.quota_erasures, o.features, \
                o.term_months, o.status, o.provider, \
                to_char(o.created_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS created_at, \
                to_char(o.updated_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS updated_at, \
                to_char(o.paid_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS paid_at, \
                to_char(o.expired_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS expired_at, \
                to_char(o.refunded_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS refunded_at \
         FROM orders o"
    )
}

/// Published offerings, by lower-cased name, then id.
pub async fn catalog<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
) -> Result<Vec<CatalogOffering>, sqlx::Error> {
    let rows: Vec<OfferingRow> = sqlx::query_as(&format!(
        "SELECT {OFFERING_COLUMNS} FROM offerings WHERE status = 'published' \
         ORDER BY lower(name), id"
    ))
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(CatalogOffering::from).collect())
}

/// The organization's orders, newest first.
pub async fn list<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
) -> Result<Vec<Order>, sqlx::Error> {
    let rows: Vec<OrderRow> = sqlx::query_as(&format!(
        "{} WHERE o.organization_id = $1 ORDER BY o.created_at DESC, o.id DESC",
        select_orders()
    ))
    .bind(organization_id)
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(Order::from).collect())
}

/// One of the organization's orders; `None` for another organization's.
pub async fn get<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    id: Uuid,
) -> Result<Option<Order>, sqlx::Error> {
    let row: Option<OrderRow> = sqlx::query_as(&format!(
        "{} WHERE o.organization_id = $1 AND o.id = $2",
        select_orders()
    ))
    .bind(organization_id)
    .bind(id)
    .fetch_optional(executor)
    .await?;
    Ok(row.map(Order::from))
}

/// The organization's order for `provider`'s checkout `session_id`, locked
/// `FOR UPDATE` so concurrent changes to it serialise; `None` for another
/// organization's.
pub async fn by_session(
    conn: &mut PgConnection,
    organization_id: Uuid,
    provider: &str,
    session_id: &str,
) -> Result<Option<Order>, sqlx::Error> {
    let row: Option<OrderRow> = sqlx::query_as(&format!(
        "{} WHERE o.organization_id = $1 AND o.provider = $2 AND o.provider_session_id = $3 \
         FOR UPDATE",
        select_orders()
    ))
    .bind(organization_id)
    .bind(provider)
    .bind(session_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(Order::from))
}

/// The order for `provider`'s checkout `session_id`, any organization,
/// locked `FOR UPDATE`.
async fn lock_by_session(
    conn: &mut PgConnection,
    provider: &str,
    session_id: &str,
) -> Result<Option<Order>, sqlx::Error> {
    let row: Option<OrderRow> = sqlx::query_as(&format!(
        "{} WHERE o.provider = $1 AND o.provider_session_id = $2 FOR UPDATE",
        select_orders()
    ))
    .bind(provider)
    .bind(session_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(Order::from))
}

/// The organization's stored customer id at `provider`, if any.
pub async fn billing_customer<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    provider: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT customer_id FROM billing_customers WHERE organization_id = $1 AND provider = $2",
    )
    .bind(organization_id)
    .bind(provider)
    .fetch_optional(executor)
    .await
}

pub(crate) async fn fetch(conn: &mut PgConnection, id: Uuid) -> Result<Order, sqlx::Error> {
    let row: OrderRow = sqlx::query_as(&format!("{} WHERE o.id = $1", select_orders()))
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(row.into())
}

// ---------------------------------------------------------------------------
// Changes

/// What [`create`] returns: the pending order and where to send the buyer.
#[derive(Debug, Clone)]
pub struct Checkout {
    pub order: Order,
    pub checkout_url: url::Url,
}

/// Start a checkout of `offering_id` for `user_id`'s organization, in the
/// caller's transaction: read the offering `FOR SHARE` (so a concurrent
/// retire waits; anything but `published` is [`OrderError::NotForSale`]),
/// ask `provider` for a hosted checkout (with the organization's stored
/// customer at that provider, if any), then insert the pending order with
/// the session id and audit `order.created {offering_id, price_minor,
/// currency, provider}`. A provider error writes nothing. A free trial is
/// [`OrderError::IsTrial`], before the provider is called.
pub async fn create(
    conn: &mut PgConnection,
    provider: &dyn PaymentProvider,
    organization_id: Uuid,
    user_id: Uuid,
    offering_id: Uuid,
) -> Result<Checkout, OrderError> {
    let offering: Option<OfferingRow> = sqlx::query_as(&format!(
        "SELECT {OFFERING_COLUMNS} FROM offerings WHERE id = $1 AND status = 'published' \
         FOR SHARE"
    ))
    .bind(offering_id)
    .fetch_optional(&mut *conn)
    .await?;
    let offering = CatalogOffering::from(offering.ok_or(OrderError::NotForSale)?);
    if offering.trial {
        return Err(OrderError::IsTrial);
    }
    let order_id = Uuid::new_v4();
    let customer_id = billing_customer(&mut *conn, organization_id, provider.name()).await?;
    let session = provider
        .create_checkout(&CheckoutOrder {
            order_id,
            organization_id,
            offering_id,
            offering_name: offering.name.clone(),
            price_minor: offering.price_minor,
            currency: offering.currency.clone(),
            interval: offering.interval.clone(),
            customer_id,
            renewal_notice: offering.renewal_notice.clone(),
        })
        .await?;
    let g = &offering.grants;
    sqlx::query(
        "INSERT INTO orders (id, organization_id, offering_id, created_by, offering_name, \
                             price_minor, currency, billing_interval, scope, seats, \
                             quota_erasures, features, term_months, provider, \
                             provider_session_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(order_id)
    .bind(organization_id)
    .bind(offering_id)
    .bind(user_id)
    .bind(&offering.name)
    .bind(offering.price_minor)
    .bind(&offering.currency)
    .bind(&offering.interval)
    .bind(&g.scope)
    .bind(g.seats)
    .bind(g.quota_erasures)
    .bind(&g.features)
    .bind(g.term_months)
    .bind(provider.name())
    .bind(&session.session_id)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &actor(user_id),
            action: "order.created",
            subject: &subject(order_id),
            detail: json!({
                "offering_id": offering_id,
                "price_minor": offering.price_minor,
                "currency": offering.currency,
                "provider": provider.name(),
            }),
        },
    )
    .await?;
    let order = fetch(conn, order_id).await?;
    Ok(Checkout {
        order,
        checkout_url: session.url,
    })
}

/// What [`take_trial`] returns: the paid order and its entitlement.
#[derive(Debug, Clone)]
pub struct TrialTaken {
    pub order: Order,
    pub entitlement: entitlements::Entitlement,
}

/// Take the free trial `offering_id` for `user_id`'s organization, in the
/// caller's transaction (ADR-0013 §2), with no payment provider: read the
/// offering `FOR SHARE` (anything but `published` is
/// [`OrderError::NotForSale`], a priced one [`OrderError::NotTrial`]),
/// insert its snapshot as an order already `paid` (provider `trial`,
/// session `trial:<order id>`), audit `order.trial_taken {offering_id}` and
/// grant its entitlement ([`entitlements::grant_trial`]). A second take by
/// the same organization hits migration 0020's partial unique index and is
/// [`OrderError::TrialTaken`]; the caller's transaction must then be rolled
/// back.
pub async fn take_trial(
    conn: &mut PgConnection,
    organization_id: Uuid,
    user_id: Uuid,
    offering_id: Uuid,
) -> Result<TrialTaken, OrderError> {
    let offering: Option<OfferingRow> = sqlx::query_as(&format!(
        "SELECT {OFFERING_COLUMNS} FROM offerings WHERE id = $1 AND status = 'published' \
         FOR SHARE"
    ))
    .bind(offering_id)
    .fetch_optional(&mut *conn)
    .await?;
    let offering = CatalogOffering::from(offering.ok_or(OrderError::NotForSale)?);
    if !offering.trial {
        return Err(OrderError::NotTrial);
    }
    let order_id = Uuid::new_v4();
    let g = &offering.grants;
    sqlx::query(
        "INSERT INTO orders (id, organization_id, offering_id, created_by, offering_name, \
                             price_minor, currency, billing_interval, scope, seats, \
                             quota_erasures, features, term_months, provider, \
                             provider_session_id, status, paid_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, 'paid', now())",
    )
    .bind(order_id)
    .bind(organization_id)
    .bind(offering_id)
    .bind(user_id)
    .bind(&offering.name)
    .bind(offering.price_minor)
    .bind(&offering.currency)
    .bind(&offering.interval)
    .bind(&g.scope)
    .bind(g.seats)
    .bind(g.quota_erasures)
    .bind(&g.features)
    .bind(g.term_months)
    .bind(TRIAL_PROVIDER)
    .bind(format!("{TRIAL_PROVIDER}:{order_id}"))
    .execute(&mut *conn)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.constraint() == Some(TRIAL_ONCE_INDEX) => {
            OrderError::TrialTaken
        }
        _ => OrderError::Db(e),
    })?;
    let actor = actor(user_id);
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &actor,
            action: "order.trial_taken",
            subject: &subject(order_id),
            detail: json!({ "offering_id": offering_id }),
        },
    )
    .await?;
    let entitlement_id = entitlements::grant_trial(conn, order_id, &actor).await?;
    let entitlement = entitlements::get(&mut *conn, organization_id, entitlement_id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    Ok(TrialTaken {
        order: fetch(conn, order_id).await?,
        entitlement,
    })
}

/// Apply a verified provider event, in the caller's transaction: the only
/// code that changes an order's status. Only
/// [`crate::payment_events::receive`] calls it, after storing the event.
///
/// - An `Ignored` event is [`ApplyOutcome::Ignored`].
/// - A completed or expired event locks its order by provider and session
///   id; none is [`ApplyOutcome::Unmatched`], and an order that is no longer
///   pending is [`ApplyOutcome::Unchanged`].
/// - A completed event whose amount (minor units and currency) or order
///   reference differs from the order is [`ApplyOutcome::Mismatch`], logged
///   at error; the order stays pending.
/// - Otherwise the order moves to `paid` or `expired`, the time is stamped
///   and `order.paid` / `order.expired {event_id}` is audited as
///   `provider:<name>` ([`ApplyOutcome::Applied`]). A paid order gets its
///   one entitlement ([`entitlements::grant`]). A paid event that names
///   the buyer's customer stores it in `billing_customers` ([`store_customer`]):
///   inserted, or replacing a different stored customer; the same customer
///   writes nothing.
/// - A renewal payment, a failed renewal, a renewal change or a subscription
///   end changes the subscription's entitlement only ([`entitlements::renew`],
///   [`entitlements::renewal_failed`], [`entitlements::change_renewal`],
///   [`entitlements::end`]).
/// - A full refund marks the refunded payment's entitlement `refunded`
///   ([`entitlements::refund`]) and then its order `paid → refunded`, audited
///   `order.refunded {event_id, payment_ref}`.
///
/// Only `Applied` writes anything.
pub async fn apply_event(
    conn: &mut PgConnection,
    event: &VerifiedEvent,
) -> Result<ApplyOutcome, ApplyError> {
    // Which order moves, from which status to which, with its stamp, audit
    // action and (for a refund) the refunded payment.
    let (order, from, to, stamp, action, payment_ref) = match event.kind() {
        EventKind::Ignored { .. } => return Ok(ApplyOutcome::Ignored),
        EventKind::RenewalPaid { .. } => return entitlements::renew(conn, event).await,
        EventKind::RenewalChanged { .. } => return entitlements::change_renewal(conn, event).await,
        EventKind::SubscriptionEnded { .. } => return entitlements::end(conn, event).await,
        EventKind::RenewalFailed { .. } => return entitlements::renewal_failed(conn, event).await,
        EventKind::Refunded { .. } => match entitlements::refund(conn, event).await? {
            entitlements::Refund::Done(outcome) => return Ok(*outcome),
            entitlements::Refund::Order {
                order_id,
                payment_ref,
            } => (
                fetch(conn, order_id).await?,
                "paid",
                "refunded",
                "refunded_at",
                "order.refunded",
                Some(payment_ref),
            ),
        },
        EventKind::CheckoutCompleted { session_id } | EventKind::CheckoutExpired { session_id } => {
            let Some(order) = lock_by_session(conn, event.provider(), session_id).await? else {
                return Ok(ApplyOutcome::Unmatched);
            };
            if order.status != "pending" {
                return Ok(ApplyOutcome::Unchanged(order));
            }
            if matches!(event.kind(), EventKind::CheckoutExpired { .. }) {
                (
                    order,
                    "pending",
                    "expired",
                    "expired_at",
                    "order.expired",
                    None,
                )
            } else {
                let amount_differs = event.amount().is_some_and(|(minor, currency)| {
                    minor != order.price_minor || currency != order.currency
                });
                let ref_differs = event.order_ref().is_some_and(|r| r != order.id);
                if amount_differs || ref_differs {
                    tracing::error!(
                        order_id = %order.id,
                        event_id = event.id(),
                        provider = event.provider(),
                        expected_amount = order.price_minor,
                        expected_currency = %order.currency,
                        event_amount = ?event.amount(),
                        event_order_ref = ?event.order_ref(),
                        "a paid payment event disagrees with its order; the order stays \
                         pending for a human to check"
                    );
                    return Ok(ApplyOutcome::Mismatch(order));
                }
                (order, "pending", "paid", "paid_at", "order.paid", None)
            }
        }
    };
    sqlx::query(&format!(
        "UPDATE orders SET status = $2, {stamp} = now(), updated_at = now() \
         WHERE id = $1 AND status = $3"
    ))
    .bind(order.id)
    .bind(to)
    .bind(from)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &format!("provider:{}", event.provider()),
            action,
            subject: &subject(order.id),
            detail: match payment_ref {
                Some(payment_ref) => json!({ "event_id": event.id(), "payment_ref": payment_ref }),
                None => json!({ "event_id": event.id() }),
            },
        },
    )
    .await?;
    if to == "paid" {
        if let Some(customer) = event.customer_id() {
            store_customer(conn, order.id, event.provider(), customer).await?;
        }
        entitlements::grant(conn, order.id, event).await?;
    }
    Ok(ApplyOutcome::Applied(fetch(conn, order.id).await?))
}

/// Store `customer` as the customer at `provider` of the organization that
/// placed `order_id`, from a verified paid event for that order (decision
/// 10): insert it, or
/// replace a different stored customer (logged at info with both ids); the
/// same customer writes nothing. A customer already stored for another
/// organization is left alone and logged at error, rather than failing the
/// event (which would make the provider retry it for days).
async fn store_customer(
    conn: &mut PgConnection,
    order_id: Uuid,
    provider: &str,
    customer: &str,
) -> Result<(), sqlx::Error> {
    let organization_id: Uuid =
        sqlx::query_scalar("SELECT organization_id FROM orders WHERE id = $1")
            .bind(order_id)
            .fetch_one(&mut *conn)
            .await?;
    let owner: Option<Uuid> = sqlx::query_scalar(
        "SELECT organization_id FROM billing_customers WHERE provider = $1 AND customer_id = $2",
    )
    .bind(provider)
    .bind(customer)
    .fetch_optional(&mut *conn)
    .await?;
    match owner {
        Some(owner) if owner == organization_id => return Ok(()),
        Some(owner) => {
            tracing::error!(
                %organization_id,
                other_organization_id = %owner,
                provider,
                customer_id = customer,
                "a paid event names a customer stored for another organization; \
                 the stored customers are left unchanged"
            );
            return Ok(());
        }
        None => {}
    }
    let previous: Option<String> = sqlx::query_scalar(
        "SELECT customer_id FROM billing_customers \
         WHERE organization_id = $1 AND provider = $2 FOR UPDATE",
    )
    .bind(organization_id)
    .bind(provider)
    .fetch_optional(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO billing_customers (organization_id, provider, customer_id) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (organization_id, provider) DO UPDATE \
         SET customer_id = EXCLUDED.customer_id, updated_at = now() \
         WHERE billing_customers.customer_id <> EXCLUDED.customer_id",
    )
    .bind(organization_id)
    .bind(provider)
    .bind(customer)
    .execute(&mut *conn)
    .await?;
    if let Some(previous) = previous {
        tracing::info!(
            %organization_id,
            provider,
            old_customer_id = %previous,
            new_customer_id = customer,
            "replaced the organization's stored billing customer"
        );
    }
    Ok(())
}
