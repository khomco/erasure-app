//! A connected station tops up its credit reserve (ADR-0013 §3,
//! controlled-licensing story 4).
//!
//! - **When.** Only on a station with a saved hub address
//!   ([`licence_fetch::saved_hub`]), at least one vendor root, and an
//!   installed `Pool` licence, and only while the credits a start may still
//!   reserve for the licence's organization (`available`: held, and neither
//!   spent nor reserved by running Jobs) are below the threshold the hub
//!   last answered with. Until the hub has answered once, the station does
//!   not know its offering's threshold, so it asks once to learn it.
//!   Triggered by the licence-fetch loop after each check (start-up and
//!   every 24 h), and by a non-blocking [`nudge`] after every pool start
//!   decision, every pool spend, every refusal for lack of pool credits and
//!   every licence install. At most one top-up is in flight. After an
//!   attempt that left the reserve below the threshold (a failure, or an
//!   empty pool), nudges are ignored for [`TOPUP_RETRY_AFTER`]; the 24-hour
//!   loop still runs.
//! - **What.** `POST /api/station/credits` signed with the instance key
//!   ([`HubClient::top_up`]), reporting `available` as the balance, the
//!   cumulative credits spent and the last grant a top-up delivered, in the
//!   shared [`wipe_license::station_credits`] types. A grant in the answer
//!   must name the licence's organization and pass
//!   [`wipe_license::check_grant_for`] against [`AppState::vendor_roots`]
//!   and this station's key (the hub can never introduce a root, ADR-0008
//!   §4); then it joins the reserve and its id is saved as the last top-up
//!   grant (also when it was already held: a resend). A grant that fails is
//!   not added and the attempt fails. The hub's threshold and target are
//!   saved from every answer.
//! - **Status.** Each attempt's outcome is a [`HubCheck`] in
//!   [`CreditTopUpSlot`], shown on `GET /api/license` as `credit_top_up`. A
//!   failure is logged with `tracing::warn`.
//!
//! **A top-up never blocks a job.** It runs on its own task, holds no lock
//! across an `.await`, never touches `start_job` or the runner, and never
//! refuses a start: an unreachable hub only changes the status line while
//! credits remain.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use time::OffsetDateTime;
use wipe_cert::VerifyingKey;

use crate::app::AppState;
use wipe_license::station_credits::TopUpRequest;

use crate::hub_client::{HubClient, HubOrigin, ProxySettings, TopUpOutcome};
use crate::licence_fetch::{self, HubCheck};
use crate::licensing::PoolReserve;

/// How long nudges are ignored after a top-up left the reserve below its
/// threshold. Spares the hub's per-network challenge throttle and an empty
/// pool.
pub const TOPUP_RETRY_AFTER: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
struct Inner {
    last: Option<HubCheck>,
    in_flight: bool,
    /// Nudges are ignored until then.
    quiet_until: Option<Instant>,
}

/// What a top-up needs from the station's configuration.
#[derive(Default, Clone)]
struct Link {
    settings_path: Option<PathBuf>,
    vendor_roots: Arc<Vec<VerifyingKey>>,
}

/// The latest top-up outcome, and whether one is in flight.
///
/// Also keeps the settings path and vendor roots a top-up uses, shared by
/// every clone of the [`AppState`]: the certificate generator, which nudges
/// after a spend, holds a clone taken in [`AppState::new`], before a host
/// configures either.
#[derive(Default)]
pub struct CreditTopUpSlot {
    inner: Mutex<Inner>,
    link: Mutex<Link>,
}

impl CreditTopUpSlot {
    /// The latest top-up; `None` before any.
    pub fn last(&self) -> Option<HubCheck> {
        self.inner.lock().last.clone()
    }

    /// Whether a top-up is running now.
    pub fn in_flight(&self) -> bool {
        self.inner.lock().in_flight
    }

    /// Record the station's settings path and trusted vendor roots
    /// ([`AppState::with_settings_path`], [`AppState::with_vendor_roots`]).
    pub(crate) fn configure(
        &self,
        settings_path: Option<PathBuf>,
        vendor_roots: Arc<Vec<VerifyingKey>>,
    ) {
        *self.link.lock() = Link {
            settings_path,
            vendor_roots,
        };
    }

