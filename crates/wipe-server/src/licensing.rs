//! Station-side licensing state (ADR-0005, ADR-0013).
//!
//! Holds the installed licence (if any), the anti-rollback lease state, and
//! this station's machine fingerprint, and answers the one question a Job's
//! start needs (ADR-0013 §1):
//!
//! > does the licence cover this work, and if so, which chain does its
//! > certificate carry and which credits are reserved for it?
//!
//! [`LicenseContext::decide_start`] answers it when a Job starts or drives
//! are recorded for destruction. On a real device backend a start the
//! licence does not cover is refused ([`StartRefusal`]) and the Job stays
//! queued; on a simulated backend nothing is refused and an uncovered start
//! is certified `evaluation` (ADR-0013 §2). The answer is recorded on the
//! Job ([`wipe_common::JobLicence`]) and is final: the certificate is marked
//! from it, never by re-checking the licence when it is signed, so a Job
//! that started covered stays licensed however long it runs. A running Job
//! is never interrupted.
//!
//! Credits (ADR-0013 §3): a licensed start reserves one credit per drive, a
//! signed certificate spends it ([`spend`](LicenseContext::spend)) and an
//! abort returns it ([`release`](LicenseContext::release)). A `Count`
//! licence counts its own credits. A `Pool` licence draws on the station's
//! **credit reserve**: the verified credit grants kept in its lease state
//! for the licence's organization ([`add_grant`](LicenseContext::add_grant)).
//! Pool reservations are settled by organization, whatever licence is
//! installed then, and a licence switch keeps the reserve.
//!
//! Lease state is loaded from a [`LeaseStore`] at start-up and saved after
//! every start decision, settlement and status check, so a restart neither
//! resets the clock watermark nor refills a `Count` quota. Reservations are
//! returned at start-up, because Jobs do not survive a restart. Every save
//! runs on one writer thread, shared by a context and every context derived
//! from it with [`with_chain`](LicenseContext::with_chain), and a caller
//! waits at most [`LEASE_SAVE_TIMEOUT`] for it. On a `Count` licence a
//! reservation that is not saved in time refuses the start; on an
//! `Unlimited` one a lease fault never makes a start uncovered, and
//! [`LeaseStatus::LeaseStateUnavailable`] reports it.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use time::OffsetDateTime;

use wipe_common::{JobLicence, Method};
use wipe_license::{
    check_grant_for, evaluate, machine_fingerprint, AllowedMethods, AttestationChain, CreditGrant,
    CreditTerms, Entitlements, GrantAdded, InstallProblem, LeaseState, LeaseStatus, MethodClass,
    Quota,
};

use crate::store::{EphemeralLeaseStore, LeasePersistence, LeaseStore, StoreError};

/// How long a start decision or status check waits for its lease save. A
/// save still running after this is a save fault: on a `Count` licence the
/// start is refused (its reservation is not on disk), and the write carries
/// on in the background.
pub const LEASE_SAVE_TIMEOUT: Duration = Duration::from_secs(2);

type SaveAck = mpsc::Sender<Result<(), StoreError>>;
type SaveJob = (LeaseState, SaveAck);

