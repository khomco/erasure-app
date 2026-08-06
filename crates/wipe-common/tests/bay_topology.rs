//! Bay topology: grid construction, numbering runs, bay→device resolution
//! and serde round-tripping (ADR-0002).

use wipe_common::*;

fn device(id: &str, serial: &str, path: &str, wwn: Option<&str>) -> Device {
    Device {
        id: DeviceId::from(id),
        vendor: "TestCo".into(),
        model: "TM-1".into(),
        serial: serial.into(),
        wwn: wwn.map(str::to_string),
        capacity_bytes: 1_000_000_000,
        media_type: MediaType::SsdSata,
        bus: BusType::Sata,
        firmware: None,
        removable: false,
        block_size: 512,
        path: path.into(),
    }
}

fn devices(n: usize) -> Vec<Device> {
    (0..n)
        .map(|i| {
            device(
                &format!("dev-{i}"),
                &format!("SN{i}"),
                &format!("/dev/sd{}", (b'a' + i as u8) as char),
                Some(&format!("wwn-{i}")),
            )
        })
        .collect()
}

fn bay<'a>(t: &'a BayTopology, label: &str) -> &'a Bay {
    t.bays()
        .find(|b| b.label == label)
        .unwrap_or_else(|| panic!("no bay labelled {label}"))
}

fn placed(r: &ResolvedBayTopology, bay_label: &str) -> Option<String> {
    let id = r
        .topology
        .bays()
        .find(|b| b.label == bay_label)
        .map(|b| b.id.clone())
        .expect("bay exists");
    r.occupancy
        .iter()
        .find(|o| o.bay_id == id)
        .map(|o| o.device_id.0.clone())
}

// --- geometry & numbering ---------------------------------------------------

#[test]
fn column_major_numbering_walks_down_each_column() {
    // The reference chassis: two columns of eight, numbered down the left
    // column first. Bay 1 is top-left, bay 8 bottom-left, bay 9 top-right.
    let bank = grid_bank(
        "c",
        "left",
        None,
        8,
        2,
        BayFormFactor::Lff35,
        TrayOrientation::Vertical,
        BayOrder::ColumnMajor,
        BayOrigin::TopLeft,
        1,
    );

    assert_eq!(bank.bays.len(), 16);
    let at = |label: &str| {
        bank.bays
            .iter()
            .find(|b| b.label == label)
            .map(|b| (b.row, b.col))
            .unwrap()
    };
    assert_eq!(at("1"), (0, 0));
    assert_eq!(at("8"), (7, 0));
    assert_eq!(at("9"), (0, 1));
    assert_eq!(at("16"), (7, 1));
}

#[test]
fn row_major_numbering_walks_across_each_row() {
    let bank = grid_bank(
        "c",
        "a",
        None,
        2,
        4,
        BayFormFactor::Sff25,
        TrayOrientation::Horizontal,
        BayOrder::RowMajor,
        BayOrigin::TopLeft,
        1,
    );
    let at = |label: &str| {
        bank.bays
            .iter()
            .find(|b| b.label == label)
            .map(|b| (b.row, b.col))
            .unwrap()
    };
    assert_eq!(at("1"), (0, 0));
    assert_eq!(at("4"), (0, 3));
    assert_eq!(at("5"), (1, 0));
}

#[test]
fn origin_flips_the_grid_without_changing_bay_count() {
    // Bottom-right origin: bay 1 sits at the bottom-right of the grid.
    let bank = grid_bank(
        "c",
        "a",
        None,
        2,
        3,
        BayFormFactor::Sff25,
        TrayOrientation::Horizontal,
        BayOrder::RowMajor,
        BayOrigin::BottomRight,
        1,
    );
    assert_eq!(bank.bays.len(), 6);
    let first = bank.bays.iter().find(|b| b.label == "1").unwrap();
    assert_eq!((first.row, first.col), (1, 2));
}

