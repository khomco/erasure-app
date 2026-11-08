//! The payment event store (offerings-and-payments story 3, CAP-3): the only
//! way from a [`VerifiedEvent`] to [`orders::apply_event`].
//!
//! [`receive`] stores the event under `(provider, event id)` first, so a
//! replayed event conflicts on the key and changes nothing, and concurrent
//! deliveries of one event serialise on it: the second insert waits for the
//! first transaction and then conflicts (or, if the first rolled back,
//! proceeds). Only the event's metadata and the SHA-256 of its raw payload
//! are stored, never the payload, which carries the buyer's personal data.
//!
//! Both the webhook (`POST /api/payments/webhook`) and the Fake provider's
//! pay and abandon routes go through [`receive`].

use sha2::{Digest, Sha256};
use sqlx::postgres::PgConnection;

use crate::orders::{self, ApplyError, ApplyOutcome};
use crate::payments::{EventKind, VerifiedEvent};

/// What [`receive`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Receipt {
    /// The event was stored before; nothing was written.
    Duplicate,
    /// A paid event for one of the hub's own orders (it names an order
    /// reference) whose checkout session no order has yet: the order is not
    /// committed yet, or was lost. Likewise a renewal payment, renewal change
    /// or subscription end naming one of the hub's orders whose entitlement
    /// does not exist yet, while that order can still be paid (its paid
    /// checkout event has not arrived). The
    /// caller must roll back, so nothing is stored, and ask the provider to
    /// retry.
    NotReady,
    /// The event was stored and applied; its stored outcome.
    Recorded(Box<ApplyOutcome>),
}

/// Store `event` (with the SHA-256 of `raw_body`) and apply it, in the
/// caller's transaction:
///
/// 1. `INSERT … ON CONFLICT DO NOTHING` on `(provider, event id)`; a
///    conflict is [`Receipt::Duplicate`] and writes nothing more.
/// 2. [`orders::apply_event`].
/// 3. Set the stored `outcome`, once.
///
/// A completed event that matches no order, or a renewal payment, renewal
/// change or subscription end that matches no entitlement for an order that
/// can still be paid, but names an order reference is [`Receipt::NotReady`]
/// before step 3; the caller must roll the transaction back.
pub async fn receive(
    conn: &mut PgConnection,
    event: &VerifiedEvent,
    raw_body: &[u8],
) -> Result<Receipt, ApplyError> {
    let digest = hex::encode(Sha256::digest(raw_body));
    let stored = sqlx::query(
        "INSERT INTO payment_events (provider, event_id, event_type, session_id, payload_sha256) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(event.provider())
    .bind(event.id())
    .bind(event.event_type())
    .bind(event.session_id())
    .bind(&digest)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if stored == 0 {
        return Ok(Receipt::Duplicate);
    }
    let outcome = orders::apply_event(&mut *conn, event).await?;
    if outcome == ApplyOutcome::Unmatched && waits_for_its_order(&mut *conn, event).await? {
        return Ok(Receipt::NotReady);
    }
    sqlx::query(
        "UPDATE payment_events SET outcome = $3 \
         WHERE provider = $1 AND event_id = $2 AND outcome IS NULL",
    )
    .bind(event.provider())
    .bind(event.id())
    .bind(outcome.as_str())
    .execute(&mut *conn)
    .await?;
    Ok(Receipt::Recorded(Box::new(outcome)))
}

/// Whether an unmatched `event` is one of the hub's own that must wait for
/// its order or entitlement. It must name an order reference (Stripe's
/// `wipehub_order_id` metadata with this hub's origin), and be:
///
/// - a completed checkout (story 3's rule), or
/// - a renewal payment, renewal change or subscription end whose order can
///   still be paid (decision 7): no order with that id exists yet, or it is
///   `pending` and no stored payment event for its checkout session was a
///   `mismatch`. An order that expired, was refunded, or stays pending after
///   a mismatch never gets an entitlement, so its subscription's events are
///   stored `unmatched` instead of being retried until the provider gives up
///   (which can disable the webhook endpoint).
async fn waits_for_its_order(
    conn: &mut PgConnection,
    event: &VerifiedEvent,
) -> Result<bool, sqlx::Error> {
    let Some(order_id) = event.order_ref() else {
        return Ok(false);
    };
    match event.kind() {
        EventKind::CheckoutCompleted { .. } => Ok(true),
        EventKind::RenewalPaid { .. }
        | EventKind::RenewalChanged { .. }
        | EventKind::SubscriptionEnded { .. } => {
            // Any organization's order: the reference is the hub's own.
            let order: Option<(String, bool)> = sqlx::query_as(
                "SELECT o.status, EXISTS ( \
                     SELECT 1 FROM payment_events pe \
                     WHERE pe.provider = o.provider \
                       AND pe.session_id = o.provider_session_id \
                       AND pe.outcome = 'mismatch') \
                 FROM orders o WHERE o.id = $1",
            )
            .bind(order_id)
            .fetch_optional(&mut *conn)
            .await?;
            Ok(match order {
                None => true,
                Some((status, mismatched)) => status == "pending" && !mismatched,
            })
        }
        _ => Ok(false),
    }
}
