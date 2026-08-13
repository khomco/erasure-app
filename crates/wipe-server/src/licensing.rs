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

use parking_lot::RwLock;
use time::OffsetDateTime;

use wipe_license::{
    evaluate, machine_fingerprint, AttestationChain, Entitlements, LeaseState, LeaseStatus,
};

pub struct LicenseContext {
    chain: Option<AttestationChain>,
    fingerprint: String,
    lease: RwLock<LeaseState>,
}

impl LicenseContext {
    /// An unlicensed station. The free-tier default.
    pub fn unlicensed(station_id: &str, now: OffsetDateTime) -> Self {
        Self {
            chain: None,
            fingerprint: machine_fingerprint(station_id),
            lease: RwLock::new(LeaseState::new(now)),
        }
    }

    pub fn licensed(station_id: &str, chain: AttestationChain, lease: LeaseState) -> Self {
        Self {
            chain: Some(chain),
            fingerprint: machine_fingerprint(station_id),
            lease: RwLock::new(lease),
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

    /// Fold an observed clock reading into the anti-rollback watermark.
    ///
    /// Called on every evaluation so the watermark tracks the highest time
    /// the station has ever seen, which is what makes a later backwards jump
    /// detectable at all.
    pub fn observe_now(&self, now: OffsetDateTime) {
        self.lease.write().observe(now);
    }

    /// Record a successful erasure against the local usage count.
    ///
    /// Best-effort by construction: this is a local counter, and ADR-0005 §3
    /// is explicit that offline consumption is not provable.
    pub fn record_erasure(&self) {
        let mut lease = self.lease.write();
        lease.erasures_used = lease.erasures_used.saturating_add(1);
    }

    pub fn status(&self, now: OffsetDateTime) -> LeaseStatus {
        // Evaluate against the watermark *before* folding `now` in, or a
        // rolled-back clock would silently become the new normal.
        let state = self.lease.read().clone();
        let status = evaluate(self.entitlements(), &state, &self.fingerprint, now);
        drop(state);
        self.observe_now(now);
        status
    }

    /// What the certificate generator needs: whether to mark the cert as
    /// evaluation, and the attestation chain to staple on if not.
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
