//! Attestation chain, entitlements and offline lease enforcement (ADR-0005).
//!
//! The property that matters most is negative: it must be impossible to end
//! up with a certificate that *looks* licensed without a vendor signature
//! over the entitlements that names the exact key which signed it.

use time::{Duration, OffsetDateTime};

use wipe_cert::SigningKey;
use wipe_license::{
    evaluate, feature_available, machine_fingerprint, verify_chain, AllowedMethods,
    AttestationChain, ChainVerdict, Entitlements, Feature, LeaseState, LeaseStatus, LicenseError,
    MethodClass, MonotonicCounter, Quota, Scope, TpmCounter, VendorRoot,
};

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_780_000_000).unwrap()
}

fn entitlements(quota: Quota, scope: Scope) -> Entitlements {
    Entitlements {
        customer_id: "cust-42".into(),
        customer_name: "Northwind ITAD".into(),
        quota,
        scope,
        not_before: now() - Duration::days(30),
        not_after: now() + Duration::days(335),
        features: vec![Feature::EnterpriseMode, Feature::HubSync],
        allowed_methods: AllowedMethods::All,
        machine_binding: None,
    }
}

fn unlimited() -> Entitlements {
    entitlements(
        Quota::Unlimited,
        Scope::Site {
            site_id: "site-1".into(),
        },
    )
}

/// Issue a licence for a freshly generated instance key.
fn issue(root: &VendorRoot, ent: Entitlements) -> (SigningKey, AttestationChain) {
    let instance = SigningKey::generate();
    let license = root
        .issue("lic-0001", instance.public_key_id(), ent, now())
        .expect("issue");
    (instance, AttestationChain::new(license))
}

// --- the chain -------------------------------------------------------------

#[test]
fn a_full_chain_verifies_to_the_vendor_root() {
    let root = VendorRoot::generate();
    let (instance, chain) = issue(&root, unlimited());

    let verdict = verify_chain(
        Some(&chain),
        &instance.public_key_id(),
        now(),
        &[root.verifying_key()],
    );

    match verdict {
        ChainVerdict::Licensed {
            customer_id,
            customer_name,
            license_id,
            expired_at_signing,
            ..
        } => {
            assert_eq!(customer_id, "cust-42");
            assert_eq!(customer_name, "Northwind ITAD");
            assert_eq!(license_id, "lic-0001");
            assert!(!expired_at_signing);
        }
        other => panic!("expected Licensed, got {other:?}"),
    }
}

#[test]
fn a_licence_stapled_to_a_cert_signed_by_a_different_key_is_invalid() {
    // The whole point of the chain. Without this link, anyone could take a
    // genuine licence and attach it to certificates signed by any key at all.
    let root = VendorRoot::generate();
    let (_licensed_instance, chain) = issue(&root, unlimited());
    let impostor = SigningKey::generate();

    let verdict = verify_chain(
        Some(&chain),
        &impostor.public_key_id(),
        now(),
        &[root.verifying_key()],
    );

    assert!(verdict.is_invalid(), "got {verdict:?}");
    if let ChainVerdict::Invalid { reason } = verdict {
        assert!(reason.contains("entitles instance key"), "{reason}");
    }
}

#[test]
fn a_licence_from_an_unknown_root_is_invalid() {
    let real_root = VendorRoot::generate();
    let other_root = VendorRoot::generate();
    let (instance, chain) = issue(&other_root, unlimited());

    let verdict = verify_chain(
        Some(&chain),
        &instance.public_key_id(),
        now(),
        &[real_root.verifying_key()],
    );
    assert!(verdict.is_invalid(), "got {verdict:?}");
}

#[test]
fn editing_entitlements_breaks_the_vendor_signature() {
    // Entitlements are only uneditable because they sit inside the signature.
    // This is the test that claim rests on.
    let root = VendorRoot::generate();
    let (instance, mut chain) = issue(&root, unlimited());

    chain.license.body.entitlements.quota = Quota::Count {
        erasures: 1_000_000,
    };

    let verdict = verify_chain(
        Some(&chain),
        &instance.public_key_id(),
        now(),
        &[root.verifying_key()],
    );
    assert!(verdict.is_invalid(), "got {verdict:?}");

    // And directly, so the error type is pinned as Tampered rather than a
    // generic verification failure.
    match chain.license.verify(&[root.verifying_key()]) {
        Err(LicenseError::Tampered(_)) => {}
        other => panic!("expected Tampered, got {other:?}"),
    }
}