#[test]
fn label_start_offsets_the_numbering_run() {
    // The chassis' second bank continues from 17 rather than restarting.
    let t = arma_4u_32();
    assert_eq!(t.bay_count(), 32);
    assert!(t.bays().any(|b| b.label == "17"));
    assert!(t.bays().any(|b| b.label == "32"));
    // Bank B's first bay is top-left *of bank B*, not of the chassis.
    let b17 = bay(&t, "17");
    assert_eq!((b17.row, b17.col), (0, 0));
}

#[test]
fn bay_ids_are_unique_across_every_preset() {
    for name in preset_names() {
        let t = preset(name).expect("preset resolves");
        assert!(
            t.duplicate_bay_ids().is_empty(),
            "preset {name} has duplicate bay ids: {:?}",
            t.duplicate_bay_ids()
        );
    }
}

#[test]
fn unknown_preset_is_none() {
    assert!(preset("no-such-chassis").is_none());
}

// --- resolution -------------------------------------------------------------

#[test]
fn declared_bindings_match_by_each_supported_key() {
    let devs = devices(4);
    let mut t = dock_2bay();
    // Rebuild with explicit bindings across the supported key types.
    let bank = &mut t.enclosures[0].banks[0];
    bank.bays[0].binding = BayBinding::Serial {
        serial: "SN2".into(),
    };
    bank.bays[1].binding = BayBinding::Path {
        path: "/dev/sda".into(),
    };
    t.auto_fill_unbound = false;

    let r = t.resolve(&devs);
    assert_eq!(placed(&r, "1").as_deref(), Some("dev-2"));
    assert_eq!(placed(&r, "2").as_deref(), Some("dev-0"));
    // Two devices had nowhere to go and are reported, not dropped.
    assert_eq!(r.unplaced_devices.len(), 2);
}

#[test]
fn wwn_and_device_id_bindings_resolve() {
    let devs = devices(3);
    let mut t = dock_2bay();
    t.auto_fill_unbound = false;
    let bank = &mut t.enclosures[0].banks[0];
    bank.bays[0].binding = BayBinding::Wwn {
        wwn: "wwn-1".into(),
    };
    bank.bays[1].binding = BayBinding::DeviceId {
        device_id: DeviceId::from("dev-2"),
    };

    let r = t.resolve(&devs);
    assert_eq!(placed(&r, "1").as_deref(), Some("dev-1"));
    assert_eq!(placed(&r, "2").as_deref(), Some("dev-2"));
}

#[test]
fn ses_slot_binding_never_matches_until_a_backend_reports_slots() {
    // Guards the ADR-0002 claim: the variant exists for wipe-engine-linux
    // but nothing populates it, so it must resolve to empty rather than
    // accidentally matching something positionally.
    let devs = devices(2);
    let mut t = dock_2bay();
    t.auto_fill_unbound = false;
    t.enclosures[0].banks[0].bays[0].binding = BayBinding::SesSlot {
        slot: 0,
        enclosure: None,
    };
    let r = t.resolve(&devs);
    assert_eq!(placed(&r, "1"), None);
}

#[test]
fn declared_bindings_win_over_enumeration_fill() {
    // The whole point of declared bindings: adding drives to the bench must
    // never displace a bay someone configured deliberately.
    let devs = devices(3);
    let mut t = nvme_carrier_8();
    t.enclosures[0].banks[0].bays[2].binding = BayBinding::DeviceId {
        device_id: DeviceId::from("dev-0"),
    };

    let r = t.resolve(&devs);
    assert_eq!(placed(&r, "3").as_deref(), Some("dev-0"));
    // The remaining two fill the first free unbound bays in order.
    assert_eq!(placed(&r, "1").as_deref(), Some("dev-1"));
    assert_eq!(placed(&r, "2").as_deref(), Some("dev-2"));
    assert!(r.unplaced_devices.is_empty());
}

