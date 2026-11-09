//! Station-side licensing state (ADR-0005).
//!
//! Holds the installed licence (if any), the anti-rollback lease state, and
//! this station's machine fingerprint, and answers the one question the
//! certificate generator needs at signing time:
//!
//! > may this certificate carry a vendor attestation chain, or must it be
//! > marked `evaluation`?
//!
//! Note what is *not* here: nothing in this module can stop an erasure. A
//! station with no licence, an expired lease, an exhausted quota or a
//! rolled-back clock still wipes drives and still produces a valid,
//! offline-verifiable certificate — it is simply marked as unlicensed
//! (ADR-0005 §5). Refusing to sanitize would destroy evidence the operator
//! needs, which is a worse failure than under-collected revenue.
//!
//! Lease state is loaded from a [`LeaseStore`] at start-up and saved after
//! every status check, signing decision and recorded erasure, so a restart
//! neither resets the clock watermark nor refills a `Count` quota. Lease I/O
//! never fails a signature and never holds one up for longer than
//! [`LEASE_SAVE_TIMEOUT`]: every save runs on one writer thread per context,
//! and a save that fails or does not finish in time only marks certificates
//! `evaluation` (ROADMAP §0.2), with [`LeaseStatus::LeaseStateUnavailable`]
//! saying why.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use time::OffsetDateTime;

use wipe_license::{
    evaluate, machine_fingerprint, AttestationChain, Entitlements, LeaseState, LeaseStatus,
};

use crate::store::{EphemeralLeaseStore, LeasePersistence, LeaseStore, StoreError};

/// How long a status check or signing decision waits for its lease save. A
/// save still running after this is a save fault: that decision is
/// `evaluation`, and the write carries on in the background.
pub const LEASE_SAVE_TIMEOUT: Duration = Duration::from_secs(2);

type SaveAck = mpsc::Sender<Result<(), StoreError>>;
type SaveJob = (LeaseState, SaveAck);

pub struct LicenseContext {
    chain: Option<AttestationChain>,
    fingerprint: String,
    lease: RwLock<LeaseState>,
    store: Arc<dyn LeaseStore>,
    /// Set when the stored lease exists but could not be read or parsed.
    /// Sticky for the life of the context: that file is never overwritten,
    /// so the operator can inspect or repair it.
    load_fault: Option<String>,
    /// The last save's error, cleared by the next successful save.
    save_fault: Mutex<Option<String>>,
    /// Feeds the writer thread, which performs every `store.save` in the
    /// order snapshots were sent. Taking the snapshot and sending it under
    /// this one lock is what keeps an older snapshot from landing after a
    /// newer one. `None` if the thread could not be started. Dropping the
    /// context drops the sender and ends the thread.
    writer: Mutex<Option<mpsc::Sender<SaveJob>>>,
}

/// One thread that saves lease snapshots in submission order. A hung write
/// only delays the snapshots queued behind it; when it resumes they land
/// oldest first, so the newest wins.
fn spawn_writer(store: Arc<dyn LeaseStore>) -> Option<mpsc::Sender<SaveJob>> {
    let (tx, rx) = mpsc::channel::<SaveJob>();
    let spawned = std::thread::Builder::new()
        .name("lease-writer".into())
        .spawn(move || {
            for (snapshot, ack) in rx {
                // The waiter may have timed out and gone; the result still
                // reached the disk (or not), and the next save reports.
                let _ = ack.send(store.save(&snapshot));
            }
        });
    match spawned {
        Ok(_) => Some(tx),
        Err(e) => {
            tracing::error!(error = %e, "lease writer thread did not start; lease state will not be saved");
            None
        }
    }
}

fn unwired_store() -> Arc<dyn LeaseStore> {
    Arc::new(EphemeralLeaseStore::new(
        "No lease store was wired in.".into(),
    ))
}

