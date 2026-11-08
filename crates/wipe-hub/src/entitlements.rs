//! Entitlements (offerings-and-payments story 4, CAP-4): what an
//! organization may license because it paid for an order, and the
//! subscription events that renew, stop or refund it. The only code that
//! writes the `entitlements` and `entitlement_renewals` tables.
//!
//! - **Fulfilment** ([`grant`]): when [`crate::orders::apply_event`] moves an
//!   order to `paid`, it creates exactly one entitlement in the same
//!   transaction: the term runs from `paid_at` for the order's
//!   `term_months`, and it renews when the payment started a subscription.
//!   Seats, scope, quota, features and term length are read from the order's
//!   immutable snapshot, never copied.
//! - **Renewal** ([`renew`]): each paid renewal invoice extends the term by
//!   12 months from its current end, once per invoice (`entitlement_renewals`
//!   is keyed by the provider's invoice id).
//! - **Renewal changes** ([`change_renewal`]): an owner cancelling at period
//!   end (or resuming) at the provider sets `renews`, ordered by the
//!   provider's event time. **Subscription end** ([`end`]) marks the
//!   entitlement `cancelled`: the term runs to its end and is not renewed.
//! - **Refunds** ([`refund`]): a full refund of the first payment or of a
//!   renewal marks the entitlement `refunded`, which withdraws its seats; the
//!   caller marks the order `refunded`.
//! - **Nothing here touches a licence.** Cancellation, refund and failed
//!   renewals change entitlement and order rows only; no licence is revoked,
//!   shortened or reissued (ROADMAP §0.2).
//! - Every grant and every change of status, term or `renews` writes one
//!   audit row in its transaction (actor `provider:<name>`, subject
//!   `entitlement:<id>`). Advancing the ordering clock
//!   (`renewal_changed_at`) on an otherwise unchanged event is not audited
//!   (decision 8).
//! - [`list`] gives an organization its entitlements and its available seats:
//!   the seats of every entitlement that is not refunded and whose term has
//!   not ended.
//!
//! The owner-approved auto-renewal notice ([`renewal_notice`]) is built here,
//! once, for the portal and for Stripe's checkout page.
//!
//! Migration 0013 repeats the lifecycle rules as CHECKs and triggers.

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::postgres::PgConnection;
use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

use crate::admin::RFC3339_UTC;
use crate::audit::{self, AuditEntry};
use crate::offering_admin::Grants;
use crate::orders::{self, ApplyError, ApplyOutcome};
use crate::payments::{EventKind, VerifiedEvent};

/// Audit subject of one entitlement.
pub fn subject(id: Uuid) -> String {
    format!("entitlement:{id}")
}

/// The months one renewal adds.
pub const RENEWAL_MONTHS: i32 = 12;

// ---------------------------------------------------------------------------
// Shapes

/// An entitlement as its organization sees it. Timestamps are RFC 3339 UTC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entitlement {
    pub id: Uuid,
    pub order_id: Uuid,
    /// The offering's name when the order was created.
    pub offering_name: String,
    /// `one_off` or `annual`.
    pub interval: String,
    /// The order's snapshot of what the offering grants.
    pub grants: Grants,
    pub term_start: String,
    pub term_end: String,
    /// `active`, `cancelled` or `refunded`.
    pub status: String,
    /// Whether the provider will charge for, and the hub grant, another year.
    pub renews: bool,
    /// Not refunded and the term has not ended: its seats are available.
    pub current: bool,
    pub cancelled_at: Option<String>,
    pub refunded_at: Option<String>,
}

/// An organization's entitlements, newest first, and the seats they make
/// available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seats {
    pub entitlements: Vec<Entitlement>,
    /// The seats of every current entitlement.
    pub available_seats: i64,
}

#[derive(sqlx::FromRow)]
struct EntitlementRow {
    id: Uuid,
    order_id: Uuid,
    offering_name: String,
    billing_interval: String,
    scope: String,
    seats: i32,
    quota_erasures: Option<i64>,
    features: Vec<String>,
    term_months: i32,
    term_start: String,
    term_end: String,
    status: String,
    renews: bool,
    current: bool,
    cancelled_at: Option<String>,
    refunded_at: Option<String>,
}

