//! Offline enforcement levers and their honest limits (ADR-0005 §3).
//!
//! Nothing here can *prove* consumption — a station with no network can
//! always under-report. What it can do is judge a licence offline (expired,
//! not yet valid, wrong machine, out of credits), notice a clock that has
//! moved backwards, and say plainly which of those guarantees is
//! cryptographic and which is a speed bump. The station acts on the verdict
//! when a Job starts (ADR-0013 §1).

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

/// Anti-rollback state: the clock watermark and the local erasure counts.
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
    /// Credits spent locally under the current licence: one per licensed
    /// certificate (ADR-0013 §3). A `Count` quota is checked against this
    /// plus [`credits_reserved`](Self::credits_reserved). Evaluation
    /// erasures are counted separately in
    /// [`evaluation_erasures`](Self::evaluation_erasures).
    pub erasures_used: u64,
    /// Credits reserved by Jobs that have started but are not yet certified
    /// or aborted: one per drive. Spending moves a credit from here to
    /// `erasures_used`; an abort returns it. Jobs are not kept across a
    /// restart, so a station returns every reservation when it starts, and a
    /// licence switch clears them with the count. 0, and omitted, when none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub credits_reserved: u64,
    /// Certificates this station signed `evaluation` — from a start
    /// decision the licence did not cover, which only a simulated backend
    /// records rather than refuses (ADR-0013 §2) — for Jobs holding an
    /// erasure event. A Job destroyed
    /// after an erasure attempt, even a failed one, counts; one destroyed
    /// with no erasure attempt does not. Counted for the life of the station: it survives a
    /// licence change, like the watermark. No limit is attached to it.
    /// Defaults to 0 for lease files written before it existed.
    #[serde(default)]
    pub evaluation_erasures: u64,
    /// Set once the counter is anchored somewhere the customer cannot
    /// rewind. False for the file-backed counter.
    #[serde(default)]
    pub counter_is_monotonic: bool,
    /// The licence `erasures_used` counts against. A station that starts
    /// under a different licence keeps the watermark (a fact about the
    /// clock) and resets the count. `None` until a licence has been seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_id: Option<String>,
    /// The station's **credit reserve** (ADR-0013 §3): every verified credit
    /// grant it has taken, oldest first, with what it has spent of each.
    /// Kept for good, never pruned, so a grant id seen before is always
    /// recognised and never counted twice. Tied to the organization, not the
    /// licence: a licence switch keeps it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credit_grants: Vec<HeldGrant>,
    /// Pool credits reserved by Jobs that have started but are not yet
    /// certified or aborted, on a [`Quota::Pool`](crate::Quota::Pool)
    /// licence. Returned at start-up like
    /// [`credits_reserved`](Self::credits_reserved), but not cleared by a
    /// licence switch: they are settled by organization.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pool_credits_reserved: u64,
    /// The id of the last grant a top-up from the hub delivered, sent with
    /// the next top-up so the hub can resend an answer that was lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_topup_grant_id: Option<String>,
    /// The threshold and target reserve the hub last answered a top-up
    /// with; `None` until it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_terms: Option<CreditTerms>,
}

/// One credit grant in the station's credit reserve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldGrant {
    pub grant_id: String,
    /// The organization whose pool it came from.
    pub organization_id: String,
    pub credits: u64,
    /// Credits of this grant spent by signed certificates.
    #[serde(default)]
    pub spent: u64,
}

/// When a connected station tops its credit reserve up, and to how much:
/// below `threshold` credits, up to `target` (the offering's terms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditTerms {
    pub threshold: u64,
    pub target: u64,
}

/// Whether [`LeaseState::add_grant`] took a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantAdded {
    Added,
    /// A grant with this id is already held: nothing changed.
    AlreadyHeld,
}