/// The installed licence (if any) and the running lease.
///
/// The lease cells (`lease`, `save_fault`, `writer`) are shared by every
/// context derived through [`with_chain`](Self::with_chain), so a licence
/// installed at runtime keeps the running lease and saves through the same
/// ordered writer as a settlement still holding the previous context.
pub struct LicenseContext {
    chain: Option<AttestationChain>,
    fingerprint: String,
    lease: Arc<RwLock<LeaseState>>,
    store: Arc<dyn LeaseStore>,
    /// Set when the stored lease exists but could not be read or parsed.
    /// Sticky for the life of the lease: that file is never overwritten, so
    /// the operator can inspect or repair it.
    load_fault: Option<String>,
    /// The last save's error, cleared by the next successful save.
    save_fault: Arc<Mutex<Option<String>>>,
    /// Feeds the writer thread, which performs every `store.save` in the
    /// order snapshots were sent. Taking the snapshot and sending it under
    /// this one lock is what keeps an older snapshot from landing after a
    /// newer one. `None` if the thread could not be started. Dropping the
    /// last context sharing it drops the sender and ends the thread.
    writer: Arc<Mutex<Option<mpsc::Sender<SaveJob>>>>,
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

/// Make `lease` count for `license_id`. A lease kept for another licence,
/// or for none, restarts its count and drops its reservations; the
/// watermark and the evaluation count are kept.
fn adopt_licence(lease: &mut LeaseState, license_id: &str) {
    if lease.license_id.as_deref() != Some(license_id) {
        lease.erasures_used = 0;
        lease.credits_reserved = 0;
        lease.license_id = Some(license_id.to_owned());
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
    /// A lease kept for another licence (or none) is switched to this one
    /// as [`open`](Self::open) does: its count and reservations restart.
    pub fn licensed(station_id: &str, chain: AttestationChain, mut lease: LeaseState) -> Self {
        adopt_licence(&mut lease, &chain.license.body.license_id);
        Self::build(Some(chain), station_id, lease, unwired_store(), None)
    }

    /// Load lease state from `store` and keep saving it there.
    ///
    /// Nothing stored yet: a fresh state at `now`. Stored under a different
    /// licence: the licensed count restarts at 0 for this licence, and the
    /// evaluation count is kept with the watermark. No licence: the stored
    /// counts and licence id are left alone. Stored reservations are always
    /// returned: the Jobs that held them did not survive the restart.
    /// Stored but unreadable: a fresh state in RAM and the file untouched.
    /// A `Count` licence then refuses every start until the file is fixed
    /// and the station restarted; an `Unlimited` one keeps starting
    /// licensed Jobs.
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
                tracing::error!(%fault, "lease state not loaded; a licence that counts credits refuses new jobs");
                (LeaseState::new(now), Some(fault))
            }
        };
        lease.credits_reserved = 0;
        lease.pool_credits_reserved = 0;
        if let Some(id) = license_id {
            adopt_licence(&mut lease, &id);
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
            lease: Arc::new(RwLock::new(lease)),
            store,
            load_fault,
            save_fault: Arc::new(Mutex::new(None)),
            writer: Arc::new(Mutex::new(writer)),
        }
    }

    /// This station under `chain`, keeping the running lease: the same
    /// watermark, evaluation count, store, writer thread and lease faults.
    /// For a licence installed while the station runs.
    ///
    /// The licensed count follows [`open`](Self::open)'s rule: it restarts at
    /// 0 when `chain`'s licence id differs from the lease's, with every
    /// reservation, and both are kept when it is the same licence. A running
    /// Job whose decision names the old licence still certifies under it,
    /// and settling it changes no count. The change is made to the shared lease in RAM
    /// at once (so `self` sees it too) and reaches the store with the next
    /// save; a lease file that failed to load is still never written.
    ///
    /// Does not verify `chain`: run `wipe_license::check_installable` first.
    pub fn with_chain(&self, chain: AttestationChain) -> LicenseContext {
        adopt_licence(&mut self.lease.write(), &chain.license.body.license_id);
        LicenseContext {
            chain: Some(chain),
            fingerprint: self.fingerprint.clone(),
            lease: self.lease.clone(),
            store: self.store.clone(),
            load_fault: self.load_fault.clone(),
            save_fault: self.save_fault.clone(),
            writer: self.writer.clone(),
        }
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The installed licence's id; `None` on an unlicensed station.
    pub fn license_id(&self) -> Option<&str> {
        self.chain
            .as_ref()
            .map(|c| c.license.body.license_id.as_str())
    }

    pub fn entitlements(&self) -> Option<&Entitlements> {
        self.chain.as_ref().map(|c| &c.license.body.entitlements)
    }

    /// When the installed licence was issued; `None` on an unlicensed
    /// station.
    pub fn issued_at(&self) -> Option<OffsetDateTime> {
        self.chain.as_ref().map(|c| c.license.body.issued_at)
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
                " Lease state is unavailable ({fault}); a licence that counts credits refuses \
                 new jobs until this is fixed."
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

    /// Save the current lease state, best effort: see
    /// [`persist_checked`](Self::persist_checked).
    fn persist(&self) {
        let _ = self.persist_checked();
    }

    /// Save the current lease state through the writer thread, waiting at
    /// most [`LEASE_SAVE_TIMEOUT`]. Never overwrites a file that failed to
    /// load: that is an error naming the load fault. A failure or timeout is
    /// recorded as the save fault, which the next successful save clears,
    /// and returned.
    fn persist_checked(&self) -> Result<(), String> {
        if let Some(fault) = &self.load_fault {
            return Err(fault.clone());
        }
        let ack = {
            let writer = self.writer.lock();
            let Some(writer) = writer.as_ref() else {
                return Err(self.set_save_fault("the lease writer thread is not running".into()));
            };
            let snapshot = self.lease.read().clone();
            let (ack_tx, ack_rx) = mpsc::channel();
            if writer.send((snapshot, ack_tx)).is_err() {
                drop(ack_rx);
                return Err(self.set_save_fault("the lease writer thread has stopped".into()));
            }
            ack_rx
        };
        match ack.recv_timeout(LEASE_SAVE_TIMEOUT) {
            Ok(Ok(())) => {
                *self.save_fault.lock() = None;
                Ok(())
            }
            Ok(Err(e)) => Err(self.set_save_fault(e.to_string())),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(self.set_save_fault(format!(
                "the write did not finish within {} s",
                LEASE_SAVE_TIMEOUT.as_secs()
            ))),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(self.set_save_fault("the lease writer thread has stopped".into()))
            }
        }
    }

    /// Record a save fault and return its text.
    fn set_save_fault(&self, why: String) -> String {
        let fault = format!(
            "lease state at {} was not saved: {why}",
            self.store.location()
        );
        tracing::error!(%fault, "lease state not saved");
        *self.save_fault.lock() = Some(fault.clone());
        fault
    }

    /// Fold an observed clock reading into the anti-rollback watermark.
    ///
    /// Called on every evaluation so the watermark tracks the highest time
    /// the station has ever seen, which is what makes a later backwards jump
    /// detectable at all.
    pub fn observe_now(&self, now: OffsetDateTime) {
        self.lease.write().observe(now);
    }

    /// Record an erasure certified `evaluation` (ADR-0005 §5), and save.
    ///
    /// Kept apart from the licensed count, so it never touches a `Count`
    /// quota, and kept across licence changes. Best-effort, like
    /// [`spend`](Self::spend): a failed save is logged and retried by the
    /// next save, and nothing is limited by this count.
    pub fn record_evaluation_erasure(&self) {
        {
            let mut lease = self.lease.write();
            lease.evaluation_erasures = lease.evaluation_erasures.saturating_add(1);
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

    /// The licence state for an operator, as `GET /api/license` returns it.
    ///
    /// The verdict at `now` against the stored lease state and the last
    /// recorded lease fault (a load fault, or the last save's error, which
    /// stays until a save succeeds), reported as `LeaseStateUnavailable`.
    /// `new_jobs_refused` is the refusal the next one-drive start would get
    /// on a real backend (`simulated` false), predicted from the status
    /// alone: a method the licence does not allow depends on the drive and is
    /// not predicted, and on a `Count` licence a save fault the next save
    /// would clear is still reported. A pure read: it neither folds `now`
    /// into the watermark nor saves, so polling it never writes to disk
    /// (GET is safe, RFC 9110 §9.2.1).
    pub fn view(&self, now: OffsetDateTime, simulated: bool) -> LicenseView {
        let state = self.lease.read().clone();
        let raw = evaluate(self.entitlements(), &state, &self.fingerprint, now);
        let new_jobs_refused = if simulated {
            None
        } else {
            self.predicted_refusal(&raw, &state).map(|r| r.message())
        };
        let status = self.with_fault(raw);
        let operator_message = status.operator_message();
        let body = self.chain.as_ref().map(|c| &c.license.body);
        let paid_through = body.and_then(|b| b.entitlements.paid_through);
        let renewal_overdue = if simulated {
            None
        } else {
            body.and_then(|b| renewal_overdue(&b.entitlements, now))
        };
        LicenseView {
            status,
            operator_message,
            customer: body.map(|b| LicenseCustomer {
                id: b.entitlements.customer_id.clone(),
                name: b.entitlements.customer_name.clone(),
            }),
            not_after: body.map(|b| b.entitlements.not_after),
            paid_through,
            instance_public_key_id: body.map(|b| b.instance_public_key_id.clone()),
            new_jobs_refused,
            renewal_overdue,
            simulated_backend: simulated,
            fingerprint: self.fingerprint.clone(),
            hub_check: None,
            credit_top_up: None,
        }
    }

    /// What a one-drive start on a real backend would be refused for, from
    /// the status and stored lease alone.
    fn predicted_refusal(&self, raw: &LeaseStatus, state: &LeaseState) -> Option<StartRefusal> {
        let Some(ent) = self.entitlements() else {
            return Some(StartRefusal::NoLicence);
        };
        licence_refusal(raw)
            .or_else(|| {
                counts_credits(ent)
                    .then(|| self.fault())
                    .flatten()
                    .map(|detail| StartRefusal::LeaseUnavailable { detail })
            })
            .or_else(|| credit_refusal(ent, state, 1))
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

    /// Decide whether the licence covers a start (ADR-0013 §1), and record
    /// it: a Job starting, or drives being recorded for destruction.
    ///
    /// Under one write lock on the lease: judge the licence against the
    /// stored watermark, fold `now` in, check the method and the credits,
    /// and reserve one credit per drive, so two concurrent starts cannot
    /// both take the last credit (on a `Pool` licence, from the station's
    /// credit reserve for the licence's organization, recorded as the
    /// decision's `credit_pool`). Then save. On a `Count` or `Pool` licence
    /// the save must land within [`LEASE_SAVE_TIMEOUT`], or the reservation is undone
    /// and the start refused; on an `Unlimited` one a lease fault changes
    /// nothing (only the rollback watermark is at stake, checked here
    /// against the one in RAM).
    ///
    /// A start the licence covers gets a licensed decision carrying the
    /// chain. One it does not cover is refused with the reason, nothing
    /// reserved; on a simulated backend (`req.simulated`) it is never
    /// refused and gets an `evaluation` decision instead (ADR-0013 §2).
    pub fn decide_start(
        &self,
        req: &StartRequest,
        now: OffsetDateTime,
    ) -> Result<JobLicence, StartRefusal> {
        let uncovered = |refusal: StartRefusal| {
            if req.simulated {
                Ok(JobLicence::evaluation(now))
            } else {
                Err(refusal)
            }
        };
        let Some(chain) = &self.chain else {
            self.observe_now(now);
            self.persist();
            return uncovered(StartRefusal::NoLicence);
        };
        let ent = &chain.license.body.entitlements;
        let license_id = chain.license.body.license_id.clone();
        let counted = counts_credits(ent);
        let pool = matches!(ent.quota, Quota::Pool).then(|| ent.customer_id.clone());
        let drives = u64::from(req.drives);

        let checked = {
            let mut lease = self.lease.write();
            // Before folding `now` in, or a rolled-back clock would silently
            // become the new normal.
            let status = evaluate(Some(ent), &lease, &self.fingerprint, now);
            lease.observe(now);
            let refusal = licence_refusal(&status)
                .or_else(|| method_refusal(&ent.allowed_methods, req.method.as_ref()))
                .or_else(|| {
                    counted
                        .then(|| self.load_fault.clone())
                        .flatten()
                        .map(|detail| StartRefusal::LeaseUnavailable { detail })
                })
                .or_else(|| credit_refusal(ent, &lease, drives));
            match refusal {
                Some(refusal) => Err(refusal),
                None if pool.is_some() => {
                    lease.pool_credits_reserved =
                        lease.pool_credits_reserved.saturating_add(drives);
                    Ok(())
                }
                None => {
                    lease.credits_reserved = lease.credits_reserved.saturating_add(drives);
                    Ok(())
                }
            }
        };
        if let Err(refusal) = checked {
            // The watermark moved; saving it is best effort.
            self.persist();
            return uncovered(refusal);
        }
        if let Err(detail) = self.persist_checked() {
            if counted {
                self.unreserve(&license_id, pool.is_some(), drives);
                return uncovered(StartRefusal::LeaseUnavailable { detail });
            }
        }
        Ok(JobLicence {
            evaluation: false,
            attestation: serde_json::to_value(chain).ok(),
            license_id: Some(license_id),
            credits_reserved: req.drives,
            credit_pool: pool,
            decided_at: now,
        })
    }

    /// Spend a decision's reservation: its certificate was signed (ADR-0013
    /// §3), including one for a destroyed drive that was never erased. The
    /// reserved credits become used ones, and the lease is saved, best
    /// effort. Nothing for an `evaluation` decision, or one taken under a
    /// `Count` licence the lease no longer counts for. A pool decision
    /// spends from its organization's grants, oldest first, whatever
    /// licence is installed now.
    pub fn spend(&self, decision: &JobLicence) {
        self.settle(decision, true);
    }

    /// Return a decision's reservation: its Job was aborted. Nothing for an
    /// `evaluation` decision, or one taken under a licence the lease no
    /// longer counts for.
    pub fn release(&self, decision: &JobLicence) {
        self.settle(decision, false);
    }

    fn settle(&self, decision: &JobLicence, spent: bool) {
        if decision.evaluation || decision.credits_reserved == 0 {
            return;
        }
        let Some(license_id) = decision.license_id.as_deref() else {
            return;
        };
        let n = u64::from(decision.credits_reserved);
        if let Some(organization_id) = decision.credit_pool.as_deref() {
            {
                let mut lease = self.lease.write();
                lease.pool_credits_reserved = lease.pool_credits_reserved.saturating_sub(n);
                if spent {
                    lease.spend_pool(organization_id, n);
                }
            }
            self.persist();
            return;
        }
        {
            let mut lease = self.lease.write();
            if lease.license_id.as_deref() != Some(license_id) {
                return;
            }
            lease.credits_reserved = lease.credits_reserved.saturating_sub(n);
            if spent {
                lease.erasures_used = lease.erasures_used.saturating_add(n);
            }
        }
        self.persist();
    }

    /// Undo a reservation that was not saved. A `Count` one only under the
    /// same licence: a licence switch meanwhile already cleared it. A pool
    /// one always: a switch keeps the reserve.
    fn unreserve(&self, license_id: &str, pool: bool, drives: u64) {
        let mut lease = self.lease.write();
        if pool {
            lease.pool_credits_reserved = lease.pool_credits_reserved.saturating_sub(drives);
        } else if lease.license_id.as_deref() == Some(license_id) {
            lease.credits_reserved = lease.credits_reserved.saturating_sub(drives);
        }
    }

    /// Take a credit grant into the station's credit reserve (ADR-0013 §3),
    /// once it verifies against `roots` and is bound to `station_key_id`
    /// (`wipe_license::check_grant_for`); a grant that does not is refused
    /// and nothing changes. A grant id already held is
    /// [`GrantAdded::AlreadyHeld`], nothing changed. A new one is added and
    /// saved: a failed save is reported in [`GrantIntake::saved`], and the
    /// grant stays in RAM for the next save to write.
    pub fn add_grant(
        &self,
        grant: &CreditGrant,
        station_key_id: &str,
        roots: &[wipe_cert::VerifyingKey],
    ) -> Result<GrantIntake, InstallProblem> {
        check_grant_for(grant, station_key_id, roots)?;
        let added = self.lease.write().add_grant(grant);
        let saved = match added {
            GrantAdded::AlreadyHeld => Ok(()),
            GrantAdded::Added => self.persist_checked(),
        };
        if added == GrantAdded::Added {
            tracing::info!(grant_id = %grant.body.grant_id, credits = grant.body.credits,
                organization = %grant.body.organization_id, "credit grant added to the reserve");
        }
        Ok(GrantIntake { added, saved })
    }

    /// The credit reserve the installed licence draws on; `None` unless it
    /// is a `Pool` licence.
    pub fn pool_reserve(&self) -> Option<PoolReserve> {
        let ent = self.entitlements()?;
        if !matches!(ent.quota, Quota::Pool) {
            return None;
        }
        let o = ent.customer_id.as_str();
        let lease = self.lease.read();
        Some(PoolReserve {
            organization_id: o.to_owned(),
            balance: lease.pool_balance(o),
            spent: lease.pool_spent(o),
            available: lease.pool_available(o),
            terms: lease.credit_terms,
            last_topup_grant_id: lease.last_topup_grant_id.clone(),
        })
    }

    /// Record a top-up answer: the hub's `terms`, and the id of the grant it
    /// delivered when one was added. Saved, best effort.
    pub fn record_top_up(&self, terms: CreditTerms, grant_id: Option<&str>) {
        {
            let mut lease = self.lease.write();
            lease.credit_terms = Some(terms);
            if let Some(id) = grant_id {
                lease.last_topup_grant_id = Some(id.to_owned());
            }
        }
        self.persist();
    }
}

/// What [`LicenseContext::add_grant`] did with a verified grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantIntake {
    pub added: GrantAdded,
    /// Whether the lease was saved with it; `Err` names the fault. The grant
    /// is held in RAM either way.
    pub saved: Result<(), String>,
}

/// The station's credit reserve for the installed `Pool` licence's
/// organization ([`LicenseContext::pool_reserve`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolReserve {
    pub organization_id: String,
    /// Held and not spent, counting credits reserved by running Jobs.
    pub balance: u64,
    /// Spent by signed certificates, over the life of the reserve.
    pub spent: u64,
    /// What a start may reserve now.
    pub available: u64,
    /// The hub's last answered threshold and target; `None` before any.
    pub terms: Option<CreditTerms>,
    pub last_topup_grant_id: Option<String>,
}

/// A licence whose starts reserve credits and so need lease state: `Count`
/// and `Pool`.
fn counts_credits(ent: &Entitlements) -> bool {
    matches!(ent.quota, Quota::Count { .. } | Quota::Pool)
}

/// The refusal a licence status gives, judged on the licence's own checks
/// (expiry, not yet valid, machine, rollback). An exhausted quota is not one
/// of them: credits are checked against the drives a start needs.
fn licence_refusal(status: &LeaseStatus) -> Option<StartRefusal> {
    match status {
        LeaseStatus::Valid { .. } | LeaseStatus::QuotaExhausted { .. } => None,
        LeaseStatus::Unlicensed => Some(StartRefusal::NoLicence),
        LeaseStatus::Expired { not_after } => Some(StartRefusal::Expired {
            not_after: *not_after,
        }),
        LeaseStatus::NotYetValid { not_before } => Some(StartRefusal::NotYetValid {
            not_before: *not_before,
        }),
        LeaseStatus::WrongMachine { expected, actual } => Some(StartRefusal::WrongMachine {
            expected: expected.clone(),
            actual: actual.clone(),
        }),
        LeaseStatus::ClockRollback {
            watermark,
            observed,
        } => Some(StartRefusal::ClockRollback {
            watermark: *watermark,
            observed: *observed,
        }),
        LeaseStatus::LeaseStateUnavailable { detail } => Some(StartRefusal::LeaseUnavailable {
            detail: detail.clone(),
        }),
    }
}

/// Under `AllowedMethods::Only`, the start's method must be resolved and
/// allowed; under `All`, anything goes, resolved or not.
fn method_refusal(allowed: &AllowedMethods, method: Option<&Method>) -> Option<StartRefusal> {
    match (allowed, method) {
        (AllowedMethods::All, _) => None,
        (_, Some(m)) if allowed.permits(m) => None,
        (_, Some(m)) => Some(StartRefusal::MethodNotAllowed {
            class: MethodClass::of(m),
        }),
        (_, None) => Some(StartRefusal::MethodUnresolvable),
    }
}

/// On a `Count` licence, are `drives` credits left once every spent and
/// reserved one is counted?
fn credit_refusal(ent: &Entitlements, lease: &LeaseState, drives: u64) -> Option<StartRefusal> {
    if let Quota::Pool = ent.quota {
        let left = lease.pool_available(&ent.customer_id);
        return (left < drives).then_some(StartRefusal::NoPoolCredits {
            needed: drives,
            left,
        });
    }
    let Quota::Count { erasures } = ent.quota else {
        return None;
    };
    let left = erasures.saturating_sub(lease.credits_committed());
    (left < drives).then_some(StartRefusal::NoCredits {
        needed: drives,
        left,
    })
}

/// What a start asks the licence to cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRequest {
    /// The method the start resolved: the one the run will issue, or
    /// `Destroy` for drives recorded for destruction. `None` when it could
    /// not be resolved (the device was not enumerated).
    pub method: Option<Method>,
    /// Drives covered: one credit each on a licensed decision.
    pub drives: u32,
    /// The station's backend only simulates devices (ADR-0013 §2): an
    /// uncovered start is `evaluation`, never refused.
    pub simulated: bool,
}