impl From<EntitlementRow> for Entitlement {
    fn from(r: EntitlementRow) -> Self {
        Entitlement {
            id: r.id,
            order_id: r.order_id,
            offering_name: r.offering_name,
            interval: r.billing_interval,
            grants: Grants {
                scope: r.scope,
                seats: r.seats,
                quota_erasures: r.quota_erasures,
                features: r.features,
                term_months: r.term_months,
            },
            term_start: r.term_start,
            term_end: r.term_end,
            status: r.status,
            renews: r.renews,
            current: r.current,
            cancelled_at: r.cancelled_at,
            refunded_at: r.refunded_at,
        }
    }
}

/// Whether the entitlement `e` is current: not refunded and its term has not
/// ended (a cancelled one stays current until then). The one definition of
/// the rule, shared by [`list`]'s `current`, station seats
/// ([`crate::stations`]) and the download check
/// ([`crate::downloads::is_entitled`]), so they cannot diverge.
pub(crate) const CURRENT_SQL: &str = "(e.status <> 'refunded' AND e.term_end > now())";

/// `SELECT … FROM entitlements e JOIN orders o`, every column of
/// [`EntitlementRow`]; callers add the `WHERE`.
fn select_entitlements() -> String {
    format!(
        "SELECT e.id, e.order_id, o.offering_name, o.billing_interval, o.scope, o.seats, \
                o.quota_erasures, o.features, o.term_months, \
                to_char(e.term_start AT TIME ZONE 'UTC', {RFC3339_UTC}) AS term_start, \
                to_char(e.term_end AT TIME ZONE 'UTC', {RFC3339_UTC}) AS term_end, \
                e.status, e.renews, \
                {CURRENT_SQL} AS current, \
                to_char(e.cancelled_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS cancelled_at, \
                to_char(e.refunded_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS refunded_at \
         FROM entitlements e JOIN orders o ON o.id = e.order_id"
    )
}

/// The organization's entitlements, newest first, and its available seats.
pub async fn list<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
) -> Result<Seats, sqlx::Error> {
    let rows: Vec<EntitlementRow> = sqlx::query_as(&format!(
        "{} WHERE e.organization_id = $1 ORDER BY e.created_at DESC, e.id DESC",
        select_entitlements()
    ))
    .bind(organization_id)
    .fetch_all(executor)
    .await?;
    let entitlements: Vec<Entitlement> = rows.into_iter().map(Entitlement::from).collect();
    let available_seats = entitlements
        .iter()
        .filter(|e| e.current)
        .map(|e| i64::from(e.grants.seats))
        .sum();
    Ok(Seats {
        entitlements,
        available_seats,
    })
}

/// One of the organization's entitlements; `None` for another
/// organization's.
pub async fn get<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    id: Uuid,
) -> Result<Option<Entitlement>, sqlx::Error> {
    let row: Option<EntitlementRow> = sqlx::query_as(&format!(
        "{} WHERE e.organization_id = $1 AND e.id = $2",
        select_entitlements()
    ))
    .bind(organization_id)
    .bind(id)
    .fetch_optional(executor)
    .await?;
    Ok(row.map(Entitlement::from))
}

/// The provider-side state of an entitlement, locked `FOR UPDATE`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Locked {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub order_id: Uuid,
    pub provider: String,
    pub subscription_id: Option<String>,
    pub status: String,
    pub renews: bool,
}

const LOCKED_COLUMNS: &str =
    "id, organization_id, order_id, provider, subscription_id, status, renews";

/// The organization's entitlement `id`, locked `FOR UPDATE`; `None` for
/// another organization's.
pub async fn lock_for_organization(
    conn: &mut PgConnection,
    organization_id: Uuid,
    id: Uuid,
) -> Result<Option<Locked>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {LOCKED_COLUMNS} FROM entitlements \
         WHERE organization_id = $1 AND id = $2 FOR UPDATE"
    ))
    .bind(organization_id)
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
}

