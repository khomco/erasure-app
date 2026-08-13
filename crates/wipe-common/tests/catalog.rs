//! Enclosure model catalog (ADR-0004).
//!
//! The catalog is a claim about the physical world, so the tests that matter
//! most are the ones that stop it claiming too much: matching must be
//! conservative, and an unrecognised enclosure must stay renderable.

use wipe_common::*;

fn inquiry(vendor: &str, product: &str, revision: Option<&str>) -> Inquiry {
    Inquiry {
        vendor: vendor.into(),
        product: product.into(),
        revision: revision.map(str::to_string),
    }
}

// --- the bundled catalog ---------------------------------------------------

#[test]
fn the_bundled_catalog_parses_and_is_internally_consistent() {
    // Compiled in via include_str!, so a malformed catalog is a build-time
    // bug rather than something a station can hit at runtime.
    let c = Catalog::bundled();
    assert_eq!(c.schema_version, CATALOG_SCHEMA_VERSION);
    assert!(!c.models.is_empty());
    assert!(
        c.duplicate_ids().is_empty(),
        "duplicate model ids: {:?}",
        c.duplicate_ids()
    );
    for m in &c.models {
        assert!(!m.vendor.is_empty() && !m.product.is_empty(), "{}", m.id);
        assert!(!m.spec.banks.is_empty(), "{} has no banks", m.id);
        assert!(m.bay_count() > 0, "{} has no bays", m.id);
        for b in &m.spec.banks {
            assert!(b.rows > 0 && b.cols > 0, "{} has an empty grid", m.id);
        }
    }
}

#[test]
fn every_model_expands_into_a_savable_enclosure() {
    // The catalog is a source of defaults for the *existing* topology model.
    // If an entry cannot expand into something the validator accepts, the
    // entry is wrong.
    for m in &Catalog::bundled().models {
        let enc = m.expand("enc1");
        let topology = BayTopology {
            schema_version: BAY_TOPOLOGY_SCHEMA_VERSION,
            label: "t".into(),
            generated: false,
            revision: 0,
            auto_fill_unbound: false,
            enclosures: vec![enc],
        };
        assert!(
            topology.is_savable(),
            "{} expands to an invalid topology: {:?}",
            m.id,
            topology.validate()
        );
        assert_eq!(topology.bay_count(), m.bay_count(), "{}", m.id);
    }
}

#[test]
fn expansion_carries_the_model_reference_and_numbering() {
    let c = Catalog::bundled();
    let m = c.get(&ModelId::from("arma/industrial-4u-32")).unwrap();
    let enc = m.expand("chassis");

    assert_eq!(enc.model_ref.as_ref(), Some(&m.id));
    assert_eq!(enc.banks.len(), 2);
    // Bank B continues the chassis numbering rather than restarting.
    let labels: Vec<_> = enc.banks[1].bays.iter().map(|b| b.label.clone()).collect();
    assert!(labels.contains(&"17".to_string()));
    assert!(labels.contains(&"32".to_string()));
    // And the run is recorded so an editor can round-trip it.
    assert_eq!(enc.banks[0].numbering.unwrap().order, BayOrder::ColumnMajor);
}

#[test]
fn expanded_bays_start_unbound_so_the_operator_still_maps_them() {
    // A catalog knows the *shape* of a chassis, never which drive is in
    // which bay. Pre-binding would be inventing knowledge we do not have.
    let c = Catalog::bundled();
    let m = c.get(&ModelId::from("startech/sdock2u33")).unwrap();
    let enc = m.expand("dock");
    assert!(enc
        .banks
        .iter()
        .flat_map(|b| b.bays.iter())
        .all(|b| matches!(b.binding, BayBinding::Unbound)));
}

// --- capabilities ----------------------------------------------------------

#[test]
fn capabilities_are_absent_rather_than_false_when_unverified() {
    // "We don't know" must not render the same as "no" (ADR-0004 §2).
    let c = Catalog::bundled();
    let unverified = c.get(&ModelId::from("icydock/mb522sp-b")).unwrap();
    assert!(
        unverified.capabilities.is_none(),
        "an unverified model must not assert capabilities"
    );

    let verified = c.get(&ModelId::from("arma/industrial-4u-32")).unwrap();
    let caps = verified.capabilities.expect("verified model states them");
    assert!(caps.locate_led);
    assert!(caps.ses_slot_addressing);
    assert!(!caps.per_bay_power, "not claimed, so it must read false");
}

#[test]
fn a_model_that_claims_capabilities_records_who_verified_it() {
    // Wrong catalog data is worse than absent catalog data.
    for m in Catalog::bundled()
        .models
        .iter()
        .filter(|m| m.capabilities.is_some())
    {
        // Not every entry has provenance yet, but the ones asserting
        // hardware behaviour should.
        if m.capabilities
            .map(|c| c.locate_led || c.ses_slot_addressing)
            == Some(true)
        {
            assert!(
                m.verified_by.is_some(),
                "{} claims hardware capabilities with no provenance",
                m.id
            );
        }
    }
}

// --- matching --------------------------------------------------------------

#[test]
fn an_exact_usb_id_matches_with_high_confidence() {
    let c = Catalog::bundled();
    let id = EnclosureIdentity {
        usb: Some(UsbId {
            vid: "0x174C".into(), // deliberately different case
            pid: "0x55aa".into(),
        }),
        ..Default::default()
    };
    let m = c.best_match(&id).expect("a high-confidence match");
    assert_eq!(m.model_id, ModelId::from("startech/sdock2u33"));
    assert_eq!(m.confidence, MatchConfidence::High);
    assert!(m.evidence.contains("USB"), "{}", m.evidence);
}