/// Why the licence does not cover a start on a real backend. Each
/// [`message`](Self::message) names the reason and the fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartRefusal {
    NoLicence,
    Expired {
        not_after: OffsetDateTime,
    },
    NotYetValid {
        not_before: OffsetDateTime,
    },
    WrongMachine {
        expected: String,
        actual: String,
    },
    ClockRollback {
        watermark: OffsetDateTime,
        observed: OffsetDateTime,
    },
    /// The licence's `allowed_methods` does not include this method class.
    MethodNotAllowed {
        class: MethodClass,
    },
    /// The licence allows only some methods, and the drive's could not be
    /// resolved, so coverage cannot be shown.
    MethodUnresolvable,
    /// A `Count` licence has too few credits left, counting reserved ones.
    NoCredits {
        needed: u64,
        left: u64,
    },
    /// A `Pool` licence, and the station's credit reserve for its
    /// organization holds too few credits, counting reserved ones.
    NoPoolCredits {
        needed: u64,
        left: u64,
    },
    /// A `Count` or `Pool` licence's lease state could not be read, or the
    /// reservation could not be saved. `detail` names the file and error.
    LeaseUnavailable {
        detail: String,
    },
}

impl StartRefusal {
    /// "<what is wrong>. <how to fix it>." for the operator.
    pub fn message(&self) -> String {
        match self {
            Self::NoLicence => "This station has no licence, so it cannot start new jobs. \
                 Install a licence to start them."
                .into(),
            Self::Expired { not_after } => format!(
                "This station's licence expired on {}. Install the renewed licence to start \
                 new jobs.",
                not_after.date()
            ),
            Self::NotYetValid { not_before } => format!(
                "This station's licence is not valid until {}. Start new jobs from that date, \
                 or install a licence that is valid now.",
                not_before.date()
            ),
            Self::WrongMachine { expected, actual } => format!(
                "This station's licence is bound to machine {expected}, but this station is \
                 {actual}. Install the licence issued for this station to start new jobs."
            ),
            Self::ClockRollback {
                watermark,
                observed,
            } => format!(
                "This station's clock reads {} but it has previously seen {}, so the clock \
                 appears to have moved backwards. Correct the system clock to start new jobs.",
                observed.date(),
                watermark.date()
            ),
            Self::MethodNotAllowed { class } => format!(
                "This station's licence does not allow {}. Install a licence that allows it to \
                 start this job.",
                method_class_label(*class)
            ),
            Self::MethodUnresolvable => "The erasure method for this drive could not be \
                 determined, so it could not be checked against the methods this station's \
                 licence allows. Check that the drive is attached and detected, then start the \
                 job again."
                .into(),
            Self::NoCredits { needed, left: 0 } if *needed <= 1 => {
                "This station's licence has no credits left. Add credits, or install a licence \
                 with credits, to start new jobs."
                    .into()
            }
            Self::NoCredits { needed, left } => format!(
                "This start needs {needed} credits, and this station's licence has {left} left. \
                 Add credits, or install a licence with credits, to start it."
            ),
            Self::NoPoolCredits { needed, left: 0 } if *needed <= 1 => {
                "This station has no credits left. Connect it to the platform so it can top up, \
                 or install a licence file carrying credits, to start new jobs."
                    .into()
            }
            Self::NoPoolCredits { needed, left } => format!(
                "This start needs {needed} credits, and this station holds {left}. Connect it to \
                 the platform so it can top up, or install a licence file carrying credits, to \
                 start it."
            ),
            Self::LeaseUnavailable { detail } => format!(
                "This station's licence counts credits, but its lease state could not be used \
                 ({detail}), so no credit could be reserved. Fix or restore that file, then \
                 start the job again."
            ),
        }
    }
}