async fn lock_by_subscription(
    conn: &mut PgConnection,
    provider: &str,
    subscription_id: &str,
) -> Result<Option<Locked>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {LOCKED_COLUMNS} FROM entitlements \
         WHERE provider = $1 AND subscription_id = $2 FOR UPDATE"
    ))
    .bind(provider)
    .bind(subscription_id)
    .fetch_optional(&mut *conn)
    .await
}

fn provider_actor(event: &VerifiedEvent) -> String {
    format!("provider:{}", event.provider())
}

// ---------------------------------------------------------------------------
// Fulfilment

/// Create the entitlement of `order`, which `event` has just marked paid, in
/// the caller's transaction (called only by [`orders::apply_event`]): the
/// term runs from the order's `paid_at` for its `term_months` (SQL month
/// arithmetic), the subscription and payment reference come from the event,
/// and it renews when there is a subscription. Audits `entitlement.granted
/// {order_id, term_end}`.
pub(crate) async fn grant(
    conn: &mut PgConnection,
    order_id: Uuid,
    event: &VerifiedEvent,
) -> Result<Uuid, ApplyError> {
    let (id, term_end): (Uuid, String) = sqlx::query_as(&format!(
        "INSERT INTO entitlements (organization_id, order_id, provider, subscription_id, \
                                   payment_ref, term_start, term_end, renews) \
         SELECT o.organization_id, o.id, o.provider, $2, $3, o.paid_at, \
                o.paid_at + make_interval(months => o.term_months), $2::text IS NOT NULL \
         FROM orders o WHERE o.id = $1 AND o.status = 'paid' \
         RETURNING id, to_char(term_end AT TIME ZONE 'UTC', {RFC3339_UTC})"
    ))
    .bind(order_id)
    .bind(event.subscription_id())
    .bind(event.payment_ref())
    .fetch_one(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &provider_actor(event),
            action: "entitlement.granted",
            subject: &subject(id),
            detail: json!({ "order_id": order_id, "term_end": term_end }),
        },
    )
    .await?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// Subscription events