#[test]
fn swapping_the_licensed_instance_key_breaks_the_signature() {
    let root = VendorRoot::generate();
    let (_i, mut chain) = issue(&root, unlimited());
    let attacker = SigningKey::generate();
    chain.license.body.instance_public_key_id = attacker.public_key_id();

    // Re-pointing the licence at another key is itself a payload edit.
    assert!(chain.license.verify(&[root.verifying_key()]).is_err());
}

#[test]
fn no_chain_is_unlicensed_not_an_error() {
    // Free tier (ADR-0005 §5): a valid, self-signed evaluation certificate.
    // "Unlicensed" and "tampered" must never collapse into one outcome.
    let root = VendorRoot::generate();
    let instance = SigningKey::generate();
    let verdict = verify_chain(
        None,
        &instance.public_key_id(),
        now(),
        &[root.verifying_key()],
    );
    assert_eq!(verdict, ChainVerdict::Unlicensed);
    assert!(!verdict.is_invalid());
    assert!(!verdict.is_licensed());
}

#[test]
fn an_expired_licence_still_verifies_but_is_reported_as_expired() {
    // The erasure was real and the chain is authentic; the licence had
    // lapsed. An auditor needs both facts, so this is not an error.
    let root = VendorRoot::generate();
    let mut ent = unlimited();
    ent.not_after = now() - Duration::days(1);
    let (instance, chain) = issue(&root, ent);

    match verify_chain(
        Some(&chain),
        &instance.public_key_id(),
        now(),
        &[root.verifying_key()],
    ) {
        ChainVerdict::Licensed {
            expired_at_signing, ..
        } => assert!(expired_at_signing),
        other => panic!("expected Licensed+expired, got {other:?}"),
    }
}

#[test]
fn a_licence_round_trips_through_json() {
    let root = VendorRoot::generate();
    let (instance, chain) = issue(&root, unlimited());
    let json = serde_json::to_string(&chain).unwrap();
    let back: AttestationChain = serde_json::from_str(&json).unwrap();
    assert_eq!(back, chain);
    assert!(verify_chain(
        Some(&back),
        &instance.public_key_id(),
        now(),
        &[root.verifying_key()]
    )
    .is_licensed());
}

// --- entitlements ----------------------------------------------------------

#[test]
fn quota_counts_down_and_unlimited_never_exhausts() {
    let q = Quota::Count { erasures: 3 };
    assert!(q.permits(0) && q.permits(2));
    assert!(!q.permits(3));
    assert_eq!(q.remaining(1), Some(2));
    assert!(Quota::Unlimited.permits(u64::MAX));
    assert_eq!(Quota::Unlimited.remaining(999), None);
}

#[test]
fn allowed_methods_gate_by_class() {
    use wipe_common::{Method, Pattern};
    let only_nvme = AllowedMethods::Only {
        classes: vec![MethodClass::NvmeSanitize],
    };
    let nvme = Method::NvmeSanitizeCryptoErase {
        ause: false,
        no_deallocate: false,
    };
    let overwrite = Method::BlockOverwrite {
        pattern: Pattern::Zeros,
        passes: 1,
    };
    assert!(only_nvme.permits(&nvme));
    assert!(!only_nvme.permits(&overwrite));
    assert!(AllowedMethods::All.permits(&overwrite));
}

#[test]
fn machine_scope_implies_its_own_fingerprint_requirement() {
    let ent = entitlements(
        Quota::Unlimited,
        Scope::Machine {
            fingerprint: "mf1:abc".into(),
        },
    );
    assert_eq!(ent.required_fingerprint(), Some("mf1:abc"));

    // An explicit binding wins, so a site licence can still be pinned.
    let mut site = unlimited();
    assert_eq!(site.required_fingerprint(), None);
    site.machine_binding = Some("mf1:xyz".into());
    assert_eq!(site.required_fingerprint(), Some("mf1:xyz"));
}

#[test]
fn machine_fingerprint_is_stable_and_station_specific() {
    assert_eq!(
        machine_fingerprint("bench-1"),
        machine_fingerprint("bench-1")
    );
    assert_ne!(
        machine_fingerprint("bench-1"),
        machine_fingerprint("bench-2")
    );
    assert!(machine_fingerprint("bench-1").starts_with("mf1:"));
}