#[test]
fn a_device_is_never_placed_in_two_bays() {
    let devs = devices(1);
    let mut t = nvme_carrier_8();
    t.auto_fill_unbound = false;
    let bank = &mut t.enclosures[0].banks[0];
    bank.bays[0].binding = BayBinding::Serial {
        serial: "SN0".into(),
    };
    bank.bays[1].binding = BayBinding::Serial {
        serial: "SN0".into(),
    };

    let r = t.resolve(&devs);
    assert_eq!(r.occupancy.len(), 1);
    assert_eq!(placed(&r, "1").as_deref(), Some("dev-0"));
    assert_eq!(placed(&r, "2"), None);
}

#[test]
fn disabled_bays_are_skipped_by_enumeration_fill() {
    let devs = devices(2);
    let mut t = nvme_carrier_8();
    t.enclosures[0].banks[0].bays[0].disabled = true;

    let r = t.resolve(&devs);
    assert_eq!(placed(&r, "1"), None, "blanked bay must stay empty");
    assert_eq!(placed(&r, "2").as_deref(), Some("dev-0"));
    assert_eq!(placed(&r, "3").as_deref(), Some("dev-1"));
}

#[test]
fn auto_fill_can_be_switched_off_so_gaps_stay_visible() {
    let devs = devices(2);
    let mut t = nvme_carrier_8();
    t.auto_fill_unbound = false;

    let r = t.resolve(&devs);
    assert!(r.occupancy.is_empty());
    assert_eq!(r.unplaced_devices.len(), 2);
}

#[test]
fn more_devices_than_bays_reports_the_overflow() {
    let devs = devices(5);
    let t = dock_2bay();
    let r = t.resolve(&devs);
    assert_eq!(r.occupancy.len(), 2);
    assert_eq!(r.unplaced_devices.len(), 3);
}

#[test]
fn resolution_is_stable_across_repeated_calls() {
    let devs = devices(4);
    let t = arma_4u_32();
    assert_eq!(t.resolve(&devs), t.resolve(&devs));
}

#[test]
fn occupancy_is_emitted_in_bay_declaration_order() {
    let devs = devices(3);
    let t = nvme_carrier_8();
    let r = t.resolve(&devs);
    let labels: Vec<_> = r
        .occupancy
        .iter()
        .map(|o| {
            r.topology
                .bays()
                .find(|b| b.id == o.bay_id)
                .unwrap()
                .label
                .clone()
        })
        .collect();
    assert_eq!(labels, vec!["1", "2", "3"]);
}

// --- generated fallback -----------------------------------------------------

#[test]
fn generated_bench_is_flagged_and_sized_to_the_devices() {
    let t = generated_bench(4);
    assert!(t.generated, "fallback must announce itself as generated");
    assert!(t.bay_count() >= 4);
    assert!(t.enclosures[0].note.is_some());

    let r = t.resolve(&devices(4));
    assert_eq!(r.occupancy.len(), 4);
    assert!(r.unplaced_devices.is_empty());
}

#[test]
fn generated_bench_handles_an_empty_bench() {
    let t = generated_bench(0);
    assert_eq!(t.bay_count(), 1);
    let r = t.resolve(&[]);
    assert!(r.occupancy.is_empty());
}

#[test]
fn presets_are_not_flagged_as_generated() {
    for name in preset_names() {
        assert!(!preset(name).unwrap().generated, "{name}");
    }
}

// --- serde ------------------------------------------------------------------

#[test]
fn topology_round_trips_through_json() {
    let t = arma_4u_32();
    let json = serde_json::to_string(&t).unwrap();
    let back: BayTopology = serde_json::from_str(&json).unwrap();
    assert_eq!(t, back);
}

#[test]
fn resolved_topology_round_trips_through_json() {
    let r = arma_4u_32().resolve(&devices(3));
    let json = serde_json::to_string(&r).unwrap();
    let back: ResolvedBayTopology = serde_json::from_str(&json).unwrap();
    assert_eq!(r, back);
}

