//! Credit grants (ADR-0013 §3, controlled-licensing story 3): the signed
//! document that moves credits from an organization's pool to one station,
//! and the licence file that carries one beside the licence.

use time::{Duration, OffsetDateTime};

use wipe_cert::{SigningKey, VerifyingKey};
use wipe_license::{
    check_grant_for, check_installable, load_license, parse_license, parse_license_file,
    AllowedMethods, CreditGrant, Entitlements, InstallProblem, LicenseCertificate, LicenseError,
    LicenseFile, Quota, Scope, VendorRoot, GRANT_FORMAT_VERSION,
};

/// A grant signed by `VendorRoot::from_signing_key(from_seed([7; 32]))` for
/// the instance key `from_seed([9; 32])`: [`pinned_grant`], pretty-printed.
const FIXTURE: &str = include_str!("fixtures/credit-grant-v1.json");
const FIXTURE_ROOT_HEX: &str = "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c";

fn fixture_root() -> VerifyingKey {
    let bytes: [u8; 32] = hex::decode(FIXTURE_ROOT_HEX).unwrap().try_into().unwrap();
    VerifyingKey::from_bytes(&bytes).unwrap()
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_780_000_000).unwrap()
}

fn instance_key() -> String {
    SigningKey::from_seed([9; 32]).public_key_id()
}

fn pinned_grant() -> CreditGrant {
    VendorRoot::from_signing_key(SigningKey::from_seed([7; 32]))
        .issue_credit_grant(
            "0b6f2a4e-3c1d-4f5a-9e8b-7c6d5e4f3a2b",
            "org-1",
            instance_key(),
            50,
            now(),
        )
        .unwrap()
}

fn licence(root: &VendorRoot, key: &str) -> LicenseCertificate {
    root.issue(
        "lic-1",
        key,
        Entitlements {
            customer_id: "org-1".into(),
            customer_name: "Northwind ITAD".into(),
            quota: Quota::Count { erasures: 5000 },
            scope: Scope::Site {
                site_id: "site-1".into(),
            },
            not_before: now() - Duration::days(1),
            not_after: now() + Duration::days(364),
            features: vec![],
            allowed_methods: AllowedMethods::All,
            machine_binding: None,
            paid_through: None,
        },
        now(),
    )
    .unwrap()
}

#[test]
fn the_fixture_pins_the_wire_shape() {
    let parsed: CreditGrant = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(parsed, pinned_grant());
    assert_eq!(
        serde_json::to_string_pretty(&pinned_grant()).unwrap(),
        FIXTURE
    );
    assert_eq!(
        parsed.verify(&[fixture_root()]).unwrap(),
        fixture_root().public_key_id()
    );
    assert_eq!(parsed.body.grant_format_version, GRANT_FORMAT_VERSION);
    let value: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let mut keys: Vec<&String> = value["body"].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "credits",
            "grant_format_version",
            "grant_id",
            "instance_public_key_id",
            "issued_at",
            "organization_id",
            "root_key_id"
        ]
    );
}

#[test]
fn a_grant_round_trips_and_checks_for_its_key() {
    let root = VendorRoot::generate();
    let key = SigningKey::generate().public_key_id();
    let grant = root
        .issue_credit_grant("g-1", "org-1", key.clone(), 25, now())
        .unwrap();
    let text = serde_json::to_string_pretty(&grant).unwrap();
    let parsed: CreditGrant = serde_json::from_str(&text).unwrap();
    assert_eq!(
        parsed.verify(&[root.verifying_key()]).unwrap(),
        root.public_key_id()
    );
    assert_eq!(parsed.body.root_key_id, root.public_key_id());
    assert_eq!(parsed.body.credits, 25);
    assert_eq!(
        check_grant_for(&parsed, &key, &[root.verifying_key()]),
        Ok(())
    );
}

#[test]
fn an_edited_grant_is_tampered() {
    let root = VendorRoot::generate();
    let key = SigningKey::generate().public_key_id();
    let grant = root
        .issue_credit_grant("g-1", "org-1", key.clone(), 25, now())
        .unwrap();
    let mut value = serde_json::to_value(&grant).unwrap();
    value["body"]["credits"] = 2500.into();
    let edited: CreditGrant = serde_json::from_value(value).unwrap();
    assert!(matches!(
        edited.verify(&[root.verifying_key()]),
        Err(LicenseError::Tampered(_))
    ));
    assert!(matches!(
        check_grant_for(&edited, &key, &[root.verifying_key()]),
        Err(InstallProblem::NotTrusted(_))
    ));
}

