//! The station's credit reserve in lease state (controlled-licensing story
//! 4, ADR-0013 §3): its arithmetic, how a `Pool` licence is judged, and
//! that lease files written before it still load.

use time::{Duration, OffsetDateTime};
use wipe_cert::SigningKey;
use wipe_license::{
    evaluate, AllowedMethods, CreditGrant, Entitlements, GrantAdded, LeaseState, LeaseStatus,
    Quota, Scope, VendorRoot,
};

fn root() -> VendorRoot {
    VendorRoot::from_signing_key(SigningKey::from_seed([7; 32]))
}

fn grant(id: &str, org: &str, credits: u64) -> CreditGrant {
    root()
        .issue_credit_grant(id, org, "ed25519:k", credits, OffsetDateTime::now_utc())
        .unwrap()
}

fn pool(customer_id: &str) -> Entitlements {
    let now = OffsetDateTime::now_utc();
    Entitlements {
        customer_id: customer_id.into(),
        customer_name: "Acme".into(),
        quota: Quota::Pool,
        scope: Scope::Site {
            site_id: "s".into(),
        },
        not_before: now - Duration::days(1),
        not_after: now + Duration::days(10),
        features: vec![],
        allowed_methods: AllowedMethods::All,
        machine_binding: None,
        paid_through: None,
    }
}

#[test]
fn the_reserve_counts_only_the_organizations_grants() {
    let mut l = LeaseState::new(OffsetDateTime::now_utc());
    assert_eq!(l.add_grant(&grant("a", "org-1", 10)), GrantAdded::Added);
    assert_eq!(l.add_grant(&grant("b", "org-1", 5)), GrantAdded::Added);
    assert_eq!(l.add_grant(&grant("c", "org-2", 100)), GrantAdded::Added);
    // A grant id already held changes nothing, whatever it says.
    assert_eq!(
        l.add_grant(&grant("a", "org-1", 99)),
        GrantAdded::AlreadyHeld
    );
    assert_eq!(l.pool_held("org-1"), 15);
    assert!(l.holds_grant("c"));

    l.pool_credits_reserved = 4;
    l.spend_pool("org-1", 12);
    // Oldest first: all of `a`, then 2 of `b`.
    let spent: Vec<u64> = l.credit_grants.iter().map(|g| g.spent).collect();
    assert_eq!(spent, [10, 2, 0]);
    assert_eq!(l.pool_spent("org-1"), 12);
    assert_eq!(l.pool_balance("org-1"), 3);
    assert_eq!(l.pool_available("org-1"), 0, "reserved exceeds the balance");
    // Spending more than is held saturates.
    l.spend_pool("org-1", 50);
    assert_eq!(l.pool_balance("org-1"), 0);
    assert_eq!(l.pool_balance("org-2"), 100);
}

#[test]
fn a_pool_licence_is_judged_against_the_reserve() {
    let now = OffsetDateTime::now_utc();
    let ent = pool("org-1");
    let mut l = LeaseState::new(now);
    assert_eq!(
        evaluate(Some(&ent), &l, "fp", now),
        LeaseStatus::QuotaExhausted {
            used: 0,
            allowed: 0
        }
    );
    l.add_grant(&grant("a", "org-1", 10));
    l.add_grant(&grant("x", "org-2", 10));
    l.pool_credits_reserved = 3;
    assert!(matches!(
        evaluate(Some(&ent), &l, "fp", now),
        LeaseStatus::Valid {
            remaining_erasures: Some(7),
            ..
        }
    ));
    l.spend_pool("org-1", 7);
    assert_eq!(
        evaluate(Some(&ent), &l, "fp", now),
        LeaseStatus::QuotaExhausted {
            used: 10,
            allowed: 10
        }
    );
    // The licence's own checks come first.
    let expired = Entitlements {
        not_after: now - Duration::hours(1),
        ..pool("org-1")
    };
    assert!(matches!(
        evaluate(Some(&expired), &l, "fp", now),
        LeaseStatus::Expired { .. }
    ));
    // Pool permits nothing by count alone.
    assert!(Quota::Pool.permits(u64::MAX));
    assert_eq!(Quota::Pool.remaining(0), None);
}

#[test]
fn the_pool_quota_and_the_reserve_have_stable_wire_forms() {
    assert_eq!(
        serde_json::to_value(Quota::Pool).unwrap(),
        serde_json::json!({"kind": "pool"})
    );
    // A lease file written before the reserve existed still loads, and an
    // empty reserve writes nothing new.
    let old = r#"{"time_watermark":"2026-01-01T00:00:00Z","erasures_used":2}"#;
    let l: LeaseState = serde_json::from_str(old).unwrap();
    assert!(l.credit_grants.is_empty());
    assert_eq!(l.pool_credits_reserved, 0);
    assert_eq!(l.credit_terms, None);
    let written = serde_json::to_value(&l).unwrap();
    for key in [
        "credit_grants",
        "pool_credits_reserved",
        "last_topup_grant_id",
        "credit_terms",
    ] {
        assert!(written.get(key).is_none(), "{key} in {written}");
    }
    let mut l = l;
    l.add_grant(&grant("a", "org-1", 3));
    let back: LeaseState = serde_json::from_value(serde_json::to_value(&l).unwrap()).unwrap();
    assert_eq!(back, l);
}