/// Apply a [`EventKind::RenewalPaid`] event, in the caller's transaction
/// (called only by [`orders::apply_event`]):
///
/// - no entitlement has the subscription: [`ApplyOutcome::Unmatched`],
///   logged at error (the provider took money for no known entitlement);
/// - the invoice was already applied: [`ApplyOutcome::Unchanged`];
/// - the entitlement is cancelled or refunded: `Unchanged`, logged at error
///   (the provider took money the hub will not grant a year for);
/// - the amount differs from the order's snapshot: [`ApplyOutcome::Mismatch`],
///   logged at error, nothing changes;
/// - otherwise the term grows by 12 months from its current end, one
///   `entitlement_renewals` row is written, and `entitlement.renewed
///   {event_id, invoice_id, previous_term_end, term_end}` is audited.
pub(crate) async fn renew(
    conn: &mut PgConnection,
    event: &VerifiedEvent,
) -> Result<ApplyOutcome, ApplyError> {
    let EventKind::RenewalPaid {
        subscription_id,
        invoice_id,
    } = event.kind()
    else {
        unreachable!("renew is only called for RenewalPaid");
    };
    let Some(ent) = lock_by_subscription(conn, event.provider(), subscription_id).await? else {
        // A charged year that grants nothing must never be silent. When the
        // event names one of this hub's orders that can still be paid,
        // `payment_events::receive` rolls back and the provider retries.
        tracing::error!(
            event_id = event.id(),
            provider = event.provider(),
            subscription_id = %subscription_id,
            invoice_id = %invoice_id,
            order_ref = ?event.order_ref(),
            "a paid renewal matches no entitlement; cancel the subscription or refund it at \
             the provider"
        );
        return Ok(ApplyOutcome::Unmatched);
    };
    let order = orders::fetch(conn, ent.order_id).await?;
    let seen: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM entitlement_renewals WHERE provider = $1 AND invoice_id = $2)",
    )
    .bind(event.provider())
    .bind(invoice_id)
    .fetch_one(&mut *conn)
    .await?;
    if seen {
        return Ok(ApplyOutcome::Unchanged(order));
    }
    if ent.status != "active" {
        tracing::error!(
            entitlement_id = %ent.id,
            order_id = %ent.order_id,
            status = %ent.status,
            event_id = event.id(),
            provider = event.provider(),
            invoice_id = %invoice_id,
            "a renewal was paid for a cancelled or refunded entitlement; refund it at the \
             provider"
        );
        return Ok(ApplyOutcome::Unchanged(order));
    }
    if event
        .amount()
        .is_some_and(|(minor, currency)| minor != order.price_minor || currency != order.currency)
    {
        tracing::error!(
            entitlement_id = %ent.id,
            order_id = %ent.order_id,
            event_id = event.id(),
            provider = event.provider(),
            invoice_id = %invoice_id,
            expected_amount = order.price_minor,
            expected_currency = %order.currency,
            event_amount = ?event.amount(),
            "a paid renewal disagrees with its order's price; the term is not extended \
             for a human to check"
        );
        return Ok(ApplyOutcome::Mismatch(order));
    }
    // One statement, so the renewal row records exactly the term it extended.
    let (previous, term_end): (String, String) = sqlx::query_as(&format!(
        "WITH old AS (SELECT id, term_end FROM entitlements WHERE id = $1), \
              grown AS (UPDATE entitlements e \
                        SET term_end = e.term_end + make_interval(months => $2), \
                            updated_at = now() \
                        FROM old WHERE e.id = old.id \
                        RETURNING e.id, old.term_end AS previous_term_end, e.term_end) \
         INSERT INTO entitlement_renewals (provider, invoice_id, entitlement_id, \
                                           previous_term_end, term_end) \
         SELECT $3, $4, id, previous_term_end, term_end FROM grown \
         RETURNING to_char(previous_term_end AT TIME ZONE 'UTC', {RFC3339_UTC}), \
                   to_char(term_end AT TIME ZONE 'UTC', {RFC3339_UTC})"
    ))
    .bind(ent.id)
    .bind(RENEWAL_MONTHS)
    .bind(event.provider())
    .bind(invoice_id)
    .fetch_one(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &provider_actor(event),
            action: "entitlement.renewed",
            subject: &subject(ent.id),
            detail: json!({
                "event_id": event.id(),
                "invoice_id": invoice_id,
                "previous_term_end": previous,
                "term_end": term_end,
            }),
        },
    )
    .await?;
    Ok(ApplyOutcome::Applied(order))
}

/// Apply a [`EventKind::RenewalChanged`] event (called only by
/// [`orders::apply_event`]): none is [`ApplyOutcome::Unmatched`]; an
/// entitlement that is not active, or an event older than the last recorded
/// change, is [`ApplyOutcome::Unchanged`]; a `renews` already equal is
/// `Unchanged` too, but still advances `renewal_changed_at` to the event's
/// time (decision 8, unaudited); otherwise `renews` and `renewal_changed_at`
/// (the event's time; left alone for an event without one) are set and
/// `entitlement.renewal_stopped` / `entitlement.renewal_resumed {event_id}`
/// is audited.
pub(crate) async fn change_renewal(
    conn: &mut PgConnection,
    event: &VerifiedEvent,
) -> Result<ApplyOutcome, ApplyError> {
    let EventKind::RenewalChanged {
        subscription_id,
        renews,
    } = event.kind()
    else {
        unreachable!("change_renewal is only called for RenewalChanged");
    };
    let Some(ent) = lock_by_subscription(conn, event.provider(), subscription_id).await? else {
        return Ok(ApplyOutcome::Unmatched);
    };
    let order = orders::fetch(conn, ent.order_id).await?;
    let older: bool = sqlx::query_scalar(
        "SELECT coalesce(to_timestamp($2::bigint) < renewal_changed_at, false) \
         FROM entitlements WHERE id = $1",
    )
    .bind(ent.id)
    .bind(event.occurred_at())
    .fetch_one(&mut *conn)
    .await?;
    if ent.status != "active" || older {
        return Ok(ApplyOutcome::Unchanged(order));
    }
    if ent.renews == *renews {
        // Decision 8: an equal, not older change still advances the ordering
        // clock, so an older opposite change delivered later stays stale. The
        // one write an `Unchanged` outcome makes; not audited.
        if event.occurred_at().is_some() {
            sqlx::query(
                "UPDATE entitlements \
                 SET renewal_changed_at = greatest(coalesce(renewal_changed_at, \
                                                            to_timestamp($2::bigint)), \
                                                   to_timestamp($2::bigint)), \
                     updated_at = now() \
                 WHERE id = $1",
            )
            .bind(ent.id)
            .bind(event.occurred_at())
            .execute(&mut *conn)
            .await?;
        }
        return Ok(ApplyOutcome::Unchanged(order));
    }
    // An event without a time (never from Stripe; the Fake always sends one)
    // leaves the ordering clock alone.
    sqlx::query(
        "UPDATE entitlements \
         SET renews = $2, \
             renewal_changed_at = coalesce(to_timestamp($3::bigint), renewal_changed_at), \
             updated_at = now() \
         WHERE id = $1",
    )
    .bind(ent.id)
    .bind(renews)
    .bind(event.occurred_at())
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &provider_actor(event),
            action: if *renews {
                "entitlement.renewal_resumed"
            } else {
                "entitlement.renewal_stopped"
            },
            subject: &subject(ent.id),
            detail: json!({ "event_id": event.id() }),
        },
    )
    .await?;
    Ok(ApplyOutcome::Applied(order))
}

