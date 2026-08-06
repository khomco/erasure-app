//! Bench topology — the physical arrangement of drive bays at a station,
//! declared as configuration so the UI can mirror the hardware the operator
//! is standing in front of (ADR-0002).
//!
//! The hierarchy is `BayTopology → Enclosure → Bank → Bay`:
//!
//! - an **Enclosure** is one physical housing (rackmount chassis, benchtop
//!   duplicator, hot-swap dock, NVMe carrier, USB caddy);
//! - a **Bank** is a contiguous grid of bays within it that share a form
//!   factor, orientation and numbering run;
//! - a **Bay** is one slot holding at most one `Device`.
//!
//! Everything a renderer needs is geometry plus labels; everything the
//! *resolver* needs is [`BayBinding`]. Resolution happens here rather than in
//! the frontend so the matching rules have one implementation and tests.

use serde::{Deserialize, Serialize};

use crate::{Device, DeviceId};

/// Bumped when the on-the-wire shape of a topology document changes
/// incompatibly. Config files carry it so a station can refuse a file it
/// doesn't understand rather than silently mis-rendering the bench.
pub const BAY_TOPOLOGY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BayId(pub String);

impl std::fmt::Display for BayId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for BayId {
    fn from(s: &str) -> Self {
        BayId(s.to_string())
    }
}

/// What kind of physical housing this is. Affects how a renderer draws the
/// shell (rack ears, a dock lip, a bare carrier board) — never how bays are
/// laid out, which is the Bank's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnclosureKind {
    /// Rack-mounted chassis with front-loading hot-swap trays.
    Rackmount,
    /// Benchtop duplicator/eraser appliance with top- or front-loading bays.
    Duplicator,
    /// Open "toaster" style hot-swap dock.
    Dock,
    /// M.2 / U.2 carrier board or NVMe duplicator.
    NvmeCarrier,
    /// Single-drive USB caddy or adapter.
    UsbCaddy,
    /// Drives attached inside the host, not in an operator-facing bay.
    Internal,
}

/// Physical drive form factor a bay accepts. Drives the aspect ratio a
/// renderer uses for the tray.
/// Wire names are the sizes an operator would say out loud (`"3.5in"`,
/// `"m2"`), because these are typed by hand into config files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BayFormFactor {
    /// 3.5" LFF tray.
    #[serde(rename = "3.5in")]
    Lff35,
    /// 2.5" SFF tray.
    #[serde(rename = "2.5in")]
    Sff25,
    /// M.2 2280-ish socket.
    #[serde(rename = "m2")]
    M2,
    /// U.2 / U.3 SFF NVMe.
    #[serde(rename = "u2")]
    U2,
    /// Anything else, drawn as a neutral slot.
    #[serde(rename = "other")]
    Other,
}

impl BayFormFactor {
    /// Nominal width:height of the *tray face* as installed, before
    /// orientation is applied. Renderers use this so a 3.5" tray doesn't
    /// come out the same shape as an M.2 stick.
    pub fn face_aspect(self) -> f32 {
        match self {
            Self::Lff35 => 4.0,
            Self::Sff25 => 3.2,
            Self::M2 => 6.0,
            Self::U2 => 3.0,
            Self::Other => 3.5,
        }
    }
}

/// How trays sit in their bank. A 4U chassis holds 3.5" trays on their side
/// (long axis vertical); a dock holds them flat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayOrientation {
    /// Tray's long axis runs left-to-right.
    Horizontal,
    /// Tray's long axis runs top-to-bottom.
    Vertical,
}

/// Direction the vendor's bay numbers run through a bank's grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BayOrder {
    /// Fill across each row before moving down.
    RowMajor,
    /// Fill down each column before moving right.
    ColumnMajor,
}

/// Which corner of the grid bay numbering starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BayOrigin {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// How a bay resolves to a device.
///
/// The variants are ordered roughly by how much we trust them. `SesSlot` is
/// the one real hardware will use — SAS-3 expanders and SES enclosure
/// services report a device slot number, which is the only mechanism that
/// reliably answers "which physical tray is this block device in". Nothing
/// populates it yet; `wipe-engine-linux` will.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum BayBinding {
    /// SES device slot number (0–255; 255 means "no associated slot").
    /// `enclosure` is the SES enclosure logical id when a host has several.
    SesSlot {
        slot: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enclosure: Option<String>,
    },
    /// Host device path, e.g. `/dev/sdb`. Stable per-port on a fixed bench,
    /// not stable across reboots on many controllers — prefer `SesSlot`.
    Path { path: String },
    /// Drive serial number. Follows the drive, not the bay, so this pins a
    /// *specific* drive to a bay — useful for reference drives, wrong for
    /// general intake.
    Serial { serial: String },
    /// World-wide name. Same caveat as `Serial`.
    Wwn { wwn: String },
    /// Explicit backend device id. What the mock backend uses.
    DeviceId { device_id: DeviceId },
    /// No rule: filled from enumeration order if the topology allows it.
    Unbound,
}

