//! Offline enforcement levers and their honest limits (ADR-0005 §3).
//!
//! Nothing here can *prove* consumption — a station with no network can
//! always under-report. What it can do is refuse to sign under an expired
//! lease, notice a clock that has moved backwards, and say plainly which of
//! those guarantees is cryptographic and which is a speed bump.

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

use crate::entitlement::{Entitlements, Feature};
use crate::{LicenseError, LicenseResult};

/// How much backwards clock movement to tolerate before calling it rollback.
/// NTP steps, VM suspend/resume and timezone-adjacent bugs all produce small
/// negative jumps that are not attacks.
pub const CLOCK_SKEW_ALLOWANCE: Duration = Duration::minutes(5);

/// Persisted anti-rollback state.
///
/// Stored through the ADR-0003 tiered config store, so on a PXE station with
/// no writable storage it lives only for the boot — which is itself worth
/// surfacing rather than hiding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseState {
    /// Highest wall-clock time this station has ever observed. A clock
    /// earlier than this (beyond skew) means time moved backwards.
    #[serde(with = "time::serde::rfc3339")]
    pub time_watermark: OffsetDateTime,
    /// Erasures observed locally under the current license.
    pub erasures_used: u64,
    /// Set once the counter is anchored somewhere the customer cannot
    /// rewind. False for the file-backed counter.
    #[serde(default)]
    pub counter_is_monotonic: bool,
}

impl LeaseState {
    pub fn new(now: OffsetDateTime) -> Self {
        Self {
            time_watermark: now,
            erasures_used: 0,
            counter_is_monotonic: false,
        }
    }

    /// Fold an observed time into the watermark. Only ever moves forward.
    pub fn observe(&mut self, now: OffsetDateTime) {
        if now > self.time_watermark {
            self.time_watermark = now;
        }
    }

    /// Has the clock moved backwards past the skew allowance?
    pub fn rollback_detected(&self, now: OffsetDateTime) -> bool {
        now < self.time_watermark - CLOCK_SKEW_ALLOWANCE
    }
}

/// A monotonic counter the station cannot rewind.
///
/// `FileCounter` is honest about being rewindable; a TPM-backed
/// implementation lands with the Linux backend and is the only variant that
/// makes the anti-rollback claim real (ADR-0005 §3b).
pub trait MonotonicCounter: Send + Sync {
    fn read(&self) -> LicenseResult<u64>;
    fn increment(&self) -> LicenseResult<u64>;
    /// False means "this counter can be rolled back by the customer" and the
    /// UI is expected to say so rather than implying hardware backing.
    fn is_hardware_backed(&self) -> bool {
        false
    }
}

/// Placeholder for TPM 2.0 / secure-element counters.
///
/// Returns `Unsupported` rather than a plausible number: a counter that
/// silently pretends to be monotonic is worse than none, because the whole
/// point is the strength of the guarantee.
pub struct TpmCounter;

impl MonotonicCounter for TpmCounter {
    fn read(&self) -> LicenseResult<u64> {
        Err(LicenseError::Unsupported(
            "TPM monotonic counter needs the Linux backend (/dev/tpmrm0)".into(),
        ))
    }
    fn increment(&self) -> LicenseResult<u64> {
        self.read()
    }
    fn is_hardware_backed(&self) -> bool {
        true
    }
}

/// Why a licence check failed, in terms an operator can act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LeaseStatus {
    /// Licensed and within its window.
    Valid {
        #[serde(skip_serializing_if = "Option::is_none")]
        remaining_erasures: Option<u64>,
        days_remaining: i64,
    },
    /// Lease window has passed.
    Expired {
        #[serde(with = "time::serde::rfc3339")]
        not_after: OffsetDateTime,
    },
    /// Licence is not yet valid.
    NotYetValid {
        #[serde(with = "time::serde::rfc3339")]
        not_before: OffsetDateTime,
    },
    /// Quota exhausted by locally-observed usage. Best-effort offline.
    QuotaExhausted { used: u64, allowed: u64 },
    /// Licence is bound to a different machine.
    WrongMachine { expected: String, actual: String },
    /// Clock moved backwards past the skew allowance.
    ClockRollback {
        #[serde(with = "time::serde::rfc3339")]
        watermark: OffsetDateTime,
        #[serde(with = "time::serde::rfc3339")]
        observed: OffsetDateTime,
    },
    /// No licence at all — free tier (ADR-0005 §5).
    Unlicensed,
}

