//! Offline enforcement levers and their honest limits (ADR-0005 §3).
//!
//! Nothing here can *prove* consumption — a station with no network can
//! always under-report. What it can do is refuse to sign under an expired
//! lease, notice a clock that has moved backwards, and say plainly which of
//! those guarantees is cryptographic and which is a speed bump.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

use crate::entitlement::{Entitlements, Feature};
use crate::{LicenseError, LicenseResult};

/// How much backwards clock movement to tolerate before calling it rollback.
/// NTP steps, VM suspend/resume and timezone-adjacent bugs all produce small
/// negative jumps that are not attacks.
pub const CLOCK_SKEW_ALLOWANCE: Duration = Duration::minutes(5);

/// Anti-rollback state: the clock watermark and the local erasure count.
///
/// This type is only the data. Whether it survives a restart depends on the
/// host: `wipestation serve` keeps it in its own file (`lease-state.json`) in
/// the directory the ADR-0003 tier detection resolves, and holds it in RAM
/// only when that directory is not writable — which it says at start-up
/// rather than hiding. A host that wires no store (the desktop shell, tests)
/// holds it in RAM for the life of the process.
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
    /// The licence `erasures_used` counts against. A station that starts
    /// under a different licence keeps the watermark (a fact about the
    /// clock) and resets the count. `None` until a licence has been seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_id: Option<String>,
}

impl LeaseState {
    pub fn new(now: OffsetDateTime) -> Self {
        Self {
            time_watermark: now,
            erasures_used: 0,
            counter_is_monotonic: false,
            license_id: None,
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

/// A counter meant to only move forward (ADR-0005 §3b).
///
/// Whether the *customer* can rewind it depends on the implementation, and
/// [`is_hardware_backed`](Self::is_hardware_backed) says whether they can:
/// [`FileCounter`] lives in an ordinary file and can be rewound by anyone who
/// edits it; [`TpmCounter`] is a seam that cannot count yet. Only a working
/// hardware-anchored counter would make the anti-rollback claim real, and no
/// such implementation exists today.
pub trait MonotonicCounter: Send + Sync {
    fn read(&self) -> LicenseResult<u64>;
    fn increment(&self) -> LicenseResult<u64>;
    /// False means "this counter can be rolled back by the customer, or does
    /// not work at all" and the UI is expected to say so rather than implying
    /// hardware backing. Only an implementation that really counts in
    /// hardware may return true.
    fn is_hardware_backed(&self) -> bool {
        false
    }
}

/// A counter persisted as a decimal integer in one file.
///
/// Honest about its limits: anyone who can write the file can rewind it, so
/// it reports `is_hardware_backed() == false`. It never fabricates a value —
/// a missing file is 0, but unreadable or unparsable content is an error.
///
/// Increments through one `FileCounter` are serialized, so concurrent callers
/// sharing it get distinct values. Two `FileCounter`s on the same path, or
/// two processes, are not coordinated: share one instance.
#[derive(Debug)]
pub struct FileCounter {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileCounter {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_unlocked(&self) -> LicenseResult<u64> {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => {
                return Err(LicenseError::Encoding(format!(
                    "reading counter {}: {e}",
                    self.path.display()
                )))
            }
        };
        raw.trim().parse::<u64>().map_err(|e| {
            LicenseError::Encoding(format!(
                "parsing counter {}: {e} (content {:?})",
                self.path.display(),
                raw.trim()
            ))
        })
    }

    /// Write beside the target and rename: a half-written counter must never
    /// be read back as a smaller number, and rename is atomic within a
    /// filesystem.
    fn write_unlocked(&self, value: u64) -> LicenseResult<()> {
        let dir = match self.path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => Path::new("."),
        };
        std::fs::create_dir_all(dir)
            .map_err(|e| LicenseError::Encoding(format!("creating {}: {e}", dir.display())))?;
        let name = self
            .path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "counter".into());
        // Unique per call, so two counters on one path never share a temp file.
        static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".{name}.tmp-{}-{seq}", std::process::id()));
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            f.write_all(value.to_string().as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, &self.path)?;
            // Persist the rename itself, or a power loss can revert the
            // counter to its previous value after `increment` returned.
            #[cfg(unix)]
            std::fs::File::open(dir)?.sync_all()?;
            Ok(())
        };
        write().map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            LicenseError::Encoding(format!("writing counter {}: {e}", self.path.display()))
        })
    }
}

impl MonotonicCounter for FileCounter {
    fn read(&self) -> LicenseResult<u64> {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.read_unlocked()
    }

    fn increment(&self) -> LicenseResult<u64> {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let current = self.read_unlocked()?;
        let next = current.checked_add(1).ok_or_else(|| {
            LicenseError::Encoding(format!(
                "counter {} is at its maximum ({current}) and cannot be incremented",
                self.path.display()
            ))
        })?;
        self.write_unlocked(next)?;
        Ok(next)
    }
}

/// Placeholder for TPM 2.0 / secure-element counters.
///
/// Returns `Unsupported` rather than a plausible number: a counter that
/// silently pretends to be monotonic is worse than none, because the whole
/// point is the strength of the guarantee. For the same reason it keeps the
/// default `is_hardware_backed() == false`; that turns true only once `read`
/// and `increment` really reach a hardware counter.
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
    /// The station's persisted lease state could not be read or saved, so
    /// the watermark and count it would check against cannot be trusted.
    /// `detail` names the file and the error.
    LeaseStateUnavailable { detail: String },
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
            Self::LeaseStateUnavailable { detail } => format!(
                "Licence state could not be read or saved ({detail}). Erasure still works; \
                 certificates will be marked evaluation until this is fixed."
            ),
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