/// Apply a [`EventKind::SubscriptionEnded`] event (called only by
/// [`orders::apply_event`]): none is [`ApplyOutcome::Unmatched`]; an
/// entitlement that is not active is [`ApplyOutcome::Unchanged`]; otherwise
/// it becomes `cancelled` and stops renewing, `cancelled_at` taken from the
/// event's `occurred_at` when present (else now), and `entitlement.cancelled
/// {event_id}` is audited. The term runs to its end and its seats stay
/// available until then.
pub(crate) async fn end(
    conn: &mut PgConnection,
    event: &VerifiedEvent,
) -> Result<ApplyOutcome, ApplyError> {
    let EventKind::SubscriptionEnded { subscription_id } = event.kind() else {
        unreachable!("end is only called for SubscriptionEnded");
    };
    let Some(ent) = lock_by_subscription(conn, event.provider(), subscription_id).await? else {
        return Ok(ApplyOutcome::Unmatched);
    };
    let order = orders::fetch(conn, ent.order_id).await?;
    if ent.status != "active" {
        return Ok(ApplyOutcome::Unchanged(order));
    }
    // Stripe's time the subscription ended (the event's `created`), so a late
    // delivery still records when it really happened.
    sqlx::query(
        "UPDATE entitlements \
         SET status = 'cancelled', renews = false, \
             cancelled_at = coalesce(to_timestamp($2::bigint), now()), updated_at = now() \
         WHERE id = $1",
    )
    .bind(ent.id)
    .bind(event.occurred_at())
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &provider_actor(event),
            action: "entitlement.cancelled",
            subject: &subject(ent.id),
            detail: json!({ "event_id": event.id() }),
        },
    )
    .await?;
    Ok(ApplyOutcome::Applied(order))
}

/// What [`refund`] found.
pub(crate) enum Refund {
    /// Nothing to change; the outcome to record.
    Done(Box<ApplyOutcome>),
    /// The entitlement is now `refunded`; the caller marks its order
    /// `refunded` too, auditing the refunded `payment_ref`.
    Order { order_id: Uuid, payment_ref: String },
}

