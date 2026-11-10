//! Enrollment from the bench app (ADR-0008, station-enrollment story 4).
//!
//! The station routes `GET /api/enrollment`, `POST /api/enrollment` and
//! `POST /api/enrollment/cancel` run one enrollment inside the running
//! station, through the same [`hub_client`](crate::hub_client) as
//! `wipestation enroll`:
//!
//! - A start is refused before any hub request when the hub address breaks
//!   [`HubOrigin::parse`]'s rule, when the station trusts no vendor root,
//!   when its instance key was generated for this run only (no key file, so
//!   a licence bound to it would be lost at restart, with the seat it spent),
//!   or when the label is over [`MAX_LABEL_CHARS`]. Otherwise the route sends
//!   the hub start itself and answers with the attempt `pending`, or already
//!   ended (`unreachable`, `refused`): hub outcomes are view states, not HTTP
//!   errors. A start answer whose approval links are not absolute `http` or
//!   `https` URLs ends `refused`: the station UI renders them as a link and a
//!   QR code, and the hub must never be able to act in it.
//! - Every ended attempt carries a message worded for the bench app: what
//!   happened, whether anything was saved or installed, and the hub's or
//!   verifier's reason, but no `wipestation` command (the panel offers Start
//!   again). The CLI keeps [`EnrollError`]'s own text.
//! - One spawned task per attempt polls the hub. On a verified licence it
//!   saves the licence to [`ENROLLED_LICENCE_FILE`](crate::store::ENROLLED_LICENCE_FILE) and the hub address to the
//!   station settings (when the station keeps configuration on disk), then
//!   installs the licence in process through [`AppState::install_license`],
//!   which holds it until restart. Nothing loads the saved file at start-up.
//! - A new start replaces (aborts) the pending attempt, as the hub replaces the
//!   key's pending enrollment; cancel stops polling and the hub's code simply
//!   expires.
//!
//! The licence is checked against [`AppState::vendor_roots`] only: the hub
//! can never introduce a root (ADR-0008 §4). The device code stays inside
//! this module and never appears in a view. Nothing here gates, delays or
//! interrupts an erasure.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::task::AbortHandle;

use crate::app::AppState;
use crate::hub_client::{
    poll_until_done, EnrollError, Enrolled, Enrollment, HubClient, HubOrigin, ProxySettings,
    StartResponse, SystemClock, MAX_EXPIRES_IN_SECS,
};
use crate::store::{
    enrolled_licence_path, load_station_settings, save_enrolled_licence, save_station_settings,
};

/// The longest label a start accepts, in characters.
pub const MAX_LABEL_CHARS: usize = 200;

/// Why the station cannot enroll: it trusts no vendor root.
pub const NO_VENDOR_ROOT_REASON: &str = "This station trusts no vendor root, so it cannot \
    verify a licence from the platform. Restart it with --vendor-root-b64 to register it with \
    the platform.";

/// Why the station cannot enroll: its instance key lasts for this run only.
pub const KEY_NOT_KEPT_REASON: &str = "This station's instance key was generated for this run \
    only, so a licence bound to it would be lost when the station restarts, together with the \
    seat it used. Restart the station with --key-path naming a key file to register it with the \
    platform.";

/// `GET /api/enrollment`: the bench app's Register with platform panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnrollmentView {
    /// The hub address in the station settings, when it passes the hub
    /// address rule; `null` otherwise or when the station keeps nothing on
    /// disk.
    pub saved_hub_origin: Option<String>,
    /// Whether a start would be accepted: the station trusts a vendor root
    /// and keeps its instance key in a key file.
    pub can_enroll: bool,
    /// Why not, when `can_enroll` is false.
    pub cannot_enroll_reason: Option<String>,
    /// The current or last attempt; `null` before any, or after cancel.
    pub attempt: Option<EnrollmentAttempt>,
}

/// One enrollment attempt, tagged on `state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EnrollmentAttempt {
    /// Waiting for an owner to approve the code.
    Pending {
        hub_origin: String,
        user_code: String,
        verification_uri: String,
        verification_uri_complete: String,
        /// Seconds until the code expires, from a monotonic clock, floored
        /// at 0.
        seconds_left: u64,
        /// Seconds between the station's polls.
        interval: u64,
    },
    /// Approved, verified and installed.
    Installed {
        hub_origin: String,
        licence_id: String,
        customer_name: String,
        #[serde(with = "time::serde::rfc3339")]
        not_after: OffsetDateTime,
        /// The station's licence status message after the install.
        operator_message: String,
        /// Where the licence was saved; `null` when it was not.
        saved_to: Option<String>,
        /// What happens at restart, and how to keep the licence.
        message: String,
    },
    /// An owner denied it.
    Denied { hub_origin: String, message: String },
    /// The code expired before approval.
    Expired { hub_origin: String, message: String },
    /// The hub could not be reached to start.
    Unreachable { hub_origin: String, message: String },
    /// The hub refused, or sent a licence this station does not accept.
    Refused { hub_origin: String, message: String },
    /// Anything else: the licence could not be installed, or the connection
    /// could not be set up.
    Failed { hub_origin: String, message: String },
}