impl BayBinding {
    /// Does this binding identify `device`?
    ///
    /// `Unbound` never matches — enumeration-order fill is handled by the
    /// resolver, deliberately as a separate and lower-priority pass.
    pub fn matches(&self, device: &Device) -> bool {
        match self {
            Self::DeviceId { device_id } => &device.id == device_id,
            Self::Path { path } => &device.path == path,
            Self::Serial { serial } => &device.serial == serial,
            Self::Wwn { wwn } => device.wwn.as_deref() == Some(wwn.as_str()),
            // Nothing reports SES slots until wipe-engine-linux lands.
            Self::SesSlot { .. } => false,
            Self::Unbound => false,
        }
    }
}

/// One physical slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bay {
    /// Stable, unique within the topology. Convention: `<enclosure>.<bank>.<label>`.
    pub id: BayId,
    /// What the operator calls this bay — normally what is silkscreened on
    /// the hardware. Never our array index.
    pub label: String,
    /// Zero-based grid position within the bank.
    pub row: u16,
    pub col: u16,
    /// Overrides the bank's form factor for mixed banks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form_factor: Option<BayFormFactor>,
    #[serde(default = "unbound")]
    pub binding: BayBinding,
    /// Physically blanked off or not wired. Rendered as a filler panel and
    /// never considered for enumeration-order fill.
    #[serde(default)]
    pub disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn unbound() -> BayBinding {
    BayBinding::Unbound
}

/// A contiguous grid of bays sharing a form factor, orientation and
/// numbering run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bank {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub rows: u16,
    pub cols: u16,
    pub form_factor: BayFormFactor,
    pub orientation: TrayOrientation,
    pub bays: Vec<Bay>,
}

impl Bank {
    /// The bay at a grid position, if the topology declares one.
    pub fn bay_at(&self, row: u16, col: u16) -> Option<&Bay> {
        self.bays.iter().find(|b| b.row == row && b.col == col)
    }
}

/// One physical housing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enclosure {
    pub id: String,
    pub label: String,
    pub kind: EnclosureKind,
    /// Banks in left-to-right visual order.
    pub banks: Vec<Bank>,
    /// Free-form note shown with the enclosure, e.g. "rack 3, position 12".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Enclosure {
    pub fn bay_count(&self) -> usize {
        self.banks.iter().map(|b| b.bays.len()).sum()
    }
}

/// A station's declared physical layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BayTopology {
    pub schema_version: u32,
    /// Human label for the bench as a whole.
    pub label: String,
    /// Set when this topology was generated because the station had no
    /// configuration. The UI says so rather than implying the station
    /// really has this hardware (ADR-0002).
    #[serde(default)]
    pub generated: bool,
    /// Fill `Unbound`, non-disabled bays from device-enumeration order.
    /// Convenient on an unconfigured bench and for the mock backend;
    /// switch it off once bays are bound properly, so an unbound bay is
    /// visibly a configuration gap rather than quietly showing a drive
    /// that may not be in it.
    #[serde(default = "default_true")]
    pub auto_fill_unbound: bool,
    pub enclosures: Vec<Enclosure>,
}

fn default_true() -> bool {
    true
}

impl BayTopology {
    pub fn bay_count(&self) -> usize {
        self.enclosures.iter().map(|e| e.bay_count()).sum()
    }

    pub fn bays(&self) -> impl Iterator<Item = &Bay> {
        self.enclosures
            .iter()
            .flat_map(|e| e.banks.iter())
            .flat_map(|b| b.bays.iter())
    }

    /// Every `BayId` that appears more than once. A topology with duplicates
    /// will render, but status will land on an arbitrary one of the twins —
    /// so callers should treat a non-empty result as a config error.
    pub fn duplicate_bay_ids(&self) -> Vec<BayId> {
        let mut seen = std::collections::BTreeSet::new();
        let mut dupes = std::collections::BTreeSet::new();
        for bay in self.bays() {
            if !seen.insert(bay.id.clone()) {
                dupes.insert(bay.id.clone());
            }
        }
        dupes.into_iter().collect()
    }