#[test]
fn a_foreign_root_or_key_is_refused() {
    let root = VendorRoot::generate();
    let other = VendorRoot::generate();
    let key = SigningKey::generate().public_key_id();
    let grant = root
        .issue_credit_grant("g-1", "org-1", key.clone(), 25, now())
        .unwrap();
    assert!(matches!(
        grant.verify(&[other.verifying_key()]),
        Err(LicenseError::UntrustedRoot(_))
    ));
    let elsewhere = SigningKey::generate().public_key_id();
    assert_eq!(
        check_grant_for(&grant, &elsewhere, &[root.verifying_key()]),
        Err(InstallProblem::WrongInstanceKey {
            expected: elsewhere.clone(),
            found: key,
        })
    );
}

#[test]
fn an_unknown_format_version_is_refused() {
    let root = VendorRoot::generate();
    let mut grant = root
        .issue_credit_grant("g-1", "org-1", instance_key(), 25, now())
        .unwrap();
    grant.body.grant_format_version = 2;
    assert!(matches!(
        grant.verify(&[root.verifying_key()]),
        Err(LicenseError::Verification(m)) if m.contains("grant_format_version 2")
    ));
}

#[test]
fn a_licence_never_parses_as_a_grant_nor_a_grant_as_a_licence() {
    let root = VendorRoot::generate();
    let key = instance_key();
    let lic = serde_json::to_string_pretty(&licence(&root, &key)).unwrap();
    let grant = serde_json::to_string_pretty(&pinned_grant()).unwrap();
    assert!(serde_json::from_str::<CreditGrant>(&lic).is_err());
    assert!(serde_json::from_str::<LicenseCertificate>(&grant).is_err());
    assert!(parse_license(&grant).is_err());
    assert!(parse_license_file(&grant).is_err());
    // Nor a grant in the licence's place in a file.
    let swapped = format!("{{\"license\": {grant}, \"credit_grant\": {lic}}}");
    assert!(parse_license_file(&swapped).is_err());
}

#[test]
fn a_bare_licence_file_has_no_grant_and_parses_as_before() {
    let root = VendorRoot::generate();
    let key = instance_key();
    let lic = licence(&root, &key);
    let text = serde_json::to_string_pretty(&lic).unwrap();
    let file = parse_license_file(&text).unwrap();
    assert_eq!(file.license, lic);
    assert_eq!(file.credit_grant, None);
    assert_eq!(parse_license(&text).unwrap(), lic);
}

#[test]
fn a_licence_file_with_a_grant_installs_like_the_bare_licence() {
    let root = VendorRoot::generate();
    let key = SigningKey::generate().public_key_id();
    let lic = licence(&root, &key);
    let grant = root
        .issue_credit_grant("g-1", "org-1", key.clone(), 50, now())
        .unwrap();
    let file = LicenseFile {
        license: lic.clone(),
        credit_grant: Some(grant.clone()),
    };
    let text = serde_json::to_string_pretty(&file).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["credit_grant", "license"]);

    let parsed = parse_license_file(&text).unwrap();
    assert_eq!(parsed, file);
    assert_eq!(parse_license(&text).unwrap(), lic);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("licence.json");
    std::fs::write(&path, &text).unwrap();
    let loaded = load_license(&path).unwrap();
    assert_eq!(loaded, lic);
    assert_eq!(
        check_installable(&loaded, &key, &[root.verifying_key()]),
        Ok(())
    );
    assert_eq!(
        check_grant_for(
            parsed.credit_grant.as_ref().unwrap(),
            &key,
            &[root.verifying_key()]
        ),
        Ok(())
    );
}

#[test]
fn an_unknown_key_is_ignored_but_a_bad_grant_is_not_a_licence_file() {
    let root = VendorRoot::generate();
    let key = instance_key();
    let licence_cert = licence(&root, &key);
    let lic = serde_json::to_value(&licence_cert).unwrap();
    // A key a later hub adds does not stop today's stations.
    let extra = serde_json::json!({ "license": lic, "credits": 5 });
    let file = parse_license_file(&extra.to_string()).unwrap();
    assert_eq!(file.license, licence_cert);
    assert_eq!(file.credit_grant, None);
    let bad = serde_json::json!({ "license": lic, "credit_grant": { "body": {} } });
    let err = parse_license(&bad.to_string()).unwrap_err().to_string();
    assert!(err.contains("not a licence file"), "{err}");
}