/// `POST /api/enrollment`'s body.
#[derive(Debug, Clone, Deserialize)]
pub struct StartEnrollment {
    pub hub_origin: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// Why a start was refused before any hub request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartRefusal {
    /// The hub address breaks the rule; the text states it.
    #[error("{0}")]
    BadOrigin(String),
    /// The station trusts no vendor root.
    #[error("{NO_VENDOR_ROOT_REASON}")]
    NoVendorRoot,
    /// The instance key was generated for this run only.
    #[error("{KEY_NOT_KEPT_REASON}")]
    KeyNotKept,
    /// The label is over [`MAX_LABEL_CHARS`].
    #[error("the label is {0} characters long; at most {MAX_LABEL_CHARS} are allowed")]
    LabelTooLong(usize),
}

/// The attempt as held: a pending one keeps its expiry instant, so
/// `seconds_left` is computed when read.
#[derive(Debug, Clone)]
enum Record {
    Pending {
        hub_origin: String,
        user_code: String,
        verification_uri: String,
        verification_uri_complete: String,
        interval: u64,
        expires_at: Instant,
    },
    Ended(EnrollmentAttempt),
}

impl Record {
    fn view(&self, now: Instant) -> EnrollmentAttempt {
        match self {
            Record::Pending {
                hub_origin,
                user_code,
                verification_uri,
                verification_uri_complete,
                interval,
                expires_at,
            } => EnrollmentAttempt::Pending {
                hub_origin: hub_origin.clone(),
                user_code: user_code.clone(),
                verification_uri: verification_uri.clone(),
                verification_uri_complete: verification_uri_complete.clone(),
                seconds_left: expires_at.saturating_duration_since(now).as_secs(),
                interval: *interval,
            },
            Record::Ended(attempt) => attempt.clone(),
        }
    }
}

#[derive(Default)]
struct Slot {
    /// Bumped by every start and cancel, so a superseded start or task never
    /// writes over a newer attempt.
    generation: u64,
    record: Option<Record>,
    task: Option<AbortHandle>,
}

/// The station's one enrollment attempt and its polling task.
#[derive(Default)]
pub struct EnrollmentSlot(Mutex<Slot>);

impl EnrollmentSlot {
    /// Abort any task, clear the attempt, and claim a new generation.
    fn reset(&self) -> u64 {
        let mut slot = self.0.lock();
        slot.generation += 1;
        if let Some(task) = slot.task.take() {
            task.abort();
        }
        slot.record = None;
        slot.generation
    }

    /// Record `attempt` as the outcome of `generation`, unless superseded.
    fn end(&self, generation: u64, attempt: EnrollmentAttempt) {
        let mut slot = self.0.lock();
        if slot.generation == generation {
            slot.record = Some(Record::Ended(attempt));
            slot.task = None;
        }
    }

    fn attempt(&self) -> Option<EnrollmentAttempt> {
        self.0
            .lock()
            .record
            .as_ref()
            .map(|r| r.view(Instant::now()))
    }
}

impl Drop for EnrollmentSlot {
    fn drop(&mut self) {
        if let Some(task) = self.0.get_mut().task.take() {
            task.abort();
        }
    }
}

/// The panel as it stands now.
pub fn view(state: &AppState) -> EnrollmentView {
    let refusal = cannot_enroll(state);
    EnrollmentView {
        saved_hub_origin: saved_hub_origin(state.settings_path.as_deref()),
        can_enroll: refusal.is_none(),
        cannot_enroll_reason: refusal.map(|r| r.to_string()),
        attempt: state.enrollment.attempt(),
    }
}

/// Why this station cannot enroll at all, if it cannot.
fn cannot_enroll(state: &AppState) -> Option<StartRefusal> {
    if state.vendor_roots.is_empty() {
        Some(StartRefusal::NoVendorRoot)
    } else if !state.instance_key_kept {
        Some(StartRefusal::KeyNotKept)
    } else {
        None
    }
}

