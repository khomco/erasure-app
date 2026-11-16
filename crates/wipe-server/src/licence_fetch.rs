//! A connected station fetches its own renewed licence (ADR-0008 §3,
//! station-enrollment story 5).
//!
//! - **When.** `build_station` spawns [`run_loop`] for a station that keeps
//!   its configuration on disk: one check at start, then one every
//!   [`CHECK_INTERVAL`]. A check runs only while `station-settings.json`
//!   holds a hub address that passes [`HubOrigin::parse`]'s rule (read each
//!   time, so a station that enrolls from the bench app is checked from the
//!   next round on). No address, no check and no status. A failed check is
//!   not retried sooner: renewals arrive 30 days early (ADR-0013 §4).
//! - **What.** [`check_once`] asks the hub for the station's latest licence,
//!   saying which one it holds ([`HubClient::fetch_licence`], signed with the
//!   instance key). A licence it gets is checked against
//!   [`AppState::vendor_roots`] and this station's key only
//!   ([`check_licence`]: the hub can never introduce a root, ADR-0008 §4)
//!   and, only when it ends strictly later than the licence installed when
//!   it arrives (or none is installed), installed through
//!   [`AppState::install_license`] and, when the station keeps configuration
//!   on disk, saved to `enrolled-licence.json` as an enrollment saves it.
//!   Nothing loads that file at start-up.
//! - **Status.** Every check's outcome is held in [`LicenceFetchSlot`] as a
//!   [`HubCheck`] and shown on `GET /api/license` as `hub_check`. A failure
//!   is logged with `tracing::warn`, and its message says the current
//!   licence still applies.
//!
//! After each check the loop tops up the credit reserve when it is due
//! ([`crate::credit_topup`]).
//!
//! **The check never blocks a job.** It runs on its own task, holds no lock
//! across an `.await`, and never touches `start_job` or the runner: an
//! unreachable or hanging hub changes nothing but the status line.

use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use time::OffsetDateTime;
use tokio::task::AbortHandle;

use crate::app::AppState;
use crate::hub_client::{
    check_licence, Clock, EnrollError, FetchOutcome, HubClient, HubOrigin, ProxySettings,
    SystemClock, REQUEST_TIMEOUT,
};
use crate::store::{enrolled_licence_path, load_station_settings, save_enrolled_licence};

/// Time between two checks while the station runs.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Ends every failed check's message.
pub const STILL_APPLIES: &str = "This station's current licence still applies.";

/// The latest check with the platform, as `GET /api/license` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HubCheck {
    #[serde(with = "time::serde::rfc3339")]
    pub checked_at: OffsetDateTime,
    /// Whether the check reached the platform and left the station with its
    /// latest licence.
    pub ok: bool,
    /// What happened, worded for the bench.
    pub message: String,
}

#[derive(Default)]
struct Inner {
    last: Option<HubCheck>,
    task: Option<AbortHandle>,
}

/// The latest [`HubCheck`] and the loop's task. The task holds a clone of
/// the [`AppState`], and so this slot, so it runs until the runtime shuts
/// down: a task this slot replaces is aborted, and dropping the slot aborts
/// its task only once nothing else (that task included) holds the slot.
#[derive(Default)]
pub struct LicenceFetchSlot(Mutex<Inner>);

impl LicenceFetchSlot {
    /// The latest check; `None` before any.
    pub fn last(&self) -> Option<HubCheck> {
        self.0.lock().last.clone()
    }

    fn record(&self, check: HubCheck) {
        self.0.lock().last = Some(check);
    }

    fn set_task(&self, task: AbortHandle) {
        if let Some(old) = self.0.lock().task.replace(task) {
            old.abort();
        }
    }
}

impl Drop for LicenceFetchSlot {
    fn drop(&mut self) {
        if let Some(task) = self.0.get_mut().task.take() {
            task.abort();
        }
    }
}

/// The hub address saved in the station settings, if it passes the rule.
pub fn saved_hub(state: &AppState) -> Option<HubOrigin> {
    saved_hub_at(state.settings_path.as_deref())
}

/// The hub address saved in the station settings at `settings_path`, if it
/// passes the rule.
pub fn saved_hub_at(settings_path: Option<&std::path::Path>) -> Option<HubOrigin> {
    let settings = load_station_settings(settings_path?).ok()??;
    HubOrigin::parse(settings.hub_origin.as_deref()?).ok()
}

/// Check once with the platform at the saved hub address, record the
/// outcome and return it; `None`, recording nothing, when no address is
/// saved. See the module docs.
pub async fn check_once(state: &AppState) -> Option<HubCheck> {
    check_once_with(state, REQUEST_TIMEOUT).await
}

/// As [`check_once`], with `request_timeout` for each hub request (tests
/// use a short one against a hub that never answers).
pub async fn check_once_with(state: &AppState, request_timeout: Duration) -> Option<HubCheck> {
    let origin = saved_hub(state)?;
    let (ok, message) = match check(state, &origin, request_timeout).await {
        Ok(message) => (true, message),
        Err(why) => (false, format!("{why} {STILL_APPLIES}")),
    };
    if ok {
        tracing::info!(hub = %origin, %message, "licence check with the platform");
    } else {
        tracing::warn!(hub = %origin, %message, "licence check with the platform failed");
    }
    let check = HubCheck {
        checked_at: OffsetDateTime::now_utc(),
        ok,
        message,
    };
    state.licence_fetch.record(check.clone());
    Some(check)
}