impl LicenseContext {
    /// An unlicensed station with lease state in RAM only. The free-tier
    /// default for hosts that wire no store.
    pub fn unlicensed(station_id: &str, now: OffsetDateTime) -> Self {
        Self::build(
            None,
            station_id,
            LeaseState::new(now),
            unwired_store(),
            None,
        )
    }

    /// A licensed station with the given lease state, held in RAM only.
    pub fn licensed(station_id: &str, chain: AttestationChain, lease: LeaseState) -> Self {
        Self::build(Some(chain), station_id, lease, unwired_store(), None)
    }

    /// Load lease state from `store` and keep saving it there.
    ///
    /// Nothing stored yet: a fresh state at `now`. Stored under a different
    /// licence: the watermark is kept and the count restarts at 0 for this
    /// licence. No licence: the stored count and licence id are left alone.
    /// Stored but unreadable: a fresh state in RAM, the file untouched, and
    /// every licensed decision `evaluation` until the file is fixed and the
    /// station restarted.
    pub fn open(
        station_id: &str,
        chain: Option<AttestationChain>,
        store: Arc<dyn LeaseStore>,
        now: OffsetDateTime,
    ) -> Self {
        let license_id = chain.as_ref().map(|c| c.license.body.license_id.clone());
        let (mut lease, load_fault) = match store.load() {
            Ok(Some(lease)) => (lease, None),
            Ok(None) => (LeaseState::new(now), None),
            Err(e) => {
                let fault = format!(
                    "lease state at {} is unreadable: {e}. Restore a good copy of the file, \
                     or remove it to start a fresh lease, then restart the station",
                    store.location()
                );
                tracing::error!(%fault, "lease state not loaded; licensed certificates will be marked evaluation");
                (LeaseState::new(now), Some(fault))
            }
        };
        if let Some(id) = license_id {
            if lease.license_id.as_deref() != Some(id.as_str()) {
                lease.erasures_used = 0;
                lease.license_id = Some(id);
            }
        }
        Self::build(chain, station_id, lease, store, load_fault)
    }