#[test]
fn a_minimal_hand_written_config_deserializes() {
    // What a customer with an unlisted chassis actually has to write.
    let json = r#"{
      "schema_version": 1,
      "label": "Bench 2",
      "enclosures": [{
        "id": "dock",
        "label": "Spare dock",
        "kind": "dock",
        "banks": [{
          "id": "a",
          "rows": 1,
          "cols": 2,
          "form_factor": "3.5in",
          "orientation": "vertical",
          "bays": [
            {"id": "dock.a.1", "label": "1", "row": 0, "col": 0},
            {"id": "dock.a.2", "label": "2", "row": 0, "col": 1,
             "binding": {"by": "serial", "serial": "SN1"}}
          ]
        }]
      }]
    }"#;
    let t: BayTopology = serde_json::from_str(json).unwrap();
    assert_eq!(t.bay_count(), 2);
    assert!(!t.generated);
    assert!(t.auto_fill_unbound, "defaults to on when omitted");
    assert_eq!(bay(&t, "1").binding, BayBinding::Unbound);

    let r = t.resolve(&devices(3));
    assert_eq!(placed(&r, "2").as_deref(), Some("dev-1"));
}

// --- arbitrary + mixed benches ---------------------------------------------
//
// The product claim is that a customer describes *their* hardware, whatever it
// is. These pin the parts of that claim the model is responsible for: several
// enclosures of different kinds at once, differing grid shapes, and form
// factors that vary both between and within banks.

fn mixed_bench() -> BayTopology {
    let rack = grid_bank(
        "rack",
        "a",
        Some("Bank A"),
        4,
        6,
        BayFormFactor::Lff35,
        TrayOrientation::Horizontal,
        BayOrder::RowMajor,
        BayOrigin::TopLeft,
        1,
    );
    let dock = grid_bank(
        "dock",
        "a",
        None,
        1,
        2,
        BayFormFactor::Sff25,
        TrayOrientation::Vertical,
        BayOrder::RowMajor,
        BayOrigin::TopLeft,
        1,
    );
    let carrier = grid_bank(
        "carrier",
        "a",
        None,
        4,
        2,
        BayFormFactor::M2,
        TrayOrientation::Horizontal,
        BayOrder::ColumnMajor,
        BayOrigin::TopLeft,
        1,
    );

    BayTopology {
        schema_version: BAY_TOPOLOGY_SCHEMA_VERSION,
        label: "Bench 3 — mixed".into(),
        generated: false,
        revision: 0,
        auto_fill_unbound: true,
        enclosures: vec![
            Enclosure {
                id: "rack".into(),
                label: "Supermicro 846 — 24 bay".into(),
                kind: EnclosureKind::Rackmount,
                banks: vec![rack],
                note: None,
            },
            Enclosure {
                id: "dock".into(),
                label: "StarTech 2-bay dock".into(),
                kind: EnclosureKind::Dock,
                banks: vec![dock],
                note: None,
            },
            Enclosure {
                id: "carrier".into(),
                label: "DiskClon NVMe-8".into(),
                kind: EnclosureKind::NvmeCarrier,
                banks: vec![carrier],
                note: None,
            },
        ],
    }
}

#[test]
fn a_bench_can_combine_enclosures_of_different_kinds_and_shapes() {
    let t = mixed_bench();
    assert_eq!(t.enclosures.len(), 3);
    assert_eq!(t.bay_count(), 24 + 2 + 8);
    assert!(t.duplicate_bay_ids().is_empty());

    let kinds: Vec<_> = t.enclosures.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        vec![
            EnclosureKind::Rackmount,
            EnclosureKind::Dock,
            EnclosureKind::NvmeCarrier
        ]
    );
    // Each enclosure keeps its own grid shape and numbering run.
    assert_eq!(
        (t.enclosures[0].banks[0].rows, t.enclosures[0].banks[0].cols),
        (4, 6)
    );
    assert_eq!(
        (t.enclosures[2].banks[0].rows, t.enclosures[2].banks[0].cols),
        (4, 2)
    );
}

