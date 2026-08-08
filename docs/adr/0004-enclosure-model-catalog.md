# Model-aware enclosure catalog, with a generic fallback that never pretends

**Status:** proposed (2026-08-06) — pending review. Do not implement until accepted.

Extends [ADR-0002](0002-configurable-bay-topology.md) (the topology model) and
builds on the bench-setup builder and identify mode.

## Context

ADR-0002 models a bench as grids of bays with a *form factor* — `3.5in`,
`m2` and so on. That is enough to place status correctly, and it is
deliberately generic: it works for hardware we have never seen, which was the
point.

But an operator does not think "a 2×8 grid of 3.5-inch bays". They think "the
ARMA chassis" or "the StarTech dock". The abstraction that made the model
universal also made the picture anonymous — every rackmount renders as the same
row of rounded rectangles, so the screen→hardware glance the bay map exists for
is weaker than it could be.

Three things become possible once the station knows the *model* rather than
just the shape:

1. **Recognition.** Artwork that looks like the gear on the bench is identified
   at a glance, not read.
2. **Auto-configuration.** A recognised model already knows its bay count,
   bank split, orientation and numbering — the operator confirms rather than
   describes.
3. **Model-specific functionality.** Locate-LED blink, per-bay power, hot-swap
   notification. These are exactly the things that make a bay map *actionable*
   rather than merely informative, and they are all model-specific.

The risk is equally clear: a catalog is a claim about the physical world. Show
someone a picture of a chassis that is not the chassis in front of them and the
bay map becomes worse than the abstract one, because they will trust it.

## Decision

### 1. An `EnclosureModel` catalog, referenced by topology — not replacing it

`Enclosure` gains an optional `model_ref`. Everything ADR-0002 already models
stays exactly where it is:

```
Enclosure {
  id, label, kind,
  model_ref: Option<ModelId>,   // NEW — "which real product is this?"
  banks: [Bank { rows, cols, form_factor, orientation, numbering, bays }],
}
```

A catalog entry is a **source of defaults and artwork**, never the source of
truth for a station's live layout. When the operator picks a model, its spec is
*expanded into* banks and bays exactly as a preset is today — and then it is
theirs to edit. A bench with a blanked-off bay, or a chassis someone rewired,
stays describable.

This is the single most important structural choice here, and it is a deliberate
repeat of ADR-0002's rejection of chassis templates: the catalog is a
convenience layer over the general model, not a replacement for it. The moment a
station's rendering depends on catalog lookup succeeding, an unlisted chassis
becomes a broken screen instead of a plain one.

### 2. Catalog entry schema

```jsonc
{
  "schema_version": 1,
  "id": "startech/sdock2u33",            // stable, vendor-namespaced
  "vendor": "StarTech",
  "product": "SDOCK2U33",
  "aliases": ["2-bay USB 3.0 dock"],     // what people actually call it
  "kind": "dock",

  // How we recognise it in the wild. Any match is a candidate; see §4.
  "identity": {
    "usb": [{ "vid": "0x174c", "pid": "0x55aa" }],
    "pci": [],
    "scsi_inquiry": [                     // SES / SCSI INQUIRY, trimmed+folded
      { "vendor": "StarTech", "product": "SDOCK2U33", "revision": null }
    ]
  },

  // Physical spec — expands into ADR-0002 banks/bays.
  "spec": {
    "banks": [{
      "label": null, "rows": 1, "cols": 2,
      "form_factor": "3.5in", "orientation": "vertical",
      "numbering": { "order": "row_major", "origin": "top_left", "label_start": 1 }
    }],
    "connectors": ["usb-b-3.0"],          // informational; helps the operator confirm
    "notes": "Top-loading; drives sit proud of the shell."
  },

  // Recognisable artwork. See §3.
  "art": {
    "kind": "svg_template",
    "shell": "…",                          // SVG for the housing
    "bay_slot": { "x": …, "y": …, "w": …, "h": … },  // where the bay grid lands
    "aspect": 0.72
  },

  // Optional. Present only for models we have actually verified. See §5.
  "capabilities": {
    "locate_led": false,
    "per_bay_power": false,
    "hotswap_notify": true,
    "ses_slot_addressing": false
  }
}
```

`capabilities` is **absent by default and never inferred**. A missing block
means "we do not know", which the UI renders as unavailable — not as "no". A
catalog that guesses at capabilities would have us offering a locate-LED button
that does nothing.

### 3. Rendering: recognisable art over a shared status layer

Art and status are **separate layers with a contract between them**, not one
drawing:

```
  ┌─ shell layer ─────────┐   per-model SVG: housing, vents, branding, buttons
  │   ┌─ bay slot ────┐   │   a declared rectangle the grid is drawn into
  │   │ status layer  │   │   the SAME renderer as today's generic bays
  │   └───────────────┘   │
  └───────────────────────┘
```

The status layer is exactly the component already in production. That is what
protects legibility: **status is drawn by the same code regardless of what is
behind it**, so a recognisable shell cannot quietly degrade the thing the map
exists for.

Three rules make that concrete, and they are testable:

- **Status colour is never on the art.** Shells render in neutral housing tones
  only. Any colour a bay carries comes from the status layer.
- **Contrast floor.** Shell tones sit in a restricted luminance band so every
  status colour keeps its contrast ratio against them. A model whose art
  violates the band fails catalog validation rather than shipping.
- **The bay slot is exclusive.** No shell artwork draws inside it — no drop
  shadows, no gloss, no logos crossing it.

If a model's art cannot satisfy those, the correct outcome is to ship the model
with generic art and its spec/capabilities intact. Recognition is worth
something; legibility is worth more.

### 4. Unknown models get the generic renderer, labelled

