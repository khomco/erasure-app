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