/// One check: the success message, or why it failed.
async fn check(
    state: &AppState,
    origin: &HubOrigin,
    request_timeout: Duration,
) -> Result<String, String> {
    let o = origin.as_str();
    if state.vendor_roots.is_empty() {
        return Err(format!(
            "This station trusts no vendor root, so it cannot verify a licence from the \
             platform at {o}."
        ));
    }
    // Cloned out of the cell: no lock is held across the requests below.
    let held = state.license().license_id().map(str::to_string);
    let client =
        HubClient::with_timeout(origin.clone(), &ProxySettings::from_env(), request_timeout)
            .map_err(|e| {
                format!("The station could not set up its connection to the platform at {o} ({e}).")
            })?;
    let up_to_date =
        || format!("Up to date: the platform at {o} has no newer licence for this station.");
    let fetched = match client
        .fetch_licence(&state.signing_key, held.as_deref())
        .await
    {
        FetchOutcome::Current => return Ok(up_to_date()),
        FetchOutcome::Licence(licence) => licence,
        FetchOutcome::NotRegistered => {
            return Err(format!(
                "This station is not registered with the platform at {o}."
            ))
        }
        FetchOutcome::Unreachable(reason) => {
            return Err(format!("Could not reach the platform at {o} ({reason})."))
        }
        FetchOutcome::Refused(reason) => {
            return Err(format!(
                "The platform at {o} refused the licence check ({reason})."
            ))
        }
    };
    let (licence, json) = check_licence(
        o,
        &fetched,
        &state.signing_key.public_key_id(),
        &state.vendor_roots,
    )
    .map_err(|e| not_installed(o, e))?;
    // Only a newer licence (ADR-0008 §3). Re-read now, with no `.await`
    // before the install: an operator may have installed a longer licence
    // while the requests above were in flight.
    let installed_until = state.license().entitlements().map(|e| e.not_after);
    if let Some(until) = installed_until {
        if licence.body.entitlements.not_after <= until {
            return Ok(format!(
                "Up to date: the platform at {o} offered no licence newer than this station's, \
                 which runs until {} UTC, so nothing was installed.",
                until.date()
            ));
        }
    }
    state.install_license(&json).map_err(|e| {
        format!("The platform at {o} sent a licence that could not be installed ({e}).")
    })?;
    let mut message = format!(
        "Installed a new licence from the platform at {o}, valid until {} UTC.",
        licence.body.entitlements.not_after.date()
    );
    if let Some(settings) = &state.settings_path {
        let path = enrolled_licence_path(settings);
        match save_enrolled_licence(&path, &json) {
            Ok(()) => message.push_str(&format!(
                " It is saved at {}; to keep using it after a restart, start the station with \
                 --license {}.",
                path.display(),
                path.display()
            )),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "fetched licence not saved");
                message.push_str(&format!(
                    " It could not be saved ({e}), so after a restart this station goes back to \
                     the licence it was started with."
                ));
            }
        }
    }
    Ok(message)
}

/// Why a fetched licence was not installed, worded for the bench.
fn not_installed(o: &str, e: EnrollError) -> String {
    match e {
        EnrollError::UntrustedLicence { reason, .. } => format!(
            "The platform at {o} sent a licence this station does not trust ({reason}), so it was \
             not installed. The station trusts only the vendor roots it was started with."
        ),
        EnrollError::WrongKey {
            expected, found, ..
        } => format!(
            "The platform at {o} sent a licence for another station (instance key {found}, but \
             this station's key is {expected}), so it was not installed."
        ),
        other => {
            format!("The platform at {o} sent a licence this station could not read ({other}).")
        }
    }
}

/// Check now, then every [`CHECK_INTERVAL`] by `clock`, for as long as the
/// task runs. Each check is followed by a credit top-up when one is due
/// ([`crate::credit_topup::top_up_once`]).
pub async fn run_loop(state: AppState, clock: &mut dyn Clock) {
    loop {
        check_once(&state).await;
        crate::credit_topup::top_up_once(&state).await;
        clock.sleep(CHECK_INTERVAL).await;
    }
}

/// Spawn [`run_loop`] on the current Tokio runtime, its handle kept in
/// [`AppState::licence_fetch`]. The loop holds a clone of `state`, so it runs
/// until the runtime shuts down (a later spawn aborts it). Nothing is
/// spawned for a station that keeps no configuration on disk (it can hold
/// no hub address), or outside a runtime. Whether it was spawned.
pub fn spawn(state: &AppState) -> bool {
    if state.settings_path.is_none() {
        return false;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return false;
    };
    let looped = state.clone();
    let task = runtime.spawn(async move {
        run_loop(looped, &mut SystemClock::default()).await;
    });
    state.licence_fetch.set_task(task.abort_handle());
    true
}