/// Apply a [`EventKind::Refunded`] event's entitlement side (called only by
/// [`orders::apply_event`], which then marks the order refunded): the
/// entitlement whose first payment, or one of whose renewal invoices, is
/// among `payment_refs` at this provider. None is
/// [`ApplyOutcome::Unmatched`] (logged at warn); an entitlement already
/// refunded is [`ApplyOutcome::Unchanged`]; otherwise it becomes `refunded`
/// and stops renewing (its seats are withdrawn) and `entitlement.refunded
/// {event_id, payment_ref}` is audited, naming the refunded payment.
pub(crate) async fn refund(
    conn: &mut PgConnection,
    event: &VerifiedEvent,
) -> Result<Refund, ApplyError> {
    let EventKind::Refunded { payment_refs } = event.kind() else {
        unreachable!("refund is only called for Refunded");
    };
    let ent: Option<Locked> = sqlx::query_as(&format!(
        "SELECT {LOCKED_COLUMNS} FROM entitlements \
         WHERE provider = $1 \
           AND (payment_ref = ANY($2) \
                OR id IN (SELECT entitlement_id FROM entitlement_renewals \
                          WHERE provider = $1 AND invoice_id = ANY($2))) \
         ORDER BY created_at, id LIMIT 1 FOR UPDATE"
    ))
    .bind(event.provider())
    .bind(payment_refs)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(ent) = ent else {
        tracing::warn!(
            event_id = event.id(),
            provider = event.provider(),
            payment_refs = ?payment_refs,
            "a refund names no payment of this hub's entitlements"
        );
        return Ok(Refund::Done(Box::new(ApplyOutcome::Unmatched)));
    };
    if ent.status == "refunded" {
        let order = orders::fetch(conn, ent.order_id).await?;
        return Ok(Refund::Done(Box::new(ApplyOutcome::Unchanged(order))));
    }
    // Which of the entitlement's payments was refunded: the first payment,
    // else the earliest matching renewal invoice.
    let payment_ref: String = sqlx::query_scalar(
        "SELECT coalesce( \
             (SELECT payment_ref FROM entitlements WHERE id = $1 AND payment_ref = ANY($2)), \
             (SELECT invoice_id FROM entitlement_renewals \
              WHERE entitlement_id = $1 AND invoice_id = ANY($2) \
              ORDER BY created_at, invoice_id LIMIT 1))",
    )
    .bind(ent.id)
    .bind(payment_refs)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE entitlements \
         SET status = 'refunded', renews = false, refunded_at = now(), updated_at = now() \
         WHERE id = $1",
    )
    .bind(ent.id)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &provider_actor(event),
            action: "entitlement.refunded",
            subject: &subject(ent.id),
            detail: json!({ "event_id": event.id(), "payment_ref": payment_ref }),
        },
    )
    .await?;
    Ok(Refund::Order {
        order_id: ent.order_id,
        payment_ref,
    })
}

// ---------------------------------------------------------------------------
// The auto-renewal notice

/// The owner-approved auto-renewal notice (2026-11-08), shown verbatim on the
/// buy page, the Fake checkout page and Stripe's checkout page. The portal
/// renders `headline` in bold and never composes the copy itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenewalNotice {
    pub headline: String,
    pub detail: String,
}

impl RenewalNotice {
    /// The headline and the detail on one line (Stripe's
    /// `custom_text[submit][message]`).
    pub fn one_line(&self) -> String {
        format!("{} {}", self.headline, self.detail)
    }
}

/// The notice for an annual offering priced `price_minor` `currency`, bought
/// on `today` (UTC): `{price}` as [`format_price`], `{date}` as `today` plus
/// 12 months ([`format_date`]).
pub fn renewal_notice(price_minor: i64, currency: &str, today: Date) -> RenewalNotice {
    let price = format_price(price_minor, currency);
    let date = format_date(add_months(today, RENEWAL_MONTHS));
    RenewalNotice {
        headline: format!("Renews automatically every year at {price} until you cancel."),
        detail: format!(
            "Your first year runs to {date}. Cancel any time from Billing in your account: \
             your current year still runs to its end, and you will not be charged again. \
             We email you before each renewal."
        ),
    }
}

/// [`renewal_notice`] for today (UTC), for an annual offering; `None` for
/// any other interval.
pub fn renewal_notice_now(
    interval: &str,
    price_minor: i64,
    currency: &str,
) -> Option<RenewalNotice> {
    (interval == "annual")
        .then(|| renewal_notice(price_minor, currency, OffsetDateTime::now_utc().date()))
}

/// Digits after the decimal point of `currency` (ISO 4217 minor units): 0 for
/// JPY and the other zero-decimal currencies, 3 for BHD and the other
/// three-decimal ones, 4 for CLF and UYW, else 2.
pub fn minor_unit_digits(currency: &str) -> u32 {
    match currency {
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX"
        | "UYI" | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        "CLF" | "UYW" => 4,
        _ => 2,
    }
}