fn method_class_label(class: MethodClass) -> &'static str {
    match class {
        MethodClass::NvmeSanitize => "NVMe Sanitize erasure",
        MethodClass::AtaSecureErase => "ATA Secure Erase",
        MethodClass::BlockOverwrite => "block overwrite erasure",
        MethodClass::OpalRevert => "TCG Opal revert",
        MethodClass::Destroy => "physical destruction",
    }
}

impl std::fmt::Display for StartRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for StartRefusal {}

/// The station's licence state for an operator (`GET /api/license`).
///
/// Every licence field is `null` on an unlicensed station. `status` stays
/// nested rather than flattened, so the `LeaseStatus` tag cannot collide with
/// a view field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LicenseView {
    pub status: LeaseStatus,
    /// `status.operator_message()`, so every client words it the same way.
    pub operator_message: String,
    pub customer: Option<LicenseCustomer>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub not_after: Option<OffsetDateTime>,
    /// The end of the paid term when the installed licence carries a renewal
    /// grace (ADR-0013 §4); `null` otherwise.
    #[serde(with = "time::serde::rfc3339::option")]
    pub paid_through: Option<OffsetDateTime>,
    /// The instance key id the licence entitles. The station's own signing
    /// key id is at `GET /api/public_key`.
    pub instance_public_key_id: Option<String>,
    /// Why the next start on this station would be refused, in the words
    /// the refusal uses; `null` when it would not be, and always on a
    /// simulated backend. A method the licence does not allow is not
    /// predicted: it depends on the drive.
    pub new_jobs_refused: Option<String>,
    /// While the installed licence's paid-through date has passed and its
    /// own end has not: that the renewal payment is overdue and when new jobs
    /// will be refused ([`renewal_overdue`]). `null` otherwise, and always on
    /// a simulated backend, which refuses nothing.
    pub renewal_overdue: Option<String>,
    /// The station's backend only simulates devices: nothing is refused, and
    /// an uncovered start is certified `evaluation` (ADR-0013 §2).
    pub simulated_backend: bool,
    /// This station's fingerprint, a hash of its station id (`--station-id`,
    /// default `standalone`); not derived from hardware.
    pub fingerprint: String,
    /// The latest licence check with the platform (ADR-0008 §3), filled in
    /// by `GET /api/license`; absent when no check has run (no hub address
    /// saved).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub_check: Option<crate::licence_fetch::HubCheck>,
    /// The latest credit top-up with the platform (ADR-0013 §3), filled in
    /// by `GET /api/license`; absent when none has run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credit_top_up: Option<crate::licence_fetch::HubCheck>,
}