// --- offline lease ---------------------------------------------------------

fn state_at(t: OffsetDateTime, used: u64) -> LeaseState {
    let mut s = LeaseState::new(t);
    s.erasures_used = used;
    s
}

#[test]
fn a_valid_lease_reports_remaining_days_and_quota() {
    let ent = entitlements(
        Quota::Count { erasures: 100 },
        Scope::Site {
            site_id: "s".into(),
        },
    );
    let status = evaluate(Some(&ent), &state_at(now(), 40), "mf1:any", now());
    match status {
        LeaseStatus::Valid {
            remaining_erasures,
            days_remaining,
        } => {
            assert_eq!(remaining_erasures, Some(60));
            assert_eq!(days_remaining, 335);
        }
        other => panic!("expected Valid, got {other:?}"),
    }
    assert!(status.permits_licensed_signing());
}

#[test]
fn an_expired_lease_stops_licensed_signing_but_is_not_an_erasure_block() {
    let mut ent = unlimited();
    ent.not_after = now() - Duration::days(1);
    let status = evaluate(Some(&ent), &state_at(now(), 0), "mf1:any", now());
    assert!(matches!(status, LeaseStatus::Expired { .. }));
    assert!(!status.permits_licensed_signing());
    // The operator message must say erasure still works, or the UI will
    // imply the station is down.
    assert!(status.operator_message().contains("evaluation"));
}

#[test]
fn a_not_yet_valid_lease_is_distinguished_from_an_expired_one() {
    let mut ent = unlimited();
    ent.not_before = now() + Duration::days(2);
    assert!(matches!(
        evaluate(Some(&ent), &state_at(now(), 0), "mf1:any", now()),
        LeaseStatus::NotYetValid { .. }
    ));
}

#[test]
fn an_exhausted_quota_is_reported_with_both_numbers() {
    let ent = entitlements(
        Quota::Count { erasures: 5 },
        Scope::Site {
            site_id: "s".into(),
        },
    );
    match evaluate(Some(&ent), &state_at(now(), 5), "mf1:any", now()) {
        LeaseStatus::QuotaExhausted { used, allowed } => {
            assert_eq!((used, allowed), (5, 5));
        }
        other => panic!("expected QuotaExhausted, got {other:?}"),
    }
}

#[test]
fn a_licence_bound_to_another_machine_is_rejected() {
    let ent = entitlements(
        Quota::Unlimited,
        Scope::Machine {
            fingerprint: "mf1:theirs".into(),
        },
    );
    match evaluate(Some(&ent), &state_at(now(), 0), "mf1:ours", now()) {
        LeaseStatus::WrongMachine { expected, actual } => {
            assert_eq!(expected, "mf1:theirs");
            assert_eq!(actual, "mf1:ours");
        }
        other => panic!("expected WrongMachine, got {other:?}"),
    }
}

#[test]
fn no_licence_evaluates_to_unlicensed() {
    assert_eq!(
        evaluate(None, &state_at(now(), 0), "mf1:any", now()),
        LeaseStatus::Unlicensed
    );
}

// --- anti-rollback ---------------------------------------------------------

#[test]
fn the_watermark_only_moves_forward() {
    let mut s = LeaseState::new(now());
    s.observe(now() - Duration::days(10));
    assert_eq!(s.time_watermark, now(), "a past reading must not lower it");
    s.observe(now() + Duration::days(3));
    assert_eq!(s.time_watermark, now() + Duration::days(3));
}

#[test]
fn a_rolled_back_clock_is_detected_and_beats_every_other_check() {
    // Rollback is evaluated first because every other check reads the clock:
    // a licence that looks valid under a rewound clock is exactly the attack.
    let mut ent = unlimited();
    ent.not_after = now() - Duration::days(1); // also expired
    let state = state_at(now(), 0);
    let rewound = now() - Duration::days(400);

    match evaluate(Some(&ent), &state, "mf1:any", rewound) {
        LeaseStatus::ClockRollback {
            watermark,
            observed,
        } => {
            assert_eq!(watermark, now());
            assert_eq!(observed, rewound);
        }
        other => panic!("expected ClockRollback, got {other:?}"),
    }
}