    /// Resolve every bay against the current device list.
    ///
    /// Two passes, in this order:
    ///
    /// 1. **Declared bindings.** Each bay with a non-`Unbound` binding takes
    ///    the first device it matches. A device claimed here cannot be
    ///    claimed again.
    /// 2. **Enumeration-order fill**, only when `auto_fill_unbound` is set.
    ///    Remaining devices drop into remaining `Unbound`, non-disabled bays
    ///    in declaration order.
    ///
    /// Declared bindings win, so adding a drive to the bench can never
    /// displace a bay that was configured deliberately.
    pub fn resolve(&self, devices: &[Device]) -> ResolvedBayTopology {
        let mut claimed: Vec<bool> = vec![false; devices.len()];
        let mut assignment: std::collections::HashMap<BayId, DeviceId> = Default::default();

        // Pass 1 — declared bindings.
        for bay in self.bays() {
            if bay.disabled || matches!(bay.binding, BayBinding::Unbound) {
                continue;
            }
            let found = devices
                .iter()
                .enumerate()
                .find(|(i, d)| !claimed[*i] && bay.binding.matches(d));
            if let Some((i, device)) = found {
                claimed[i] = true;
                assignment.insert(bay.id.clone(), device.id.clone());
            }
        }

        // Pass 2 — enumeration-order fill of whatever is left.
        if self.auto_fill_unbound {
            let mut next = 0usize;
            for bay in self.bays() {
                if bay.disabled || !matches!(bay.binding, BayBinding::Unbound) {
                    continue;
                }
                while next < devices.len() && claimed[next] {
                    next += 1;
                }
                if next >= devices.len() {
                    break;
                }
                claimed[next] = true;
                assignment.insert(bay.id.clone(), devices[next].id.clone());
            }
        }

        let unplaced = devices
            .iter()
            .enumerate()
            .filter(|(i, _)| !claimed[*i])
            .map(|(_, d)| d.id.clone())
            .collect();

        // Emit in bay-declaration order so the payload is stable across runs.
        let occupancy = self
            .bays()
            .filter_map(|bay| {
                assignment.get(&bay.id).map(|device_id| BayOccupancy {
                    bay_id: bay.id.clone(),
                    device_id: device_id.clone(),
                })
            })
            .collect();

        ResolvedBayTopology {
            topology: self.clone(),
            occupancy,
            unplaced_devices: unplaced,
        }
    }
}

/// One bay's resolved device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BayOccupancy {
    pub bay_id: BayId,
    pub device_id: DeviceId,
}

/// A topology plus the current bay→device resolution. This is what
/// `GET /api/bay-topology` serves: the frontend renders geometry from
/// `topology` and joins `occupancy` against the devices and jobs it already
/// holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedBayTopology {
    pub topology: BayTopology,
    pub occupancy: Vec<BayOccupancy>,
    /// Devices the station can see that no bay claimed. Surfaced rather than
    /// dropped — a drive present but off the map is exactly the situation an
    /// operator must not be left unaware of.
    pub unplaced_devices: Vec<DeviceId>,
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/// Build a bank as a plain grid, numbering bays along `order` from `origin`
/// starting at `label_start`.
///
/// This is the workhorse the built-in presets are expressed in, and covers
/// the numbering runs vendors actually use. A bench whose labels don't follow
/// any run can still declare `Bay`s explicitly.
#[allow(clippy::too_many_arguments)]
pub fn grid_bank(
    enclosure_id: &str,
    bank_id: &str,
    label: Option<&str>,
    rows: u16,
    cols: u16,
    form_factor: BayFormFactor,
    orientation: TrayOrientation,
    order: BayOrder,
    origin: BayOrigin,
    label_start: u16,
) -> Bank {
    let mut bays = Vec::with_capacity((rows as usize) * (cols as usize));

    // Walk the grid in numbering order, mapping each step back to a real
    // (row, col) through the origin.
    let (outer, inner) = match order {
        BayOrder::RowMajor => (rows, cols),
        BayOrder::ColumnMajor => (cols, rows),
    };

    let mut n = label_start;
    for o in 0..outer {
        for i in 0..inner {
            let (mut row, mut col) = match order {
                BayOrder::RowMajor => (o, i),
                BayOrder::ColumnMajor => (i, o),
            };
            if matches!(origin, BayOrigin::TopRight | BayOrigin::BottomRight) {
                col = cols - 1 - col;
            }
            if matches!(origin, BayOrigin::BottomLeft | BayOrigin::BottomRight) {
                row = rows - 1 - row;
            }
            let label_text = n.to_string();
            bays.push(Bay {
                id: BayId(format!("{enclosure_id}.{bank_id}.{label_text}")),
                label: label_text,
                row,
                col,
                form_factor: None,
                binding: BayBinding::Unbound,
                disabled: false,
                note: None,
            });
            n += 1;
        }
    }

    Bank {
        id: bank_id.to_string(),
        label: label.map(str::to_string),
        rows,
        cols,
        form_factor,
        orientation,
        bays,
    }
}