/// The bench's overdue line for a licence whose renewal grace has begun:
/// when `paid_through < now <= not_after`, "Renewal payment overdue since
/// {paid_through date}. New jobs will be refused from {not_after date}
/// unless it is paid."; `None` otherwise (no paid-through date, not yet due,
/// or already ended, which the expiry status reports).
pub fn renewal_overdue(ent: &Entitlements, now: OffsetDateTime) -> Option<String> {
    let paid_through = ent.paid_through?;
    (paid_through < now && now <= ent.not_after).then(|| {
        format!(
            "Renewal payment overdue since {}. New jobs will be refused from {} unless it is \
             paid.",
            paid_through.date(),
            ent.not_after.date()
        )
    })
}

/// Who a licence is issued to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LicenseCustomer {
    pub id: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{StoreError, StoreTier};
    use time::Duration;

    /// A lease store in RAM that records saves, or whose load or saves fail.
    struct MemStore {
        stored: Mutex<Option<LeaseState>>,
        corrupt: bool,
        fail_saves: bool,
        saves: Mutex<usize>,
    }

    impl MemStore {
        fn holding(state: Option<LeaseState>) -> Arc<Self> {
            Arc::new(Self {
                stored: Mutex::new(state),
                corrupt: false,
                fail_saves: false,
                saves: Mutex::new(0),
            })
        }
        fn corrupt() -> Arc<Self> {
            Arc::new(Self {
                stored: Mutex::new(None),
                corrupt: true,
                fail_saves: false,
                saves: Mutex::new(0),
            })
        }
        fn failing_saves() -> Arc<Self> {
            Arc::new(Self {
                stored: Mutex::new(None),
                corrupt: false,
                fail_saves: true,
                saves: Mutex::new(0),
            })
        }
    }

    impl LeaseStore for MemStore {
        fn tier(&self) -> StoreTier {
            StoreTier::LocalFile
        }
        fn location(&self) -> String {
            "mem".into()
        }
        fn detail(&self) -> String {
            "mem".into()
        }
        fn load(&self) -> Result<Option<LeaseState>, StoreError> {
            if self.corrupt {
                return Err(StoreError::LeaseParse("garbage".into()));
            }
            Ok(self.stored.lock().clone())
        }
        fn save(&self, state: &LeaseState) -> Result<(), StoreError> {
            if self.fail_saves {
                return Err(StoreError::Unavailable);
            }
            *self.saves.lock() += 1;
            *self.stored.lock() = Some(state.clone());
            Ok(())
        }
    }

    fn entitlements(quota: Quota, allowed_methods: AllowedMethods) -> Entitlements {
        let now = OffsetDateTime::now_utc();
        Entitlements {
            customer_id: "c".into(),
            customer_name: "C".into(),
            quota,
            scope: wipe_license::Scope::Site {
                site_id: "s".into(),
            },
            not_before: now - Duration::days(30),
            not_after: now + Duration::days(30),
            features: vec![],
            allowed_methods,
            machine_binding: None,
            paid_through: None,
        }
    }

    fn chain_with(license_id: &str, ent: Entitlements) -> AttestationChain {
        let cert = wipe_license::VendorRoot::generate()
            .issue(license_id, "ed25519:k", ent, OffsetDateTime::now_utc())
            .unwrap();
        AttestationChain::new(cert)
    }

    fn chain(license_id: &str) -> AttestationChain {
        chain_with(
            license_id,
            entitlements(Quota::Unlimited, AllowedMethods::All),
        )
    }

    fn count_chain(license_id: &str, erasures: u64) -> AttestationChain {
        chain_with(
            license_id,
            entitlements(Quota::Count { erasures }, AllowedMethods::All),
        )
    }

    fn stored(license_id: Option<&str>, used: u64, evaluation: u64) -> LeaseState {
        let mut s = LeaseState::new(OffsetDateTime::now_utc() - Duration::hours(1));
        s.license_id = license_id.map(str::to_owned);
        s.erasures_used = used;
        s.evaluation_erasures = evaluation;
        s
    }

    fn erase() -> Method {
        Method::BlockOverwrite {
            passes: 1,
            pattern: wipe_common::Pattern::Zeros,
        }
    }

    fn real(drives: u32) -> StartRequest {
        StartRequest {
            method: Some(erase()),
            drives,
            simulated: false,
        }
    }

    fn simulated() -> StartRequest {
        StartRequest {
            simulated: true,
            ..real(1)
        }
    }

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    /// A licensed context with a fresh lease in a working RAM store.
    fn ctx_with(chain: AttestationChain) -> LicenseContext {
        LicenseContext::open("st", Some(chain), MemStore::holding(None), now())
    }

    #[test]
    fn a_different_licence_restarts_the_licensed_count_and_keeps_the_rest() {
        let before = stored(Some("lic-a"), 5, 3);
        let store = MemStore::holding(Some(before.clone()));
        let ctx = LicenseContext::open(
            "st",
            Some(chain("lic-a")),
            store.clone(),
            before.time_watermark,
        );
        assert_eq!(ctx.lease_state().erasures_used, 5);

        let next = ctx.with_chain(chain("lic-b"));
        let lease = next.lease_state();
        assert_eq!(lease.erasures_used, 0);
        assert_eq!(lease.license_id.as_deref(), Some("lic-b"));
        assert_eq!(lease.evaluation_erasures, 3);
        assert_eq!(lease.time_watermark, before.time_watermark);
        assert_eq!(next.fingerprint(), ctx.fingerprint());
        assert_eq!(
            next.entitlements().map(|e| e.customer_id.as_str()),
            Some("c")
        );
    }

    #[test]
    fn the_same_licence_keeps_the_licensed_count() {
        let before = stored(Some("lic-a"), 5, 0);
        let store = MemStore::holding(Some(before.clone()));
        let ctx = LicenseContext::open("st", Some(chain("lic-a")), store, before.time_watermark);
        assert_eq!(
            ctx.with_chain(chain("lic-a")).lease_state().erasures_used,
            5
        );
    }

    #[test]
    fn an_unlicensed_station_takes_a_licence_with_its_evaluation_count() {
        let before = stored(None, 0, 7);
        let store = MemStore::holding(Some(before.clone()));
        let ctx = LicenseContext::open("st", None, store, before.time_watermark);
        let next = ctx.with_chain(chain("lic-a"));
        let lease = next.lease_state();
        assert_eq!(lease.evaluation_erasures, 7);
        assert_eq!(lease.erasures_used, 0);
        assert_eq!(lease.license_id.as_deref(), Some("lic-a"));
        assert!(matches!(
            next.view(now(), false).status,
            LeaseStatus::Valid { .. }
        ));
    }

    #[test]
    fn both_contexts_share_one_lease_and_one_writer() {
        let store = MemStore::holding(None);
        let ctx = LicenseContext::open("st", None, store.clone(), now());
        let next = ctx.with_chain(chain("lic-a"));
        // A settlement still holding the old context counts into the same lease.
        ctx.record_evaluation_erasure();
        let decision = next.decide_start(&real(1), now()).unwrap();
        ctx.spend(&decision);
        let lease = next.lease_state();
        assert_eq!(lease.evaluation_erasures, 1);
        assert_eq!(lease.erasures_used, 1);
        assert_eq!(lease.credits_reserved, 0);
        assert_eq!(ctx.lease_state(), lease);
        assert_eq!(store.stored.lock().clone(), Some(lease));
    }

    #[test]
    fn a_load_fault_is_inherited_and_the_file_is_never_written() {
        let store = MemStore::corrupt();
        let ctx = LicenseContext::open("st", None, store.clone(), now());
        let next = ctx.with_chain(chain("lic-a"));
        assert!(matches!(
            next.view(now(), false).status,
            LeaseStatus::LeaseStateUnavailable { .. }
        ));
        // An unlimited licence still starts licensed Jobs (decision 5).
        let decision = next.decide_start(&real(1), now()).unwrap();
        assert!(!decision.evaluation);
        next.spend(&decision);
        assert_eq!(*store.saves.lock(), 0);
    }

    // --- the start gate -----------------------------------------------------

    #[test]
    fn a_covered_start_is_licensed_and_reserves_one_credit_per_drive() {
        let ctx = ctx_with(count_chain("lic-a", 5));
        let at = now();
        let decision = ctx.decide_start(&real(2), at).unwrap();
        assert!(!decision.evaluation);
        assert_eq!(decision.license_id.as_deref(), Some("lic-a"));
        assert_eq!(decision.credits_reserved, 2);
        assert_eq!(decision.decided_at, at);
        let chain: AttestationChain =
            serde_json::from_value(decision.attestation.clone().unwrap()).unwrap();
        assert_eq!(chain.license.body.license_id, "lic-a");
        let lease = ctx.lease_state();
        assert_eq!(lease.credits_reserved, 2);
        assert_eq!(lease.erasures_used, 0);
        assert!(matches!(
            ctx.view(now(), false).status,
            LeaseStatus::Valid {
                remaining_erasures: Some(3),
                ..
            }
        ));
    }

    #[test]
    fn spend_moves_the_reservation_to_used_and_release_returns_it() {
        let ctx = ctx_with(count_chain("lic-a", 5));
        let spent = ctx.decide_start(&real(1), now()).unwrap();
        let aborted = ctx.decide_start(&real(2), now()).unwrap();
        assert_eq!(ctx.lease_state().credits_reserved, 3);
        ctx.spend(&spent);
        ctx.release(&aborted);
        let lease = ctx.lease_state();
        assert_eq!(lease.erasures_used, 1);
        assert_eq!(lease.credits_reserved, 0);
        // Evaluation decisions settle nothing.
        ctx.spend(&JobLicence::evaluation(now()));
        ctx.release(&JobLicence::evaluation(now()));
        assert_eq!(ctx.lease_state(), lease);
    }

    #[test]
    fn no_licence_refuses_on_a_real_backend_and_is_evaluation_on_a_simulated_one() {
        let ctx = LicenseContext::open("st", None, MemStore::holding(None), now());
        assert_eq!(
            ctx.decide_start(&real(1), now()),
            Err(StartRefusal::NoLicence)
        );
        let decision = ctx.decide_start(&simulated(), now()).unwrap();
        assert!(decision.evaluation);
        assert!(decision.attestation.is_none());
        assert_eq!(decision.credits_reserved, 0);
    }

    #[test]
    fn each_licence_check_refuses_with_its_own_reason() {
        let mut expired = entitlements(Quota::Unlimited, AllowedMethods::All);
        expired.not_after = now() - Duration::days(2);
        let mut early = entitlements(Quota::Unlimited, AllowedMethods::All);
        early.not_before = now() + Duration::days(2);
        let mut bound = entitlements(Quota::Unlimited, AllowedMethods::All);
        bound.machine_binding = Some("mf1:elsewhere".into());
        for (ent, want) in [
            (expired.clone(), "expired on"),
            (early, "not valid until"),
            (bound, "bound to machine mf1:elsewhere"),
        ] {
            let ctx = ctx_with(chain_with("lic-a", ent));
            let refusal = ctx.decide_start(&real(1), now()).unwrap_err();
            assert!(refusal.message().contains(want), "{refusal}");
            assert_eq!(ctx.lease_state().credits_reserved, 0);
            assert!(ctx.decide_start(&simulated(), now()).unwrap().evaluation);
        }
        let ctx = ctx_with(chain_with("lic-a", expired.clone()));
        let msg = ctx.decide_start(&real(1), now()).unwrap_err().message();
        assert!(msg.contains(&expired.not_after.date().to_string()), "{msg}");
        assert!(msg.contains("Install the renewed licence"), "{msg}");
    }

    #[test]
    fn a_clock_moved_back_refuses_and_does_not_lower_the_watermark() {
        let ctx = ctx_with(chain("lic-a"));
        let later = now() + Duration::hours(2);
        ctx.decide_start(&real(1), later).unwrap();
        let rewound = later - Duration::hours(1);
        let refusal = ctx.decide_start(&real(1), rewound).unwrap_err();
        assert!(
            matches!(refusal, StartRefusal::ClockRollback { .. }),
            "{refusal}"
        );
        assert!(refusal.message().contains("Correct the system clock"));
        assert_eq!(ctx.lease_state().time_watermark, later);
    }

    #[test]
    fn a_method_outside_the_allow_list_or_unresolved_is_refused() {
        let only = AllowedMethods::Only {
            classes: vec![MethodClass::BlockOverwrite],
        };
        let ctx = ctx_with(chain_with("lic-a", entitlements(Quota::Unlimited, only)));
        let nvme = StartRequest {
            method: Some(Method::NvmeSanitizeBlockErase {
                ause: false,
                no_deallocate: false,
            }),
            ..real(1)
        };
        assert_eq!(
            ctx.decide_start(&nvme, now()),
            Err(StartRefusal::MethodNotAllowed {
                class: MethodClass::NvmeSanitize
            })
        );
        assert!(ctx
            .decide_start(&nvme, now())
            .unwrap_err()
            .message()
            .contains("NVMe Sanitize"));
        let unresolved = StartRequest {
            method: None,
            ..real(1)
        };
        assert_eq!(
            ctx.decide_start(&unresolved, now()),
            Err(StartRefusal::MethodUnresolvable)
        );
        assert!(!ctx.decide_start(&real(1), now()).unwrap().evaluation);
        // Under `All`, an unresolved method is not a reason.
        let all = ctx_with(chain("lic-b"));
        assert!(!all.decide_start(&unresolved, now()).unwrap().evaluation);
    }

    #[test]
    fn too_few_credits_refuses_naming_what_is_needed_and_left() {
        let ctx = ctx_with(count_chain("lic-a", 2));
        assert_eq!(
            ctx.decide_start(&real(3), now()),
            Err(StartRefusal::NoCredits { needed: 3, left: 2 })
        );
        assert!(ctx
            .decide_start(&real(3), now())
            .unwrap_err()
            .message()
            .contains("needs 3 credits"));
        ctx.decide_start(&real(2), now()).unwrap();
        let none_left = ctx.decide_start(&real(1), now()).unwrap_err();
        assert_eq!(none_left, StartRefusal::NoCredits { needed: 1, left: 0 });
        assert!(none_left.message().contains("no credits left"));
        // Simulated: never refused, evaluation, nothing reserved.
        assert!(ctx.decide_start(&simulated(), now()).unwrap().evaluation);
        assert_eq!(ctx.lease_state().credits_reserved, 2);
    }

    #[test]
    fn concurrent_starts_cannot_both_take_the_last_credit() {
        let ctx = Arc::new(ctx_with(count_chain("lic-a", 1)));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let ctx = ctx.clone();
                std::thread::spawn(move || ctx.decide_start(&real(1), now()).is_ok())
            })
            .collect();
        let granted = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert_eq!(granted, 1);
        assert_eq!(ctx.lease_state().credits_reserved, 1);
    }

    #[test]
    fn a_count_reservation_that_is_not_saved_is_undone_and_refused() {
        let ctx = LicenseContext::open(
            "st",
            Some(count_chain("lic-a", 5)),
            MemStore::failing_saves(),
            now(),
        );
        let refusal = ctx.decide_start(&real(1), now()).unwrap_err();
        match &refusal {
            StartRefusal::LeaseUnavailable { detail } => {
                assert!(detail.contains("mem"), "{detail}")
            }
            other => panic!("expected LeaseUnavailable, got {other:?}"),
        }
        assert_eq!(ctx.lease_state().credits_reserved, 0);
        assert!(ctx.decide_start(&simulated(), now()).unwrap().evaluation);
        assert_eq!(ctx.lease_state().credits_reserved, 0);
        assert!(ctx.view(now(), false).new_jobs_refused.is_some());
    }

    #[test]
    fn a_count_licence_over_an_unreadable_lease_refuses() {
        let ctx = LicenseContext::open(
            "st",
            Some(count_chain("lic-a", 5)),
            MemStore::corrupt(),
            now(),
        );
        assert!(matches!(
            ctx.decide_start(&real(1), now()),
            Err(StartRefusal::LeaseUnavailable { .. })
        ));
    }

    #[test]
    fn an_unlimited_licence_with_failing_saves_stays_licensed() {
        let ctx =
            LicenseContext::open("st", Some(chain("lic-a")), MemStore::failing_saves(), now());
        let decision = ctx.decide_start(&real(1), now()).unwrap();
        assert!(!decision.evaluation);
        assert!(decision.attestation.is_some());
        let view = ctx.view(now(), false);
        assert!(matches!(
            view.status,
            LeaseStatus::LeaseStateUnavailable { .. }
        ));
        assert_eq!(view.new_jobs_refused, None);
    }

    #[test]
    fn a_restart_returns_every_stored_reservation() {
        let store = MemStore::holding(None);
        let ctx = LicenseContext::open("st", Some(count_chain("lic-a", 5)), store.clone(), now());
        let decision = ctx.decide_start(&real(2), now()).unwrap();
        ctx.spend(&JobLicence {
            credits_reserved: 1,
            ..decision
        });
        let on_disk = store.stored.lock().clone().unwrap();
        assert_eq!((on_disk.erasures_used, on_disk.credits_reserved), (1, 1));
        drop(ctx);
        let again = LicenseContext::open("st", Some(count_chain("lic-a", 5)), store, now());
        let lease = again.lease_state();
        assert_eq!((lease.erasures_used, lease.credits_reserved), (1, 0));
    }

    #[test]
    fn a_licence_switch_clears_reservations_and_old_decisions_settle_nothing() {
        let ctx = ctx_with(count_chain("lic-a", 5));
        let old = ctx.decide_start(&real(2), now()).unwrap();
        let next = ctx.with_chain(count_chain("lic-b", 3));
        assert_eq!(next.lease_state().credits_reserved, 0);
        let fresh = next.decide_start(&real(1), now()).unwrap();
        next.spend(&old);
        next.release(&old);
        let lease = next.lease_state();
        assert_eq!((lease.erasures_used, lease.credits_reserved), (0, 1));
        next.spend(&fresh);
        assert_eq!(next.lease_state().erasures_used, 1);
    }

    #[test]
    fn the_view_predicts_the_status_refusal_only_on_a_real_backend() {
        let mut ent = entitlements(Quota::Unlimited, AllowedMethods::All);
        ent.not_after = now() - Duration::days(1);
        let ctx = ctx_with(chain_with("lic-a", ent));
        let view = ctx.view(now(), false);
        let refused = ctx.decide_start(&real(1), now()).unwrap_err().message();
        assert_eq!(view.new_jobs_refused.as_deref(), Some(refused.as_str()));
        assert!(!view.simulated_backend);
        let sim = ctx.view(now(), true);
        assert_eq!(sim.new_jobs_refused, None);
        assert!(sim.simulated_backend);
        let unlicensed = LicenseContext::unlicensed("st", now());
        assert_eq!(
            unlicensed.view(now(), false).new_jobs_refused,
            Some(StartRefusal::NoLicence.message())
        );
        assert_eq!(
            ctx_with(chain("lic-b")).view(now(), false).new_jobs_refused,
            None
        );
    }

    #[test]
    fn every_refusal_names_a_fix() {
        let t = now();
        for refusal in [
            StartRefusal::NoLicence,
            StartRefusal::Expired { not_after: t },
            StartRefusal::NotYetValid { not_before: t },
            StartRefusal::WrongMachine {
                expected: "a".into(),
                actual: "b".into(),
            },
            StartRefusal::ClockRollback {
                watermark: t,
                observed: t,
            },
            StartRefusal::MethodNotAllowed {
                class: MethodClass::Destroy,
            },
            StartRefusal::MethodUnresolvable,
            StartRefusal::NoCredits { needed: 1, left: 0 },
            StartRefusal::NoCredits { needed: 3, left: 2 },
            StartRefusal::NoPoolCredits { needed: 1, left: 0 },
            StartRefusal::NoPoolCredits { needed: 3, left: 2 },
            StartRefusal::LeaseUnavailable { detail: "x".into() },
        ] {
            let msg = refusal.message();
            assert!(msg.contains(". "), "{msg}");
            assert!(msg.ends_with('.'), "{msg}");
            assert!(
                [
                    "Install",
                    "Correct",
                    "Check",
                    "Add",
                    "Fix",
                    "Connect",
                    "Start new jobs from"
                ]
                .iter()
                .any(|fix| msg.contains(fix)),
                "{msg}"
            );
        }
    }

    fn pool_chain(license_id: &str) -> AttestationChain {
        chain_with(license_id, entitlements(Quota::Pool, AllowedMethods::All))
    }

    fn with_grant(credits: u64) -> LeaseState {
        let mut s = stored(None, 0, 0);
        s.credit_grants.push(wipe_license::HeldGrant {
            grant_id: "g-1".into(),
            organization_id: "c".into(),
            credits,
            spent: 0,
        });
        s
    }

    #[test]
    fn a_pool_start_reserves_from_the_reserve_and_settles_by_organization() {
        let ctx = LicenseContext::open(
            "st",
            Some(pool_chain("lic-a")),
            MemStore::holding(Some(with_grant(3))),
            now(),
        );
        let d = ctx.decide_start(&real(2), now()).unwrap();
        assert_eq!(d.credit_pool.as_deref(), Some("c"));
        assert_eq!(d.credits_reserved, 2);
        assert_eq!(ctx.lease_state().pool_credits_reserved, 2);
        assert_eq!(ctx.lease_state().credits_reserved, 0);
        assert_eq!(
            ctx.decide_start(&real(2), now()).unwrap_err(),
            StartRefusal::NoPoolCredits { needed: 2, left: 1 }
        );
        // A licence switch keeps the reserve and the reservation.
        let switched = ctx.with_chain(pool_chain("lic-b"));
        assert_eq!(switched.lease_state().pool_credits_reserved, 2);
        switched.spend(&d);
        let l = switched.lease_state();
        assert_eq!((l.pool_credits_reserved, l.credit_grants[0].spent), (0, 2));
        assert_eq!(
            switched.view(now(), false).status,
            LeaseStatus::Valid {
                remaining_erasures: Some(1),
                days_remaining: 29
            }
        );
        // An abort returns it.
        let d = switched.decide_start(&real(1), now()).unwrap();
        switched.release(&d);
        assert_eq!(switched.lease_state().pool_credits_reserved, 0);
        assert_eq!(switched.pool_reserve().unwrap().available, 1);
    }

    #[test]
    fn a_pool_reservation_that_is_not_saved_is_undone_and_refused() {
        let ctx = LicenseContext::open(
            "st",
            Some(pool_chain("lic-a")),
            MemStore::failing_saves(),
            now(),
        );
        // In RAM only: the grant cannot be saved either.
        let g = wipe_license::VendorRoot::generate();
        let grant = g
            .issue_credit_grant("g-1", "c", "ed25519:k", 5, now())
            .unwrap();
        let intake = ctx
            .add_grant(&grant, "ed25519:k", &[g.verifying_key()])
            .unwrap();
        assert_eq!(intake.added, wipe_license::GrantAdded::Added);
        assert!(intake.saved.is_err());
        assert!(matches!(
            ctx.decide_start(&real(1), now()).unwrap_err(),
            StartRefusal::LeaseUnavailable { .. }
        ));
        assert_eq!(ctx.lease_state().pool_credits_reserved, 0);
        // A grant for another key, or from another root, is refused.
        assert!(ctx
            .add_grant(&grant, "ed25519:other", &[g.verifying_key()])
            .is_err());
        assert!(ctx
            .add_grant(
                &grant,
                "ed25519:k",
                &[wipe_license::VendorRoot::generate().verifying_key()]
            )
            .is_err());
    }

    #[test]
    fn open_returns_pool_reservations() {
        let mut s = with_grant(5);
        s.pool_credits_reserved = 4;
        let ctx = LicenseContext::open(
            "st",
            Some(pool_chain("lic-a")),
            MemStore::holding(Some(s)),
            now(),
        );
        assert_eq!(ctx.lease_state().pool_credits_reserved, 0);
        assert_eq!(ctx.pool_reserve().unwrap().available, 5);
        assert_eq!(
            LicenseContext::open(
                "st",
                Some(count_chain("lic-a", 5)),
                MemStore::holding(None),
                now()
            )
            .pool_reserve(),
            None
        );
    }

    #[test]
    fn the_overdue_line_shows_between_the_paid_through_date_and_the_licence_end() {
        let now = OffsetDateTime::now_utc();
        let mut ent = entitlements(Quota::Unlimited, AllowedMethods::All);
        // No paid-through date: never overdue.
        assert_eq!(renewal_overdue(&ent, now), None);
        // Paid through yesterday, ends in 29 days.
        let paid_through = now - Duration::days(1);
        ent.paid_through = Some(paid_through);
        ent.not_after = now + Duration::days(29);
        let line = renewal_overdue(&ent, now).unwrap();
        assert_eq!(
            line,
            format!(
                "Renewal payment overdue since {}. New jobs will be refused from {} unless it \
                 is paid.",
                paid_through.date(),
                ent.not_after.date()
            )
        );
        // Not yet due, or already ended: none.
        ent.paid_through = Some(now + Duration::days(1));
        assert_eq!(renewal_overdue(&ent, now), None);
        ent.paid_through = Some(now - Duration::days(31));
        ent.not_after = now - Duration::days(1);
        assert_eq!(renewal_overdue(&ent, now), None);

        // On the view: a real backend shows it, a simulated one never does.
        let mut ent = entitlements(Quota::Unlimited, AllowedMethods::All);
        ent.paid_through = Some(paid_through);
        let ctx = ctx_with(chain_with("lic-g", ent));
        let real = ctx.view(now, false);
        assert_eq!(
            real.paid_through.map(|t| t.unix_timestamp()),
            Some(paid_through.unix_timestamp())
        );
        assert!(real.renewal_overdue.is_some());
        let simulated = ctx.view(now, true);
        assert_eq!(simulated.renewal_overdue, None);
        assert!(simulated.paid_through.is_some());
    }
}