/// The settings file's hub address, if it parses.
fn saved_hub_origin(settings_path: Option<&std::path::Path>) -> Option<String> {
    let settings = load_station_settings(settings_path?).ok()??;
    HubOrigin::parse(settings.hub_origin.as_deref()?)
        .ok()
        .map(|o| o.to_string())
}

/// Start an enrollment, replacing any pending one. Refused before any hub
/// request for a bad address, no vendor root, a run-only instance key or an
/// over-long label; else the
/// view with the attempt pending, or ended if the hub could not be reached
/// or refused.
pub async fn start(state: &AppState, req: StartEnrollment) -> Result<EnrollmentView, StartRefusal> {
    let origin =
        HubOrigin::parse(&req.hub_origin).map_err(|e| StartRefusal::BadOrigin(e.to_string()))?;
    if let Some(refusal) = cannot_enroll(state) {
        return Err(refusal);
    }
    let label = req
        .label
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty());
    if let Some(l) = &label {
        let n = l.chars().count();
        if n > MAX_LABEL_CHARS {
            return Err(StartRefusal::LabelTooLong(n));
        }
    }

    let generation = state.enrollment.reset();
    let hub_origin = origin.to_string();
    let client = match HubClient::new(origin, &ProxySettings::from_env()) {
        Ok(c) => c,
        Err(e) => {
            state.enrollment.end(generation, ended(hub_origin, e));
            return Ok(view(state));
        }
    };
    match client
        .start(&state.signing_key, &state.station_id, label.as_deref())
        .await
    {
        Err(e) => state.enrollment.end(generation, ended(hub_origin, e)),
        Ok(started) if bad_link(&started).is_some() => {
            let (which, value) = bad_link(&started).unwrap_or_default();
            tracing::warn!(%hub_origin, which, value, "hub sent an approval link that is not http(s)");
            state.enrollment.end(
                generation,
                EnrollmentAttempt::Refused {
                    message: format!(
                        "The platform at {hub_origin} sent an approval link this station will not \
                         show (its {which} is not an http or https address), so no code is shown \
                         and nothing was saved or installed. Check that the address is the \
                         platform's."
                    ),
                    hub_origin,
                },
            );
        }
        Ok(started) => {
            let mut slot = state.enrollment.0.lock();
            if slot.generation == generation {
                slot.record = Some(Record::Pending {
                    hub_origin: hub_origin.clone(),
                    user_code: started.user_code.clone(),
                    verification_uri: started.verification_uri.clone(),
                    verification_uri_complete: started.verification_uri_complete.clone(),
                    interval: started.interval,
                    // Capped as the poll loop caps it, so a hub's absurd
                    // `expires_in` cannot overflow the clock.
                    expires_at: Instant::now()
                        + Duration::from_secs(started.expires_in.min(MAX_EXPIRES_IN_SECS)),
                });
                let task = tokio::spawn(poll(
                    state.clone(),
                    client,
                    started,
                    label,
                    hub_origin,
                    generation,
                ));
                slot.task = Some(task.abort_handle());
            }
        }
    }
    Ok(view(state))
}

/// The first approval link in `started` that is not an absolute `http` or
/// `https` URL: which field, and its value.
fn bad_link(started: &StartResponse) -> Option<(&'static str, String)> {
    [
        ("verification_uri", &started.verification_uri),
        (
            "verification_uri_complete",
            &started.verification_uri_complete,
        ),
    ]
    .into_iter()
    .find(|(_, link)| !is_web_link(link))
    .map(|(which, link)| (which, link.clone()))
}

/// An absolute `http`/`https` URL with a host.
fn is_web_link(link: &str) -> bool {
    url::Url::parse(link)
        .map(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some())
        .unwrap_or(false)
}

/// Stop any attempt; the view with none.
pub fn cancel(state: &AppState) -> EnrollmentView {
    state.enrollment.reset();
    view(state)
}

/// The attempt's task: poll until it ends, then save and install.
async fn poll(
    state: AppState,
    client: HubClient,
    started: StartResponse,
    label: Option<String>,
    hub_origin: String,
    generation: u64,
) {
    let who = Enrollment {
        key: &state.signing_key,
        station_id: &state.station_id,
        label: label.as_deref(),
        roots: &state.vendor_roots,
    };
    let outcome = match poll_until_done(&client, &who, &started, &mut SystemClock::default()).await
    {
        Ok(enrolled) => install(&state, hub_origin, enrolled),
        Err(e) => ended(hub_origin, e),
    };
    state.enrollment.end(generation, outcome);
}