#[test]
fn an_exact_pci_id_matches_with_high_confidence() {
    let c = Catalog::bundled();
    let id = EnclosureIdentity {
        pci: Some(PciId {
            vendor: "0x1b21".into(),
            device: "0x2824".into(),
        }),
        ..Default::default()
    };
    assert_eq!(
        c.best_match(&id).unwrap().model_id,
        ModelId::from("diskclon/nvme-8")
    );
}

#[test]
fn a_full_inquiry_match_is_high_and_a_revision_mismatch_drops_to_medium() {
    let c = Catalog::bundled();

    let exact = EnclosureIdentity {
        scsi_inquiry: Some(inquiry("ARMA    ", "BP4U32          ", Some("0100"))),
        ..Default::default()
    };
    let hit = c.best_match(&exact).expect("high match");
    assert_eq!(hit.model_id, ModelId::from("arma/industrial-4u-32"));

    // Firmware moved on. Still almost certainly the same chassis, but we
    // downgrade rather than assert.
    let newer = EnclosureIdentity {
        scsi_inquiry: Some(inquiry("ARMA", "BP4U32", Some("0201"))),
        ..Default::default()
    };
    let matches = c.match_identity(&newer);
    assert_eq!(matches[0].confidence, MatchConfidence::Medium);
    assert!(matches[0].evidence.contains("revision differs"));
    assert!(
        c.best_match(&newer).is_none(),
        "medium confidence must not pre-select"
    );
}

#[test]
fn a_product_string_alone_is_only_a_suggestion() {
    let c = Catalog::bundled();
    let id = EnclosureIdentity {
        scsi_inquiry: Some(inquiry("Unknown", "MB522SP-B", None)),
        ..Default::default()
    };
    let matches = c.match_identity(&id);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].confidence, MatchConfidence::Low);
    assert!(
        c.best_match(&id).is_none(),
        "a fuzzy hit must never pre-select a model"
    );
}

#[test]
fn unknown_hardware_matches_nothing_at_all() {
    // The important non-event: an unrecognised enclosure produces no match,
    // which the UI renders with the generic shell rather than a guess.
    let c = Catalog::bundled();
    let id = EnclosureIdentity {
        usb: Some(UsbId {
            vid: "0xdead".into(),
            pid: "0xbeef".into(),
        }),
        ..Default::default()
    };
    assert!(c.match_identity(&id).is_empty());
    assert!(c.best_match(&id).is_none());
}

#[test]
fn an_empty_identity_matches_nothing() {
    assert!(Catalog::bundled()
        .match_identity(&EnclosureIdentity::default())
        .is_empty());
}

#[test]
fn ambiguous_high_confidence_selects_nothing() {
    // Two entries claiming the same hardware is a catalog bug. Picking a
    // winner silently would hide it and mis-render someone's bench.
    let mut c = Catalog::bundled();
    let mut clone = c.get(&ModelId::from("startech/sdock2u33")).unwrap().clone();
    clone.id = ModelId::from("someoneelse/clone");
    c.models.push(clone);

    let id = EnclosureIdentity {
        usb: Some(UsbId {
            vid: "0x174c".into(),
            pid: "0x55aa".into(),
        }),
        ..Default::default()
    };
    assert_eq!(c.match_identity(&id).len(), 2);
    assert!(c.best_match(&id).is_none());
}

#[test]
fn matches_are_ranked_best_first_and_stable() {
    let c = Catalog::bundled();
    let id = EnclosureIdentity {
        scsi_inquiry: Some(inquiry("ARMA", "BP4U32", Some("0100"))),
        ..Default::default()
    };
    let a = c.match_identity(&id);
    let b = c.match_identity(&id);
    assert_eq!(a, b, "matching must be deterministic");
    for w in a.windows(2) {
        assert!(w[0].confidence >= w[1].confidence);
    }
}

// --- overlay + search ------------------------------------------------------

#[test]
fn a_local_overlay_corrects_bundled_data_without_a_release() {
    let bundled = Catalog::bundled();
    let mut fixed = bundled
        .get(&ModelId::from("generic/rackmount-24"))
        .unwrap()
        .clone();
    fixed.product = "Rackmount 24-bay (corrected)".into();

    let overlay = Catalog {
        schema_version: CATALOG_SCHEMA_VERSION,
        models: vec![fixed],
    };
    let merged = bundled.overlay(&overlay);

    assert_eq!(
        merged.models.len(),
        bundled.models.len(),
        "same id replaces"
    );
    assert_eq!(
        merged
            .get(&ModelId::from("generic/rackmount-24"))
            .unwrap()
            .product,
        "Rackmount 24-bay (corrected)"
    );
}

#[test]
fn an_overlay_can_add_a_model_the_bundle_never_had() {
    let bundled = Catalog::bundled();
    let mut extra = bundled.models[0].clone();
    extra.id = ModelId::from("customer/one-off");
    let merged = bundled.overlay(&Catalog {
        schema_version: CATALOG_SCHEMA_VERSION,
        models: vec![extra],
    });
    assert_eq!(merged.models.len(), bundled.models.len() + 1);
    assert!(merged.get(&ModelId::from("customer/one-off")).is_some());
}

#[test]
fn search_finds_models_by_vendor_product_and_alias() {
    let c = Catalog::bundled();
    assert!(!c.search("startech").is_empty());
    assert!(!c.search("SDOCK2U33").is_empty());
    assert!(!c.search("toaster dock").is_empty(), "alias lookup");
    assert!(
        c.search("").len() == c.models.len(),
        "empty query lists all"
    );
    assert!(c.search("nothing like this").is_empty());
}