#[test]
fn resolution_spans_every_enclosure_on_the_bench() {
    // Enumeration fill must walk the whole bench, not stop at the first
    // enclosure: a 24-bay rack full of drives must not starve the dock.
    let t = mixed_bench();
    // More devices than the first enclosure holds, fewer than the whole bench.
    let devs = devices(30);
    let r = t.resolve(&devs);
    assert_eq!(r.occupancy.len(), 30, "every device should land somewhere");
    assert_eq!(r.unplaced_devices.len(), 0);

    let filled_enclosures: std::collections::BTreeSet<_> = r
        .occupancy
        .iter()
        .map(|o| o.bay_id.0.split('.').next().unwrap().to_string())
        .collect();
    assert_eq!(filled_enclosures.len(), 3, "all three enclosures used");
}

#[test]
fn form_factor_can_vary_within_a_single_bank() {
    // A 3.5" caddy carrying a 2.5" sled is ordinary on an ITAD bench.
    let mut t = mixed_bench();
    t.enclosures[0].banks[0].bays[2].form_factor = Some(BayFormFactor::Sff25);

    let json = serde_json::to_string(&t).unwrap();
    let back: BayTopology = serde_json::from_str(&json).unwrap();
    assert_eq!(back, t, "per-bay override must survive a round trip");

    let bay3 = bay(&back, "3");
    assert_eq!(bay3.form_factor, Some(BayFormFactor::Sff25));
    // Bank default is unchanged for its other bays.
    assert_eq!(
        back.enclosures[0].banks[0].form_factor,
        BayFormFactor::Lff35
    );
    assert_eq!(bay(&back, "4").form_factor, None);
}

#[test]
fn bay_ids_stay_unique_across_enclosures_that_share_bank_names() {
    // Every enclosure here has a bank called "a" and bays labelled "1".
    let t = mixed_bench();
    assert!(t.duplicate_bay_ids().is_empty());
    let ones: Vec<_> = t.bays().filter(|b| b.label == "1").collect();
    assert_eq!(ones.len(), 3, "three bays are labelled 1");
    let ids: std::collections::BTreeSet<_> = ones.iter().map(|b| b.id.clone()).collect();
    assert_eq!(ids.len(), 3, "but their ids are distinct");
}

// --- editor round-tripping (ADR-0003 / bench-setup builder) ----------------

#[test]
fn a_bank_remembers_the_run_its_labels_came_from() {
    // Without this an editor can re-open a saved topology but cannot show,
    // let alone change, how it was numbered.
    let t = arma_4u_32();
    let run = t.enclosures[0].banks[0]
        .numbering
        .expect("generated banks record their run");
    assert_eq!(run.order, BayOrder::ColumnMajor);
    assert_eq!(run.origin, BayOrigin::TopLeft);
    assert_eq!(run.label_start, 1);
    assert_eq!(
        t.enclosures[0].banks[1].numbering.unwrap().label_start,
        17,
        "bank B continues the chassis numbering"
    );
}

#[test]
fn bay_ids_survive_a_label_rename() {
    // The regression this guards: ids used to embed the label, so renaming a
    // bay silently orphaned anything referring to it.
    let mut t = dock_2bay();
    let before = t.enclosures[0].banks[0].bays[0].id.clone();
    t.enclosures[0].banks[0].bays[0].label = "A1".into();
    let after = &t.enclosures[0].banks[0].bays[0];
    assert_eq!(after.id, before);
    assert_eq!(after.label, "A1");
    assert!(t.duplicate_bay_ids().is_empty());
}