/// Save the licence and the hub address (when the station keeps
/// configuration on disk), then install the licence in process.
fn install(state: &AppState, hub_origin: String, enrolled: Enrolled) -> EnrollmentAttempt {
    let (saved_to, not_saved): (Option<PathBuf>, Option<String>) = match &state.settings_path {
        None => (
            None,
            Some("this station keeps no configuration on disk".to_string()),
        ),
        Some(settings) => {
            let path = enrolled_licence_path(settings);
            match save_enrolled_licence(&path, &enrolled.licence_json) {
                Ok(()) => (Some(path), None),
                Err(e) => (None, Some(e.to_string())),
            }
        }
    };
    let hub_not_saved = state.settings_path.as_deref().and_then(|path| {
        let mut settings = load_station_settings(path)
            .ok()
            .flatten()
            .unwrap_or_default();
        settings.hub_origin = Some(hub_origin.clone());
        save_station_settings(path, &settings)
            .err()
            .map(|e| format!("{} could not be written ({e})", path.display()))
    });

    let licence_view = match state.install_license(&enrolled.licence_json) {
        Ok(v) => v,
        Err(e) => {
            return EnrollmentAttempt::Failed {
                hub_origin,
                message: format!(
                    "The platform approved this station, but its licence could not be installed: \
                     {e}"
                ),
            }
        }
    };
    let mut message = match (&saved_to, &not_saved) {
        (Some(path), _) => format!(
            "This station holds the licence until it restarts. It is saved at {}; to keep it \
             after a restart, start the station with --license {}.",
            path.display(),
            path.display()
        ),
        (None, reason) => format!(
            "This station holds the licence until it restarts. It could not be saved ({}), so \
             after a restart the station runs without it unless it is given the licence again: \
             download it from the platform's portal.",
            reason.as_deref().unwrap_or("no reason given")
        ),
    };
    if let Some(why) = hub_not_saved {
        message.push_str(&format!(" The platform address was not saved: {why}."));
    }
    let body = &enrolled.licence.body;
    tracing::info!(
        licence_id = %body.license_id, %hub_origin,
        saved_to = ?saved_to, "enrolled with the platform from the bench app"
    );
    EnrollmentAttempt::Installed {
        hub_origin,
        licence_id: body.license_id.clone(),
        customer_name: body.entitlements.customer_name.clone(),
        not_after: body.entitlements.not_after,
        operator_message: licence_view.operator_message,
        saved_to: saved_to.map(|p| p.display().to_string()),
        message,
    }
}