impl LeaseState {
    pub fn new(now: OffsetDateTime) -> Self {
        Self {
            time_watermark: now,
            erasures_used: 0,
            credits_reserved: 0,
            evaluation_erasures: 0,
            counter_is_monotonic: false,
            license_id: None,
            credit_grants: Vec::new(),
            pool_credits_reserved: 0,
            last_topup_grant_id: None,
            credit_terms: None,
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

    /// Credits spent or reserved: what a `Count` quota is checked against.
    pub fn credits_committed(&self) -> u64 {
        self.erasures_used.saturating_add(self.credits_reserved)
    }

    fn grants_of<'a>(&'a self, organization_id: &'a str) -> impl Iterator<Item = &'a HeldGrant> {
        self.credit_grants
            .iter()
            .filter(move |g| g.organization_id == organization_id)
    }

    /// Credits ever granted to this station for `organization_id`.
    pub fn pool_held(&self, organization_id: &str) -> u64 {
        self.grants_of(organization_id)
            .fold(0u64, |n, g| n.saturating_add(g.credits))
    }

    /// Credits of `organization_id`'s grants spent by signed certificates.
    pub fn pool_spent(&self, organization_id: &str) -> u64 {
        self.grants_of(organization_id)
            .fold(0u64, |n, g| n.saturating_add(g.spent))
    }

    /// Credits held for `organization_id` and not spent, counting those
    /// reserved by running Jobs (they are still held).
    pub fn pool_balance(&self, organization_id: &str) -> u64 {
        self.pool_held(organization_id)
            .saturating_sub(self.pool_spent(organization_id))
    }

    /// Credits a start may reserve for `organization_id`: the balance less
    /// every pool credit reserved. What a top-up reports as its balance and
    /// compares with the threshold.
    pub fn pool_available(&self, organization_id: &str) -> u64 {
        self.pool_balance(organization_id)
            .saturating_sub(self.pool_credits_reserved)
    }

    /// Whether a grant with this id is held.
    pub fn holds_grant(&self, grant_id: &str) -> bool {
        self.credit_grants.iter().any(|g| g.grant_id == grant_id)
    }

    /// Take a verified grant into the reserve. Verify it first
    /// ([`crate::check_grant_for`]): this only refuses an id already held.
    pub fn add_grant(&mut self, grant: &crate::CreditGrant) -> GrantAdded {
        if self.holds_grant(&grant.body.grant_id) {
            return GrantAdded::AlreadyHeld;
        }
        self.credit_grants.push(HeldGrant {
            grant_id: grant.body.grant_id.clone(),
            organization_id: grant.body.organization_id.clone(),
            credits: grant.body.credits,
            spent: 0,
        });
        GrantAdded::Added
    }

    /// Spend `n` credits of `organization_id`'s grants, oldest first. Credits
    /// beyond what the grants hold are dropped (saturating).
    pub fn spend_pool(&mut self, organization_id: &str, n: u64) {
        let mut left = n;
        for g in self
            .credit_grants
            .iter_mut()
            .filter(|g| g.organization_id == organization_id)
        {
            if left == 0 {
                break;
            }
            let take = g.credits.saturating_sub(g.spent).min(left);
            g.spent += take;
            left -= take;
        }
    }
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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
    /// Every credit of a `Count` quota is spent or reserved by a running
    /// Job (`used` counts both). Best-effort offline. On a `Pool` licence:
    /// the station's credit reserve for the organization is spent or
    /// reserved (`allowed` is every credit granted to it).
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
    /// Is this a valid licence with a credit to spare?
    ///
    /// The station decides at a Job's start, not here, whether a licence
    /// covers the work (ADR-0013 §1); this only reads the verdict.
    pub fn permits_licensed_signing(&self) -> bool {
        matches!(self, Self::Valid { .. })
    }

    /// The status in words, without its consequence: the station's start
    /// check and the bench banner say what it means for new Jobs.
    pub fn operator_message(&self) -> String {
        match self {
            Self::Valid {
                remaining_erasures,
                days_remaining,
            } => match remaining_erasures {
                Some(n) => format!("Licensed - {n} erasures remaining, {days_remaining} days left"),
                None => format!("Licensed - unlimited, {days_remaining} days left"),
            },
            Self::Expired { not_after } => {
                format!("Licence expired on {}.", not_after.date())
            }
            Self::NotYetValid { not_before } => {
                format!("Licence is not valid until {}.", not_before.date())
            }
            Self::QuotaExhausted { used, allowed } => format!(
                "Licence credits used ({used} of {allowed}, counting credits reserved by running \
                 jobs)."
            ),
            Self::WrongMachine { expected, actual } => {
                format!("Licence is bound to machine {expected}, but this station is {actual}.")
            }
            Self::ClockRollback {
                watermark,
                observed,
            } => format!(
                "System clock reads {} but this station has previously seen {}. Time appears to \
                 have moved backwards.",
                observed.date(),
                watermark.date()
            ),
            Self::Unlicensed => "No licence installed.".into(),
            Self::LeaseStateUnavailable { detail } => {
                format!("Licence state could not be read or saved ({detail}).")
            }
        }
    }
}

/// Evaluate a licence offline.
///
/// `fingerprint` is this station's machine fingerprint; `now` is the current
/// wall clock. `state` supplies the anti-rollback watermark and the credits
/// spent and reserved, and is **not** mutated here — callers decide when to
/// reserve or spend.
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

    if let crate::entitlement::Quota::Pool = ent.quota {
        let o = ent.customer_id.as_str();
        let available = state.pool_available(o);
        if available == 0 {
            return LeaseStatus::QuotaExhausted {
                used: state
                    .pool_spent(o)
                    .saturating_add(state.pool_credits_reserved),
                allowed: state.pool_held(o),
            };
        }
        return LeaseStatus::Valid {
            remaining_erasures: Some(available),
            days_remaining: (ent.not_after - now).whole_days(),
        };
    }

    let committed = state.credits_committed();
    if !ent.quota.permits(committed) {
        if let crate::entitlement::Quota::Count { erasures } = ent.quota {
            return LeaseStatus::QuotaExhausted {
                used: committed,
                allowed: erasures,
            };
        }
    }

    LeaseStatus::Valid {
        remaining_erasures: ent.quota.remaining(committed),
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