#[test]
fn renumbering_preserves_operator_edits_by_position() {
    let mut t = nvme_carrier_8();
    let bank = &mut t.enclosures[0].banks[0];
    bank.bays[2].binding = BayBinding::Serial {
        serial: "SN9".into(),
    };
    bank.bays[3].disabled = true;
    bank.bays[4].form_factor = Some(BayFormFactor::U2);
    let (kept_pos, disabled_pos, ff_pos) = (
        (bank.bays[2].row, bank.bays[2].col),
        (bank.bays[3].row, bank.bays[3].col),
        (bank.bays[4].row, bank.bays[4].col),
    );

    let run = NumberingRun {
        order: BayOrder::RowMajor,
        origin: BayOrigin::BottomRight,
        label_start: 0,
    };
    let renumbered = renumber_bank(bank, "carrier", run);

    // Labels follow the new run...
    assert_eq!(renumbered.numbering.unwrap(), run);
    assert!(renumbered.bays.iter().any(|b| b.label == "0"));
    // ...and everything the operator set is still on the same physical slot.
    let at = |p: (u16, u16)| renumbered.bay_at(p.0, p.1).expect("bay at position");
    assert_eq!(
        at(kept_pos).binding,
        BayBinding::Serial {
            serial: "SN9".into()
        }
    );
    assert!(at(disabled_pos).disabled);
    assert_eq!(at(ff_pos).form_factor, Some(BayFormFactor::U2));
}

#[test]
fn form_factor_of_honours_the_per_bay_override() {
    // Backs the renderer fix: a 2.5" sled in a 3.5" bank must draw at 2.5".
    let mut t = arma_4u_32();
    t.enclosures[0].banks[0].bays[0].form_factor = Some(BayFormFactor::Sff25);
    let bank = &t.enclosures[0].banks[0];
    assert_eq!(bank.form_factor_of(&bank.bays[0]), BayFormFactor::Sff25);
    assert_eq!(bank.form_factor_of(&bank.bays[1]), BayFormFactor::Lff35);
}

// --- validation ------------------------------------------------------------

#[test]
fn a_clean_preset_is_savable() {
    for name in preset_names() {
        let t = preset(name).unwrap();
        let errs: Vec<_> = t
            .validate()
            .into_iter()
            .filter(|p| p.severity == ProblemSeverity::Error)
            .collect();
        assert!(errs.is_empty(), "{name} should be savable, got {errs:?}");
        assert!(t.is_savable());
    }
}

#[test]
fn duplicate_labels_within_a_bank_block_the_save() {
    let mut t = arma_4u_32();
    t.enclosures[0].banks[0].bays[1].label = "1".into();
    let problems = t.validate();
    assert!(!t.is_savable());
    assert!(problems
        .iter()
        .any(|p| p.code == "duplicate_bay_label" && p.severity == ProblemSeverity::Error));
}

#[test]
fn a_bay_outside_its_grid_blocks_the_save() {
    let mut t = dock_2bay();
    t.enclosures[0].banks[0].bays[0].row = 9;
    assert!(!t.is_savable());
    assert!(t.validate().iter().any(|p| p.code == "bay_outside_grid"));
}

#[test]
fn an_empty_grid_blocks_the_save() {
    let mut t = dock_2bay();
    t.enclosures[0].banks[0].rows = 0;
    assert!(!t.is_savable());
    assert!(t.validate().iter().any(|p| p.code == "empty_grid"));
}

#[test]
fn auto_fill_on_a_large_bench_warns_but_still_saves() {
    // Enumeration order is a guess about physical position; on 32 bays that
    // is the kind of guess an operator stops double-checking.
    let t = arma_4u_32();
    assert!(t.auto_fill_unbound);
    let problems = t.validate();
    assert!(problems
        .iter()
        .any(|p| p.code == "auto_fill_on_large_bench" && p.severity == ProblemSeverity::Warning));
    assert!(t.is_savable(), "a warning must not block saving");
}

#[test]
fn a_small_bench_does_not_warn_about_auto_fill() {
    let t = dock_2bay();
    assert!(!t
        .validate()
        .iter()
        .any(|p| p.code == "auto_fill_on_large_bench"));
}