#[test]
fn small_backwards_jumps_are_tolerated_as_clock_skew() {
    // NTP steps and VM suspend/resume produce small negative jumps that are
    // not attacks; treating them as such would strand honest stations.
    let ent = unlimited();
    let state = state_at(now(), 0);
    let slightly_behind = now() - Duration::minutes(2);
    assert!(matches!(
        evaluate(Some(&ent), &state, "mf1:any", slightly_behind),
        LeaseStatus::Valid { .. }
    ));
}

#[test]
fn the_file_counter_does_not_claim_hardware_backing() {
    // The guarantee is only as strong as the anchor; a counter that claimed
    // to be monotonic while living in a rewritable file would be a lie.
    let s = LeaseState::new(now());
    assert!(!s.counter_is_monotonic);
}

#[test]
fn the_tpm_counter_seam_fails_rather_than_fabricating_a_value() {
    let c = TpmCounter;
    assert!(c.is_hardware_backed());
    match c.read() {
        Err(LicenseError::Unsupported(msg)) => assert!(msg.contains("TPM")),
        other => panic!("expected Unsupported, got {other:?}"),
    }
    assert!(c.increment().is_err());
}

// --- features --------------------------------------------------------------

#[test]
fn features_need_both_a_grant_and_a_live_lease() {
    let ent = unlimited();
    let valid = evaluate(Some(&ent), &state_at(now(), 0), "mf1:any", now());
    assert!(feature_available(
        Some(&ent),
        &valid,
        Feature::EnterpriseMode
    ));
    assert!(!feature_available(
        Some(&ent),
        &valid,
        Feature::PdfCertificates
    ));

    // An expired licence must stop unlocking features, not just signing.
    let mut expired_ent = unlimited();
    expired_ent.not_after = now() - Duration::days(1);
    let expired = evaluate(Some(&expired_ent), &state_at(now(), 0), "mf1:any", now());
    assert!(!feature_available(
        Some(&expired_ent),
        &expired,
        Feature::EnterpriseMode
    ));
}

#[test]
fn an_unlicensed_station_grants_no_features() {
    assert!(!feature_available(
        None,
        &LeaseStatus::Unlicensed,
        Feature::EnterpriseMode
    ));
}

// --- installation ----------------------------------------------------------

#[test]
fn a_licence_installs_only_on_the_station_it_names() {
    // Regression for a real bug class: a perfectly valid licence issued to a
    // *different* station is not forged, it is simply not ours, and the two
    // must not report the same way.
    use wipe_license::{check_installable, InstallProblem};

    let root = VendorRoot::generate();
    let (instance, chain) = issue(&root, unlimited());
    let other = SigningKey::generate();

    assert!(check_installable(
        &chain.license,
        &instance.public_key_id(),
        &[root.verifying_key()]
    )
    .is_ok());

    match check_installable(
        &chain.license,
        &other.public_key_id(),
        &[root.verifying_key()],
    ) {
        Err(InstallProblem::WrongInstanceKey { expected, found }) => {
            assert_eq!(expected, other.public_key_id());
            assert_eq!(found, instance.public_key_id());
        }
        other => panic!("expected WrongInstanceKey, got {other:?}"),
    }
}

#[test]
fn an_untrusted_licence_is_refused_before_the_key_check() {
    // Order matters: verifying first means we never report "wrong key" about
    // a licence we have no reason to believe at all.
    use wipe_license::{check_installable, InstallProblem};

    let real = VendorRoot::generate();
    let rogue = VendorRoot::generate();
    let (instance, chain) = issue(&rogue, unlimited());

    match check_installable(
        &chain.license,
        &instance.public_key_id(),
        &[real.verifying_key()],
    ) {
        Err(InstallProblem::NotTrusted(_)) => {}
        other => panic!("expected NotTrusted, got {other:?}"),
    }
}

#[test]
fn a_licence_round_trips_through_a_file() {
    use wipe_license::{load_license, save_license};
    let root = VendorRoot::generate();
    let (_i, chain) = issue(&root, unlimited());

    let dir = std::env::temp_dir().join(format!("wipestation-lic-{}", std::process::id()));
    let path = dir.join("license.json");
    save_license(&path, &chain.license).unwrap();
    let back = load_license(&path).unwrap();
    assert_eq!(back, chain.license);
    assert!(back.verify(&[root.verifying_key()]).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}
