//! The lapse policy in the signed licence body (ADR-0012).
//!
//! Back-compat is the property that matters: every licence file signed before
//! the field existed must keep verifying, byte for byte, and read as
//! `evaluation`. A `stop` licence must carry the field under the signature.

use time::{Duration, OffsetDateTime};

use wipe_cert::{SigningKey, VerifyingKey};
use wipe_license::{
    check_installable, AllowedMethods, Entitlements, Feature, LapsePolicy, LicenseCertificate,
    LicenseError, Quota, Scope, VendorRoot,
};

/// A licence file written by the code before `lapse_policy` existed
/// (baseline c0d892d), signed by `VendorRoot::from_signing_key(from_seed([7; 32]))`
/// for the instance key `from_seed([9; 32])`.
const OLD_FIXTURE: &str = include_str!("fixtures/licence-before-lapse-policy.json");
const OLD_FIXTURE_ROOT_HEX: &str =
    "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c";

fn fixture_root() -> VerifyingKey {
    let bytes: [u8; 32] = hex::decode(OLD_FIXTURE_ROOT_HEX)
        .unwrap()
        .try_into()
        .unwrap();
    VerifyingKey::from_bytes(&bytes).unwrap()
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_780_000_000).unwrap()
}

fn entitlements() -> Entitlements {
    Entitlements {
        customer_id: "org-1".into(),
        customer_name: "Northwind ITAD".into(),
        quota: Quota::Unlimited,
        scope: Scope::Site {
            site_id: "site-1".into(),
        },
        not_before: now() - Duration::days(1),
        not_after: now() + Duration::days(364),
        features: vec![Feature::HubSync],
        allowed_methods: AllowedMethods::All,
        machine_binding: None,
    }
}

fn issue(root: &VendorRoot, policy: LapsePolicy) -> (SigningKey, LicenseCertificate) {
    let instance = SigningKey::generate();
    let licence = root
        .issue_with_lapse_policy(
            "lic-1",
            instance.public_key_id(),
            entitlements(),
            now(),
            policy,
        )
        .unwrap();
    (instance, licence)
}

#[test]
fn a_licence_signed_before_the_field_existed_still_verifies_and_reads_as_evaluation() {
    let licence: LicenseCertificate = serde_json::from_str(OLD_FIXTURE).unwrap();

    assert_eq!(
        licence.verify(&[fixture_root()]).unwrap(),
        fixture_root().public_key_id()
    );
    assert_eq!(licence.body.lapse_policy, LapsePolicy::Evaluation);
    assert_eq!(
        check_installable(
            &licence,
            &SigningKey::from_seed([9; 32]).public_key_id(),
            &[fixture_root()]
        ),
        Ok(())
    );
    // Re-serialized exactly as `save_license` writes it: byte-identical.
    assert_eq!(serde_json::to_string_pretty(&licence).unwrap(), OLD_FIXTURE);
}

#[test]
fn the_fixture_root_is_the_seeded_root_it_claims() {
    let root = VendorRoot::from_signing_key(SigningKey::from_seed([7; 32]));
    assert_eq!(
        root.verifying_key().public_key_id(),
        fixture_root().public_key_id()
    );
}

#[test]
fn issue_and_an_evaluation_policy_produce_the_same_bytes_with_no_field() {
    let root = VendorRoot::from_signing_key(SigningKey::from_seed([3; 32]));
    let instance = SigningKey::from_seed([4; 32]).public_key_id();
    let plain = root
        .issue("lic-1", instance.clone(), entitlements(), now())
        .unwrap();
    let explicit = root
        .issue_with_lapse_policy(
            "lic-1",
            instance,
            entitlements(),
            now(),
            LapsePolicy::Evaluation,
        )
        .unwrap();

    assert_eq!(plain, explicit);
    let value = serde_json::to_value(&plain).unwrap();
    assert!(value["body"].get("lapse_policy").is_none(), "{value}");
}

#[test]
fn a_stop_licence_carries_the_field_and_round_trips() {
    let root = VendorRoot::generate();
    let (instance, licence) = issue(&root, LapsePolicy::Stop);

    let value = serde_json::to_value(&licence).unwrap();
    assert_eq!(value["body"]["lapse_policy"], "stop");

    let text = serde_json::to_string_pretty(&licence).unwrap();
    let parsed: LicenseCertificate = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed.body.lapse_policy, LapsePolicy::Stop);
    assert!(parsed.verify(&[root.verifying_key()]).is_ok());
    assert_eq!(
        check_installable(&parsed, &instance.public_key_id(), &[root.verifying_key()]),
        Ok(())
    );
}

#[test]
fn removing_the_policy_from_a_stop_licence_is_tampering() {
    let root = VendorRoot::generate();
    let (_, licence) = issue(&root, LapsePolicy::Stop);

    let mut value = serde_json::to_value(&licence).unwrap();
    value["body"]
        .as_object_mut()
        .unwrap()
        .remove("lapse_policy");
    let stripped: LicenseCertificate = serde_json::from_value(value).unwrap();

    assert_eq!(stripped.body.lapse_policy, LapsePolicy::Evaluation);
    assert!(matches!(
        stripped.verify(&[root.verifying_key()]),
        Err(LicenseError::Tampered(_))
    ));
}

#[test]
fn adding_stop_to_an_evaluation_licence_is_tampering() {
    let root = VendorRoot::generate();
    let (_, mut licence) = issue(&root, LapsePolicy::Evaluation);
    licence.body.lapse_policy = LapsePolicy::Stop;
    assert!(matches!(
        licence.verify(&[root.verifying_key()]),
        Err(LicenseError::Tampered(_))
    ));
}

#[test]
fn an_unknown_policy_value_does_not_parse() {
    let root = VendorRoot::generate();
    let (_, licence) = issue(&root, LapsePolicy::Stop);
    let mut value = serde_json::to_value(&licence).unwrap();
    value["body"]["lapse_policy"] = "sometimes".into();
    assert!(serde_json::from_value::<LicenseCertificate>(value).is_err());
}

#[test]
fn the_wire_spelling_matches_parse() {
    for policy in [LapsePolicy::Evaluation, LapsePolicy::Stop] {
        assert_eq!(
            serde_json::to_value(policy).unwrap(),
            serde_json::Value::from(policy.as_str())
        );
        assert_eq!(LapsePolicy::parse(policy.as_str()), Some(policy));
    }
    assert_eq!(LapsePolicy::parse("sometimes"), None);
    assert_eq!(LapsePolicy::default(), LapsePolicy::Evaluation);
}