    fn link(&self) -> Link {
        self.link.lock().clone()
    }

    /// Claim the one in-flight top-up: false when one is running, or (for a
    /// nudge) while nudges are quiet.
    fn begin(&self, nudged: bool) -> bool {
        let mut inner = self.inner.lock();
        if inner.in_flight {
            return false;
        }
        if nudged && inner.quiet_until.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        inner.in_flight = true;
        true
    }

    /// End the claim; record `check` if the attempt ran, and quiet nudges
    /// when it left the reserve below its threshold.
    fn finish(&self, check: Option<HubCheck>, still_low: bool) {
        let mut inner = self.inner.lock();
        inner.in_flight = false;
        if let Some(check) = check {
            inner.last = Some(check);
        }
        inner.quiet_until = still_low.then(|| Instant::now() + TOPUP_RETRY_AFTER);
    }
}

/// Releases the in-flight claim even if the attempt's task is cancelled.
struct Claim<'a> {
    slot: &'a CreditTopUpSlot,
    done: bool,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.slot.finish(None, false);
        }
    }
}

/// Top up after a pool start, spend or refusal, or a licence install, on a
/// task of its own: returns at once. Does nothing while a top-up is in flight or nudges are quiet, or
/// outside a Tokio runtime.
pub fn nudge(state: &AppState) {
    if state.credit_topup.link().settings_path.is_none() {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if !state.credit_topup.begin(true) {
        return;
    }
    let state = state.clone();
    runtime.spawn(async move {
        attempt(&state, crate::hub_client::REQUEST_TIMEOUT).await;
    });
}

/// Top up now if it is due, unless one is in flight (nudge quiet periods do
/// not apply): the outcome when an attempt ran. Recorded in
/// [`AppState::credit_topup`].
pub async fn top_up_once(state: &AppState) -> Option<HubCheck> {
    top_up_once_with(state, crate::hub_client::REQUEST_TIMEOUT).await
}

/// As [`top_up_once`], with `request_timeout` for each hub request (tests
/// use a short one against a hub that never answers).
pub async fn top_up_once_with(state: &AppState, request_timeout: Duration) -> Option<HubCheck> {
    if !state.credit_topup.begin(false) {
        return None;
    }
    attempt(state, request_timeout).await
}

/// One attempt, the in-flight claim already held.
async fn attempt(state: &AppState, request_timeout: Duration) -> Option<HubCheck> {
    let mut claim = Claim {
        slot: &state.credit_topup,
        done: false,
    };
    let outcome = run(state, request_timeout).await;
    let still_low = outcome.as_ref().is_some_and(|(_, low)| *low);
    let check = outcome.map(|(check, _)| check);
    state.credit_topup.finish(check.clone(), still_low);
    claim.done = true;
    check
}

/// Whether a top-up is due for `reserve`: its terms are unknown (the hub
/// has never answered, so ask once to learn them), or what a start may
/// still reserve is below the threshold.
fn due(reserve: &PoolReserve) -> bool {
    match reserve.terms {
        Some(terms) => reserve.available < terms.threshold,
        None => true,
    }
}

/// The attempt: `None` when no top-up is due; else its outcome and whether
/// it left the reserve below its threshold (or the terms still unknown).
async fn run(state: &AppState, request_timeout: Duration) -> Option<(HubCheck, bool)> {
    let link = state.credit_topup.link();
    let origin = licence_fetch::saved_hub_at(link.settings_path.as_deref())?;
    if link.vendor_roots.is_empty() {
        return None;
    }
    // Cloned out of the cell: no lock is held across the request below.
    if !due(&state.license().pool_reserve()?) {
        return None;
    }
    let (ok, message) = match top_up(state, &link.vendor_roots, &origin, request_timeout).await {
        Ok(message) => (true, message),
        Err(message) => (false, message),
    };
    // Re-read: the answer may have added credits or taught the terms.
    let still_low = match state.license().pool_reserve() {
        Some(reserve) => due(&reserve),
        None => true,
    };
    if ok {
        tracing::info!(hub = %origin, %message, "credit top-up with the platform");
    } else {
        tracing::warn!(hub = %origin, %message, "credit top-up with the platform failed");
    }
    Some((
        HubCheck {
            checked_at: OffsetDateTime::now_utc(),
            ok,
            message,
        },
        still_low,
    ))
}

/// "This station has N credits available."
fn holds(n: u64) -> String {
    match n {
        1 => "This station has 1 credit available.".into(),
        n => format!("This station has {n} credits available."),
    }
}

/// One request: the success message, or why it failed.
async fn top_up(
    state: &AppState,
    vendor_roots: &[VerifyingKey],
    origin: &HubOrigin,
    request_timeout: Duration,
) -> Result<String, String> {
    let o = origin.as_str();
    let license = state.license();
    let Some(reserve) = license.pool_reserve() else {
        return Err("This station's licence no longer draws on a credit pool.".into());
    };
    let client =
        HubClient::with_timeout(origin.clone(), &ProxySettings::from_env(), request_timeout)
            .map_err(|e| {
                format!(
                    "The station could not set up its connection to the platform at {o} ({e}). {}",
                    holds(reserve.available)
                )
            })?;
    let report = TopUpRequest {
        // Filled in from the signing key by the client.
        instance_public_key: String::new(),
        licence_id: license.license_id().map(str::to_owned),
        balance: reserve.available,
        credits_spent: reserve.spent,
        last_grant_id: reserve.last_topup_grant_id.clone(),
    };
    let answer = match client.top_up(&state.signing_key, &report).await {
        TopUpOutcome::Answer(answer) => answer,
        TopUpOutcome::NotRegistered => {
            return Err(format!(
                "This station is not registered with the platform at {o}, so it could not top \
                 up its credits. {}",
                holds(reserve.available)
            ))
        }
        TopUpOutcome::Unreachable(why) => {
            return Err(format!(
                "Could not reach the platform at {o} to top up credits ({why}). {}",
                holds(reserve.available)
            ))
        }
        TopUpOutcome::Refused(why) => {
            return Err(format!(
                "The platform at {o} refused the credit top-up ({why}). {}",
                holds(reserve.available)
            ))
        }
    };
    let terms = wipe_license::CreditTerms {
        threshold: answer.threshold,
        target: answer.target_reserve,
    };
    // The terms are kept from every answer, whatever happens to its grant.
    license.record_top_up(terms, None);
    let available_now = || {
        license
            .pool_reserve()
            .map_or(reserve.available, |r| r.available)
    };
    let Some(grant) = answer.credit_grant else {
        let now = available_now();
        if now < answer.threshold && answer.available == 0 {
            return Err(format!(
                "The organization's credit pool on the platform at {o} is empty, so this station \
                 was not topped up. {} Add credits to the pool to keep starting jobs.",
                holds(now)
            ));
        }
        return Ok(format!(
            "No top-up needed: the platform at {o} sent no credits. {}",
            holds(now)
        ));
    };
    // A grant counts only for the licence's own organization: one for
    // another would sit unused while the station kept asking.
    if grant.body.organization_id != reserve.organization_id {
        return Err(format!(
            "The platform at {o} sent a credit grant for another organization ({}), not the \
             one this station's licence is issued to ({}), so it was not added. {}",
            grant.body.organization_id,
            reserve.organization_id,
            holds(available_now())
        ));
    }
    let intake = license
        .add_grant(&grant, &state.signing_key.public_key_id(), vendor_roots)
        .map_err(|p| {
            format!(
                "The platform at {o} sent a credit grant that was not added ({}). {}",
                p.grant_message(),
                holds(available_now())
            )
        })?;
    // Recorded even when already held: the hub resends its latest grant
    // until the station names it, so a station that took it but did not
    // save its id must name it now.
    license.record_top_up(terms, Some(grant.body.grant_id.as_str()));
    let now = available_now();
    let mut message = match intake.added {
        wipe_license::GrantAdded::Added => format!(
            "Topped up from the platform at {o}: received {} credits. {}",
            grant.body.credits,
            holds(now)
        ),
        wipe_license::GrantAdded::AlreadyHeld => format!(
            "The platform at {o} resent a credit grant this station already holds. {}",
            holds(now)
        ),
    };
    if answer.available == 0 {
        message.push_str(" The organization's credit pool is now empty.");
    }
    if let Err(fault) = intake.saved {
        message.push_str(&format!(
            " The credits are held in memory but were not saved ({fault})."
        ));
    }
    Ok(message)
}