/// The honest default for a station with no bay configuration: one
/// auto-sized bank, explicitly marked generated, bays filled in enumeration
/// order.
///
/// Deliberately *not* a plausible-looking chassis — see ADR-0002.
pub fn generated_bench(device_count: usize) -> BayTopology {
    let n = device_count.max(1) as u16;
    let cols = if n <= 4 {
        n
    } else {
        (n as f32).sqrt().ceil() as u16
    };
    let rows = n.div_ceil(cols);

    let bank = grid_bank(
        "bench",
        "a",
        None,
        rows,
        cols,
        BayFormFactor::Other,
        TrayOrientation::Horizontal,
        BayOrder::RowMajor,
        BayOrigin::TopLeft,
        1,
    );

    BayTopology {
        schema_version: BAY_TOPOLOGY_SCHEMA_VERSION,
        label: "Unconfigured bench".to_string(),
        generated: true,
        auto_fill_unbound: true,
        enclosures: vec![Enclosure {
            id: "bench".to_string(),
            label: "Attached devices".to_string(),
            kind: EnclosureKind::Internal,
            banks: vec![bank],
            note: Some(
                "No bay topology configured — devices are shown in enumeration \
                 order and these positions do not reflect physical bays. \
                 Point --bay-topology at a config file, or pick a preset with \
                 --bay-profile."
                    .to_string(),
            ),
        }],
    }
}

// ---------------------------------------------------------------------------
// Built-in presets
// ---------------------------------------------------------------------------

/// A named starting point. Presets expand into the general model — they are
/// a convenience, not a closed set of supported hardware (ADR-0002).
pub fn preset(name: &str) -> Option<BayTopology> {
    match name {
        "arma-4u-32" => Some(arma_4u_32()),
        "dock-2bay" => Some(dock_2bay()),
        "nvme-carrier-8" => Some(nvme_carrier_8()),
        _ => None,
    }
}

pub fn preset_names() -> &'static [&'static str] {
    &["arma-4u-32", "dock-2bay", "nvme-carrier-8"]
}

/// The reference bench: a 4U chassis with two banks of front-loading
/// hot-swap trays either side of a ventilation column. Each bank is two
/// columns of eight trays; the trays lie on their side, so each tray face is
/// a wide horizontal bar and the eight of them stack up the column.
pub fn arma_4u_32() -> BayTopology {
    let left = grid_bank(
        "chassis",
        "left",
        Some("Bank A"),
        8,
        2,
        BayFormFactor::Lff35,
        TrayOrientation::Horizontal,
        BayOrder::ColumnMajor,
        BayOrigin::TopLeft,
        1,
    );
    let right = grid_bank(
        "chassis",
        "right",
        Some("Bank B"),
        8,
        2,
        BayFormFactor::Lff35,
        TrayOrientation::Horizontal,
        BayOrder::ColumnMajor,
        BayOrigin::TopLeft,
        17,
    );

    BayTopology {
        schema_version: BAY_TOPOLOGY_SCHEMA_VERSION,
        label: "Bench 1".to_string(),
        generated: false,
        auto_fill_unbound: true,
        enclosures: vec![Enclosure {
            id: "chassis".to_string(),
            label: "ARMA Industrial 4U — 32 bay".to_string(),
            kind: EnclosureKind::Rackmount,
            banks: vec![left, right],
            note: None,
        }],
    }
}

/// Two-bay top-loading hot-swap dock.
pub fn dock_2bay() -> BayTopology {
    let bank = grid_bank(
        "dock",
        "a",
        None,
        1,
        2,
        BayFormFactor::Lff35,
        TrayOrientation::Vertical,
        BayOrder::RowMajor,
        BayOrigin::TopLeft,
        1,
    );

    BayTopology {
        schema_version: BAY_TOPOLOGY_SCHEMA_VERSION,
        label: "Bench 1".to_string(),
        generated: false,
        auto_fill_unbound: true,
        enclosures: vec![Enclosure {
            id: "dock".to_string(),
            label: "2-bay hot-swap dock".to_string(),
            kind: EnclosureKind::Dock,
            banks: vec![bank],
            note: None,
        }],
    }
}

/// Eight-socket M.2 NVMe carrier / duplicator.
pub fn nvme_carrier_8() -> BayTopology {
    let bank = grid_bank(
        "carrier",
        "a",
        None,
        8,
        1,
        BayFormFactor::M2,
        TrayOrientation::Horizontal,
        BayOrder::RowMajor,
        BayOrigin::TopLeft,
        1,
    );

    BayTopology {
        schema_version: BAY_TOPOLOGY_SCHEMA_VERSION,
        label: "Bench 1".to_string(),
        generated: false,
        auto_fill_unbound: true,
        enclosures: vec![Enclosure {
            id: "carrier".to_string(),
            label: "NVMe carrier — 8 socket".to_string(),
            kind: EnclosureKind::NvmeCarrier,
            banks: vec![bank],
            note: None,
        }],
    }
}
