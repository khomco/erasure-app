use wipe_common::*;

fn nvme_caps() -> Capabilities {
    Capabilities {
        ata_security: None,
        nvme_sanitize: Some(NvmeSanitizeCaps {
            block_erase: true,
            overwrite: true,
            crypto_erase: true,
            ndi_inhibited: false,
            nodmmas: 0,
            estimated_block_erase_secs: Some(2),
            estimated_crypto_erase_secs: Some(1),
            estimated_overwrite_secs: Some(600),
        }),
        trim: true,
        crypto_erase_supported: true,
        sed: SedStatus::Provisioned,
        hpa_present: false,
        dco_present: false,
        frozen: false,
    }
}

fn sata_ssd_caps() -> Capabilities {
    Capabilities {
        ata_security: Some(AtaSecurityCaps {
            supported: true,
            enhanced_supported: true,
            estimated_minutes: Some(2),
            enhanced_estimated_minutes: Some(2),
            frozen: false,
        }),
        nvme_sanitize: None,
        trim: true,
        crypto_erase_supported: false,
        sed: SedStatus::None,
        hpa_present: false,
        dco_present: false,
        frozen: false,
    }
}

#[test]
fn high_class_nvme_picks_crypto_erase_when_sed_provisioned() {
    let m = select_method(
        &nvme_caps(),
        MediaType::SsdNvme,
        Classification::High,
        Intent::Reuse,
    )
    .unwrap();
    assert!(matches!(m, Method::NvmeSanitizeCryptoErase { .. }));
    assert_eq!(m.category(), Category::Purge);
}

#[test]
fn high_class_nvme_no_sed_picks_block_erase() {
    let mut caps = nvme_caps();
    caps.sed = SedStatus::None;
    let m = select_method(
        &caps,
        MediaType::SsdNvme,
        Classification::High,
        Intent::Reuse,
    )
    .unwrap();
    assert!(matches!(m, Method::NvmeSanitizeBlockErase { .. }));
    assert_eq!(m.category(), Category::Purge);
}

#[test]
fn moderate_class_sata_picks_ata_secure_erase_enhanced() {
    let m = select_method(
        &sata_ssd_caps(),
        MediaType::SsdSata,
        Classification::Moderate,
        Intent::Reuse,
    )
    .unwrap();
    assert!(matches!(m, Method::AtaSecureErase { enhanced: true }));
    assert_eq!(m.category(), Category::Purge);
}

#[test]
fn low_class_sata_picks_basic_secure_erase() {
    let m = select_method(
        &sata_ssd_caps(),
        MediaType::SsdSata,
        Classification::Low,
        Intent::Reuse,
    )
    .unwrap();
    // Current selector prefers ATA Secure Erase (basic) for Low → Clear.
    match m {
        Method::AtaSecureErase { enhanced: false } => {}
        other => panic!("expected basic ATA secure erase, got {other:?}"),
    }
    assert_eq!(m.category(), Category::Clear);
}

#[test]
fn destroy_intent_routes_to_destroy() {
    let m = select_method(
        &nvme_caps(),
        MediaType::SsdNvme,
        Classification::High,
        Intent::Destroy,
    )
    .unwrap();
    assert!(matches!(m, Method::Destroy { .. }));
    assert_eq!(m.category(), Category::Destroy);
}

#[test]
fn frozen_ata_with_no_nvme_fails_for_purge() {
    let mut caps = sata_ssd_caps();
    caps.ata_security.as_mut().unwrap().frozen = true;
    let m = select_method(
        &caps,
        MediaType::SsdSata,
        Classification::High,
        Intent::Reuse,
    );
    assert!(m.is_none(), "frozen device without alternative should not select a Purge method");
}

#[test]
fn serde_round_trip_evidence() {
    let ev = CommandEvidence {
        interface: "nvme-admin".into(),
        opcode: Some(0x84),
        action: Some(0x02),
        raw_cdb: Some(vec![0x84, 0x00, 0x02, 0x00]),
        status: Some(0),
        sense: None,
        log_page: Some(vec![0x01, 0x01, 0x00, 0x00]),
        duration_ms: 1500,
        note: Some("simulated".into()),
    };
    let s = serde_json::to_string(&ev).unwrap();
    let back: CommandEvidence = serde_json::from_str(&s).unwrap();
    assert_eq!(back.raw_cdb, ev.raw_cdb);
    assert_eq!(back.log_page, ev.log_page);
}
