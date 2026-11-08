//! Orders and the customer catalog (offerings-and-payments story 2, CAP-2):
//! the data layer behind [`crate::order_routes`].
//!
//! - **Catalog**: published offerings only, by lower-cased name, then id.
//! - **An order snapshots** the offering's name, price, currency, interval
//!   and grants, so retiring the offering never changes what was bought.
//! - **Lifecycle** `pending → paid` or `pending → expired`; both terminal.
//!   [`apply_event`], fed a [`VerifiedEvent`] that only a provider's
//!   `verify_event` can make, is the only code that changes an order's
//!   status.
//! - **Every state change writes one audit row** in its transaction:
//!   `order.created` (actor `user:<id>`), then `order.paid` or
//!   `order.expired` (actor `provider:<name>`, detail `{event_id}`); the
//!   subject is `order:<id>`.
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
use crate::offering_admin::Grants;
use crate::payments::{CheckoutOrder, EventKind, PaymentProvider, ProviderError, VerifiedEvent};

/// Audit subject of one order.
pub fn subject(id: Uuid) -> String {
    format!("order:{id}")
}

// ---------------------------------------------------------------------------
// Shapes

/// A published offering as a customer sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogOffering {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub price_minor: i64,
    pub currency: String,
    /// `one_off` or `annual`.
    pub interval: String,
    pub grants: Grants,
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
    /// `pending`, `paid` or `expired`.
    pub status: String,
    /// `fake` or `stripe`.
    pub provider: String,
    /// RFC 3339, UTC.
    pub created_at: String,
    pub updated_at: String,
    pub paid_at: Option<String>,
    pub expired_at: Option<String>,
}

#[derive(sqlx::FromRow)]
struct OfferingRow {
    id: Uuid,
    name: String,
    description: String,
    price_minor: i64,
    currency: String,
    billing_interval: String,
    scope: String,
    seats: i32,
    quota_erasures: Option<i64>,
    features: Vec<String>,
    term_months: i32,
}

impl From<OfferingRow> for CatalogOffering {
    fn from(r: OfferingRow) -> Self {
        CatalogOffering {
            id: r.id,
            name: r.name,
            description: r.description,
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
        }
    }
}

/// A refused checkout, or a failure.
#[derive(Debug, thiserror::Error)]
pub enum OrderError {
    /// The offering is unknown, a draft or retired.
    #[error("the offering is not for sale")]
    NotForSale,
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

/// A failure applying an event.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

// ---------------------------------------------------------------------------
// Queries

const OFFERING_COLUMNS: &str = "id, name, description, price_minor, currency, billing_interval, \
     scope, seats, quota_erasures, features, term_months";

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
                to_char(o.expired_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS expired_at \
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

async fn fetch(conn: &mut PgConnection, id: Uuid) -> Result<Order, sqlx::Error> {
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
/// ask `provider` for a hosted checkout, then insert the pending order with
/// the session id and audit `order.created {offering_id, price_minor,
/// currency, provider}`. A provider error writes nothing.
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
    let order_id = Uuid::new_v4();
    let session = provider
        .create_checkout(&CheckoutOrder {
            order_id,
            organization_id,
            offering_id,
            offering_name: offering.name.clone(),
            price_minor: offering.price_minor,
            currency: offering.currency.clone(),
            interval: offering.interval.clone(),
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

/// Apply a verified provider event, in the caller's transaction: the only
/// code that changes an order's status.
///
/// A completed or expired event locks its order by provider and session id
/// and, only if the order is `pending`, moves it to `paid` or `expired`,
/// stamps the time and audits `order.paid` / `order.expired {event_id}` as
/// `provider:<name>`. An event for an unknown session is `None`; one for an
/// order that is no longer pending returns it unchanged; an `Ignored` event
/// is `None`. None of those writes anything.
pub async fn apply_event(
    conn: &mut PgConnection,
    event: &VerifiedEvent,
) -> Result<Option<Order>, ApplyError> {
    let (session_id, to, stamp, action) = match event.kind() {
        EventKind::CheckoutCompleted { session_id } => {
            (session_id, "paid", "paid_at", "order.paid")
        }
        EventKind::CheckoutExpired { session_id } => {
            (session_id, "expired", "expired_at", "order.expired")
        }
        EventKind::Ignored { .. } => return Ok(None),
    };
    let Some(order) = lock_by_session(conn, event.provider(), session_id).await? else {
        return Ok(None);
    };
    if order.status != "pending" {
        return Ok(Some(order));
    }
    sqlx::query(&format!(
        "UPDATE orders SET status = $2, {stamp} = now(), updated_at = now() \
         WHERE id = $1 AND status = 'pending'"
    ))
    .bind(order.id)
    .bind(to)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &format!("provider:{}", event.provider()),
            action,
            subject: &subject(order.id),
            detail: json!({ "event_id": event.id() }),
        },
    )
    .await?;
    Ok(Some(fetch(conn, order.id).await?))
}