An enclosure with no `model_ref`, or a `model_ref` we cannot resolve, renders
exactly as it does today — the generic per-form-factor shape — and the UI says
so: *"Generic 3.5-inch rackmount — model not in catalog"*, with a link to
contribute one.

This is the ADR-0002 "honest default" rule applied again. The failure mode to
avoid is silently substituting a plausible-looking chassis of the same shape;
an operator who sees a picture that is nearly their gear will stop checking.

### 5. Auto-discovery seam

Detection is a **matcher over identifiers the backend reports**, defined now and
populated when `wipe-engine-linux` lands:

```rust
/// Hardware identity a backend can report about an enclosure it can see.
/// Empty today: the mock has no enclosures and no real backend exists.
pub struct EnclosureIdentity {
    pub usb: Option<UsbId>,          // VID/PID — docks, carriers, caddies
    pub pci: Option<PciId>,          // HBAs and NVMe carriers
    pub scsi_inquiry: Option<Inquiry>, // SES/SCSI vendor+product+revision — backplanes
    pub ses_enclosure_id: Option<String>,
}

pub trait EnclosureDiscovery: Send + Sync {
    fn enclosures(&self) -> Vec<EnclosureIdentity>;
}
```

Matching is deliberately conservative and **ranked, not boolean**:

| Rank | Signal | Confidence |
| --- | --- | --- |
| 1 | SES/SCSI inquiry vendor+product+revision exact | high |
| 2 | USB VID/PID or PCI vendor/device exact | high |
| 3 | Inquiry vendor+product, revision differs | medium |
| 4 | Alias/product-string fuzzy match | low — **suggest only** |

A high-confidence match **pre-selects** the model in the builder and says why
("detected: StarTech SDOCK2U33 via USB 174c:55aa"). It never silently rewrites a
saved topology — the operator's layout outranks our guess, the same rule that
made a saved layout outrank `--bay-profile` in ADR-0003. Medium and low
confidence surface as a suggestion the operator confirms.

`DeviceSimulator` set the precedent for how this ships: define the trait,
implement nothing that lies, and let the absence be visible.

### 6. Model-specific functionality seam

```rust
/// Actions a *known* model can offer. Every method returns
/// `Err(Unsupported)` unless a backend has actually implemented it for
/// this model — "we don't know" and "no" must not look the same.
pub trait EnclosureControl: Send + Sync {
    fn locate(&self, bay: &BayId, on: bool) -> Result<(), ControlError>;
    fn set_bay_power(&self, bay: &BayId, on: bool) -> Result<(), ControlError>;
    fn supported(&self) -> ControlCapabilities;
}
```

The UI derives affordances from `supported()`, not from the catalog's
`capabilities` block — the catalog says what the *model* can do, the control
seam says what *this station* can currently do about it. They differ whenever
the Linux backend is missing, the user lacks permission, or the SES device is
not exposed. Showing a locate button that fails is worse than not showing one.

`locate` is the first one worth building, because it closes the identify-mode
loop from the other end: today the operator tells us which bay a drive is in;
with locate, we can tell *them* — and a station that can blink the right tray
no longer needs identify mode for that chassis at all.

### 7. Catalog storage and extensibility

- **Bundled** with the binary as data (not code), so an air-gapped station has
  the catalog it shipped with. Consistent with ADR-0003: no network dependency
  for core function.
- **Extensible** from the same tiered config store — a station-local overlay
  directory, and later a control-plane-distributed catalog for a fleet. Local
  entries win over bundled ones for the same `id`, so a customer can correct our
  data without waiting for us.
- **Versioned** per entry, with a catalog-level `schema_version` that fails
  closed exactly like the topology document does.
- **Contribution** is a JSON file plus an SVG shell, reviewed against the §3
  legibility rules. The rules are the review checklist; art that fails ships as
  spec-only.

## Considered and rejected

- **Photographs of each model.** Highest recognition, and wrong: they cannot
  carry a status layer without overlay gymnastics, they raise licensing
  questions for gear we do not own, they are heavy to bundle on a PXE image, and
  they date badly. Vector shells give most of the recognition and all of the
  compositing control. (Already rejected once in ADR-0002; restated because a
  model catalog makes it newly tempting.)
- **Replace form factors with models.** Would make an unlisted chassis
  unrenderable. Form factor stays the floor; models are an enrichment.
- **Auto-apply a detected model to a saved topology.** Rejected for the same
  reason positional binding was: the operator's declaration outranks our
  inference, and a layout that silently rearranges itself after a reboot is
  exactly the kind of thing that destroys trust in the map.
- **Infer capabilities from `kind`.** "It's a rackmount, so it probably has
  locate LEDs" produces buttons that do nothing.
- **Ship art before the legibility rules.** Tempting to draw first and check
  later; the whole value of the bay map is that status reads instantly, and
  that is the property most easily lost to prettier artwork.

## Consequences

- `Enclosure` gains one optional field. Existing saved topologies remain valid
  and render exactly as before — the catalog is additive.
- The renderer splits into shell and status layers, which is work even for the
  generic case, but it is the split that keeps status legible.
- We take on a data-maintenance obligation. Wrong catalog data is worse than
  absent catalog data, so entries need provenance (who verified this, against
  what hardware) and the UI needs to make "unverified" visible.
- Bundle size grows with the catalog. Vector shells are small, but a
  thousand-model catalog on a PXE image is a real constraint — hence
  bundled-core plus extensible-overlay rather than one monolith.
- Three seams (`EnclosureDiscovery`, `EnclosureControl`, and the matcher) are
  defined here and unimplemented until `wipe-engine-linux` exists. Each must
  fail visibly rather than silently, per the `ControlPlaneStore` precedent.