/// The ended attempt for `e`, with a message worded for the bench app: what
/// happened, that nothing was saved or installed, and the hub's or the
/// verifier's reason. No `wipestation` command: the panel offers Start again.
fn ended(hub_origin: String, e: EnrollError) -> EnrollmentAttempt {
    const NOTHING: &str = "Nothing was saved or installed.";
    let o = &hub_origin;
    match e {
        EnrollError::Client(reason) => EnrollmentAttempt::Failed {
            message: format!(
                "The station could not set up its connection to the platform at {o} ({reason}). Check \
                 the station's HTTPS_PROXY setting. {NOTHING}"
            ),
            hub_origin,
        },
        EnrollError::HubUnreachable { reason, .. } => EnrollmentAttempt::Unreachable {
            message: format!(
                "Could not reach the platform at {o} ({reason}). Check the address, and that this \
                 station can reach it, directly or through its HTTPS_PROXY and NO_PROXY settings. \
                 {NOTHING}"
            ),
            hub_origin,
        },
        EnrollError::StartRefused {
            status,
            message,
            retry_after_secs,
            ..
        } => EnrollmentAttempt::Refused {
            message: format!(
                "The platform at {o} refused to start a registration (HTTP {status}: {message}). \
                 {NOTHING}{}",
                match retry_after_secs {
                    Some(s) => format!(" Try again in {s} seconds."),
                    None => String::new(),
                }
            ),
            hub_origin,
        },
        EnrollError::BadResponse { reason, .. } => EnrollmentAttempt::Refused {
            message: format!(
                "The platform at {o} sent an answer this station does not understand ({reason}). \
                 {NOTHING}"
            ),
            hub_origin,
        },
        EnrollError::Denied { .. } => EnrollmentAttempt::Denied {
            message: format!("An owner denied this registration at {o}. {NOTHING}"),
            hub_origin,
        },
        EnrollError::Expired => EnrollmentAttempt::Expired {
            message: format!("The code expired before an owner approved it. {NOTHING}"),
            hub_origin,
        },
        EnrollError::PollRefused { reason, .. } => EnrollmentAttempt::Refused {
            message: format!("The platform at {o} refused this registration ({reason}). {NOTHING}"),
            hub_origin,
        },
        EnrollError::UntrustedLicence { reason, .. } => EnrollmentAttempt::Refused {
            message: format!(
                "The platform at {o} sent a licence this station does not trust ({reason}). The \
                 station trusts only the vendor roots it was started with (--vendor-root-b64). \
                 {NOTHING}"
            ),
            hub_origin,
        },
        EnrollError::WrongKey {
            expected, found, ..
        } => EnrollmentAttempt::Refused {
            message: format!(
                "The platform at {o} sent a licence for another station: it names instance key \
                 {found}, but this station's key is {expected}. {NOTHING}"
            ),
            hub_origin,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_left_counts_down_and_floors_at_zero() {
        let now = Instant::now();
        let record = |expires_at| Record::Pending {
            hub_origin: "https://hub.example".into(),
            user_code: "BCDF-GHJK".into(),
            verification_uri: "https://hub.example/activate".into(),
            verification_uri_complete: "https://hub.example/activate?code=BCDF-GHJK".into(),
            interval: 5,
            expires_at,
        };
        let left = |r: &Record| match r.view(now) {
            EnrollmentAttempt::Pending { seconds_left, .. } => seconds_left,
            other => panic!("{other:?}"),
        };
        assert_eq!(left(&record(now + Duration::from_millis(90_500))), 90);
        assert_eq!(left(&record(now)), 0);
        // An expiry already past (the clock moved on) is 0, never an error.
        let past = record(now);
        let later = now + Duration::from_secs(30);
        match past.view(later) {
            EnrollmentAttempt::Pending { seconds_left, .. } => assert_eq!(seconds_left, 0),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_pending_view_never_carries_the_device_code() {
        let json = serde_json::to_value(EnrollmentAttempt::Pending {
            hub_origin: "h".into(),
            user_code: "u".into(),
            verification_uri: "v".into(),
            verification_uri_complete: "c".into(),
            seconds_left: 1,
            interval: 5,
        })
        .unwrap();
        assert_eq!(json["state"], "pending");
        assert!(json.get("device_code").is_none());
    }

    #[test]
    fn every_error_maps_to_its_state_with_bench_wording() {
        let o = || "https://hub.example".to_string();
        // Exhaustive: a new `EnrollError` variant fails to compile in
        // `ended` and must be added here.
        let cases = [
            (
                EnrollError::Client("bad proxy".into()),
                "failed",
                "bad proxy",
            ),
            (
                EnrollError::HubUnreachable {
                    origin: o(),
                    reason: "connection refused".into(),
                },
                "unreachable",
                "connection refused",
            ),
            (
                EnrollError::StartRefused {
                    origin: o(),
                    status: 429,
                    message: "too many enrollments".into(),
                    retry_after_secs: Some(30),
                },
                "refused",
                "Try again in 30 seconds",
            ),
            (
                EnrollError::BadResponse {
                    origin: o(),
                    reason: "not JSON".into(),
                },
                "refused",
                "not JSON",
            ),
            (EnrollError::Denied { origin: o() }, "denied", "denied"),
            (EnrollError::Expired, "expired", "expired"),
            (
                EnrollError::PollRefused {
                    origin: o(),
                    reason: "invalid_grant".into(),
                },
                "refused",
                "invalid_grant",
            ),
            (
                EnrollError::UntrustedLicence {
                    origin: o(),
                    reason: "bad signature".into(),
                },
                "refused",
                "does not trust",
            ),
            (
                EnrollError::WrongKey {
                    origin: o(),
                    expected: "key-a".into(),
                    found: "key-b".into(),
                },
                "refused",
                "key-b",
            ),
        ];
        for (e, want_state, want_text) in cases {
            let json = serde_json::to_value(ended(o(), e)).unwrap();
            assert_eq!(json["state"], want_state, "{json}");
            assert_eq!(json["hub_origin"], o(), "{json}");
            let message = json["message"].as_str().unwrap();
            assert!(message.contains(want_text), "{message}");
            assert!(!message.contains("wipestation"), "{message}");
            assert!(
                message.contains("Nothing was saved or installed"),
                "{message}"
            );
        }
    }

    #[test]
    fn only_absolute_web_links_are_shown() {
        assert!(is_web_link("https://hub.example/activate?code=BCDF-GHJK"));
        assert!(is_web_link("http://127.0.0.1:7900/activate"));
        for bad in [
            "javascript:alert(1)",
            "data:text/html,hi",
            "/activate",
            "hub.example/activate",
            "file:///etc/passwd",
            "",
        ] {
            assert!(!is_web_link(bad), "{bad}");
        }
    }
}