impl LeaseStatus {
    /// Does this permit signing a *licensed* certificate?
    ///
    /// Note `Unlicensed` is false here but does **not** stop the erasure: a
    /// free-tier station still erases and still emits a valid evaluation
    /// certificate. This asks only whether a vendor chain may be attached.
    pub fn permits_licensed_signing(&self) -> bool {
        matches!(self, Self::Valid { .. })
    }

    pub fn operator_message(&self) -> String {
        match self {
            Self::Valid {
                remaining_erasures,
                days_remaining,
            } => match remaining_erasures {
                Some(n) => format!("Licensed - {n} erasures remaining, {days_remaining} days left"),
                None => format!("Licensed - unlimited, {days_remaining} days left"),
            },
            Self::Expired { not_after } => format!(
                "Licence expired on {}. Certificates will be marked evaluation until it is renewed.",
                not_after.date()
            ),
            Self::NotYetValid { not_before } => {
                format!("Licence is not valid until {}.", not_before.date())
            }
            Self::QuotaExhausted { used, allowed } => format!(
                "Licence quota used ({used} of {allowed}). Erasure still works; certificates \
                 will be marked evaluation."
            ),
            Self::WrongMachine { expected, actual } => format!(
                "Licence is bound to machine {expected}, but this station is {actual}."
            ),
            Self::ClockRollback {
                watermark,
                observed,
            } => format!(
                "System clock reads {} but this station has previously seen {}. Time appears to \
                 have moved backwards; fix the clock to restore licensed signing.",
                observed.date(),
                watermark.date()
            ),
            Self::Unlicensed => {
                "No licence installed - running in evaluation mode. Erasure works normally and \
                 certificates are valid, but are marked as unlicensed."
                    .into()
            }
        }
    }
}

/// Evaluate a licence offline.
///
/// `fingerprint` is this station's machine fingerprint; `now` is the current
/// wall clock. `state` supplies the anti-rollback watermark and local usage
/// count and is **not** mutated here — callers decide when to record usage.
pub fn evaluate(
    entitlements: Option<&Entitlements>,
    state: &LeaseState,
    fingerprint: &str,
    now: OffsetDateTime,
) -> LeaseStatus {
    let Some(ent) = entitlements else {
        return LeaseStatus::Unlicensed;
    };

    // Rollback first: every check below reads the clock, so a moved clock
    // makes all of them untrustworthy.
    if state.rollback_detected(now) {
        return LeaseStatus::ClockRollback {
            watermark: state.time_watermark,
            observed: now,
        };
    }

    if let Some(expected) = ent.required_fingerprint() {
        if expected != fingerprint {
            return LeaseStatus::WrongMachine {
                expected: expected.to_string(),
                actual: fingerprint.to_string(),
            };
        }
    }

    if now < ent.not_before {
        return LeaseStatus::NotYetValid {
            not_before: ent.not_before,
        };
    }
    if now > ent.not_after {
        return LeaseStatus::Expired {
            not_after: ent.not_after,
        };
    }

    if !ent.quota.permits(state.erasures_used) {
        if let crate::entitlement::Quota::Count { erasures } = ent.quota {
            return LeaseStatus::QuotaExhausted {
                used: state.erasures_used,
                allowed: erasures,
            };
        }
    }

    LeaseStatus::Valid {
        remaining_erasures: ent.quota.remaining(state.erasures_used),
        days_remaining: (ent.not_after - now).whole_days(),
    }
}

/// Is a feature available under these entitlements right now?
///
/// A feature needs both the grant and a currently-valid lease — an expired
/// licence should not keep unlocking enterprise mode.
pub fn feature_available(
    entitlements: Option<&Entitlements>,
    status: &LeaseStatus,
    feature: Feature,
) -> bool {
    status.permits_licensed_signing() && entitlements.map(|e| e.grants(feature)).unwrap_or(false)
}