/// `minor` units of `currency` for display, like the portal's `formatPrice`:
/// `120000` USD is `1,200.00 USD`, `1500000` JPY is `1,500,000 JPY`.
pub fn format_price(minor: i64, currency: &str) -> String {
    let digits = minor_unit_digits(currency);
    let sign = if minor < 0 { "-" } else { "" };
    let abs = minor.unsigned_abs();
    let scale = 10u64.pow(digits);
    let whole = (abs / scale).to_string();
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if digits == 0 {
        format!("{sign}{grouped} {currency}")
    } else {
        let frac = abs % scale;
        format!(
            "{sign}{grouped}.{frac:0width$} {currency}",
            width = digits as usize
        )
    }
}

/// `date` as `8 November 2027`.
pub fn format_date(date: Date) -> String {
    format!("{} {} {}", date.day(), date.month(), date.year())
}

/// `date` plus `months`, clamped to the last day of the month (31 January
/// plus one month is 28 or 29 February), like Postgres interval arithmetic.
pub fn add_months(date: Date, months: i32) -> Date {
    let zero_based = date.year() * 12 + (date.month() as i32 - 1) + months;
    let year = zero_based.div_euclid(12);
    let month = Month::try_from((zero_based.rem_euclid(12) + 1) as u8).expect("1..=12");
    let last = month.length(year);
    Date::from_calendar_date(year, month, date.day().min(last)).expect("a valid date")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn prices_format_like_the_portal() {
        assert_eq!(format_price(120000, "USD"), "1,200.00 USD");
        assert_eq!(format_price(99999999, "USD"), "999,999.99 USD");
        assert_eq!(format_price(5, "USD"), "0.05 USD");
        assert_eq!(format_price(100, "EUR"), "1.00 EUR");
        assert_eq!(format_price(1500000, "JPY"), "1,500,000 JPY");
        assert_eq!(format_price(999, "JPY"), "999 JPY");
        assert_eq!(format_price(1000, "JPY"), "1,000 JPY");
        assert_eq!(format_price(1234567, "BHD"), "1,234.567 BHD");
        assert_eq!(format_price(5, "KWD"), "0.005 KWD");
        assert_eq!(format_price(123456789, "GBP"), "1,234,567.89 GBP");
    }

    #[test]
    fn dates_format_as_day_month_year() {
        assert_eq!(format_date(date!(2027 - 11 - 08)), "8 November 2027");
        assert_eq!(format_date(date!(2028 - 01 - 31)), "31 January 2028");
    }

    #[test]
    fn months_add_with_clamping() {
        assert_eq!(add_months(date!(2026 - 11 - 08), 12), date!(2027 - 11 - 08));
        assert_eq!(add_months(date!(2028 - 02 - 29), 12), date!(2029 - 02 - 28));
        assert_eq!(add_months(date!(2027 - 01 - 31), 1), date!(2027 - 02 - 28));
        assert_eq!(add_months(date!(2027 - 12 - 15), 1), date!(2028 - 01 - 15));
    }

    #[test]
    fn the_notice_is_the_approved_copy() {
        let n = renewal_notice(120000, "USD", date!(2026 - 11 - 08));
        assert_eq!(
            n.headline,
            "Renews automatically every year at 1,200.00 USD until you cancel."
        );
        assert_eq!(
            n.detail,
            "Your first year runs to 8 November 2027. Cancel any time from Billing in your \
             account: your current year still runs to its end, and you will not be charged \
             again. We email you before each renewal."
        );
        assert_eq!(n.one_line(), format!("{} {}", n.headline, n.detail));
        let jpy = renewal_notice(1500000, "JPY", date!(2028 - 02 - 29));
        assert_eq!(
            jpy.headline,
            "Renews automatically every year at 1,500,000 JPY until you cancel."
        );
        assert!(jpy
            .detail
            .starts_with("Your first year runs to 28 February 2029. "));
        assert!(renewal_notice_now("one_off", 120000, "USD").is_none());
        assert!(renewal_notice_now("annual", 120000, "USD").is_some());
    }
}