    fn build(
        chain: Option<AttestationChain>,
        station_id: &str,
        lease: LeaseState,
        store: Arc<dyn LeaseStore>,
        load_fault: Option<String>,
    ) -> Self {
        // A file that failed to load is never written, so it needs no writer.
        let writer = if load_fault.is_none() {
            spawn_writer(store.clone())
        } else {
            None
        };
        Self {
            chain,
            fingerprint: machine_fingerprint(station_id),
            lease: RwLock::new(lease),
            store,
            load_fault,
            save_fault: Mutex::new(None),
            writer: Mutex::new(writer),
        }
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn entitlements(&self) -> Option<&Entitlements> {
        self.chain.as_ref().map(|c| &c.license.body.entitlements)
    }

    pub fn lease_state(&self) -> LeaseState {
        self.lease.read().clone()
    }

    /// Where lease state lives and whether it survives reboot, for the
    /// operator. Names the fault when the stored state is unusable.
    pub fn lease_persistence(&self) -> LeasePersistence {
        let tier = self.store.tier();
        let mut detail = self.store.detail();
        if let Some(fault) = self.fault() {
            detail.push_str(&format!(
                " Lease state is unavailable ({fault}); licensed certificates are marked \
                 evaluation until this is fixed."
            ));
        }
        LeasePersistence {
            tier,
            survives_reboot: tier.survives_reboot(),
            location: self.store.location(),
            detail,
        }
    }

    /// The current lease I/O fault, if any: a sticky load fault first, else
    /// the last save's error.
    fn fault(&self) -> Option<String> {
        self.load_fault
            .clone()
            .or_else(|| self.save_fault.lock().clone())
    }

    /// Save the current lease state through the writer thread, waiting at
    /// most [`LEASE_SAVE_TIMEOUT`]. Never overwrites a file that failed to
    /// load. A failure or timeout is recorded as the save fault, which the
    /// next successful save clears.
    fn persist(&self) {
        if self.load_fault.is_some() {
            return;
        }
        let ack = {
            let writer = self.writer.lock();
            let Some(writer) = writer.as_ref() else {
                self.set_save_fault("the lease writer thread is not running".into());
                return;
            };
            let snapshot = self.lease.read().clone();
            let (ack_tx, ack_rx) = mpsc::channel();
            if writer.send((snapshot, ack_tx)).is_err() {
                drop(ack_rx);
                self.set_save_fault("the lease writer thread has stopped".into());
                return;
            }
            ack_rx
        };
        match ack.recv_timeout(LEASE_SAVE_TIMEOUT) {
            Ok(Ok(())) => *self.save_fault.lock() = None,
            Ok(Err(e)) => self.set_save_fault(e.to_string()),
            Err(mpsc::RecvTimeoutError::Timeout) => self.set_save_fault(format!(
                "the write did not finish within {} s",
                LEASE_SAVE_TIMEOUT.as_secs()
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.set_save_fault("the lease writer thread has stopped".into())
            }
        }
    }

    fn set_save_fault(&self, why: String) {
        let fault = format!(
            "lease state at {} was not saved: {why}",
            self.store.location()
        );
        tracing::error!(%fault, "lease state not saved; licensed certificates will be marked evaluation");
        *self.save_fault.lock() = Some(fault);
    }

    /// Fold an observed clock reading into the anti-rollback watermark.
    ///
    /// Called on every evaluation so the watermark tracks the highest time
    /// the station has ever seen, which is what makes a later backwards jump
    /// detectable at all.
    pub fn observe_now(&self, now: OffsetDateTime) {
        self.lease.write().observe(now);
    }

    /// Record a successful erasure against the local usage count, and save.
    ///
    /// Best-effort by construction: this is a local counter, and ADR-0005 §3
    /// is explicit that offline consumption is not provable. A failed save is
    /// logged and retried by the next save.
    pub fn record_erasure(&self) {
        {
            let mut lease = self.lease.write();
            lease.erasures_used = lease.erasures_used.saturating_add(1);
        }
        self.persist();
    }

    /// The licence verdict at `now`. Folds `now` into the watermark and
    /// saves, so the watermark on disk tracks every clock reading checked; a
    /// failed save makes the verdict `LeaseStateUnavailable` for a licensed
    /// station.
    pub fn status(&self, now: OffsetDateTime) -> LeaseStatus {
        // Evaluate against the watermark *before* folding `now` in, or a
        // rolled-back clock would silently become the new normal.
        let state = self.lease.read().clone();
        let status = evaluate(self.entitlements(), &state, &self.fingerprint, now);
        drop(state);
        self.observe_now(now);
        self.persist();
        self.with_fault(status)
    }

    /// A licensed station whose lease state cannot be trusted reports that
    /// first: every other verdict was computed from it.
    fn with_fault(&self, status: LeaseStatus) -> LeaseStatus {
        if self.chain.is_none() {
            return status;
        }
        match self.fault() {
            Some(detail) => LeaseStatus::LeaseStateUnavailable { detail },
            None => status,
        }
    }

    /// What the certificate generator needs: whether to mark the cert as
    /// evaluation, and the attestation chain to staple on if not.
    ///
    /// Saves the lease state (the watermark just moved) through
    /// [`status`](Self::status); a failed or slow save makes this decision
    /// `evaluation`, never an error.
    pub fn signing_decision(&self, now: OffsetDateTime) -> SigningDecision {
        let status = self.status(now);
        if status.permits_licensed_signing() {
            if let Some(chain) = &self.chain {
                return SigningDecision {
                    evaluation: false,
                    attestation: serde_json::to_value(chain).ok(),
                    status,
                };
            }
        }
        SigningDecision {
            evaluation: true,
            attestation: None,
            status,
        }
    }
}

pub struct SigningDecision {
    /// Goes inside the signed payload, so it cannot be stripped.
    pub evaluation: bool,
    /// Rides alongside the signature, so a renewal can re-staple without
    /// invalidating an erasure signature an auditor already checked.
    pub attestation: Option<serde_json::Value>,
    pub status: LeaseStatus,
}
