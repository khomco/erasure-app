# Bench setup — customer-facing bay layout builder

**Status:** accepted (2026-08-06). The core builder and identify mode are
built in `wipestation serve` and in the Tauri shell, which both start their
station through `wipe_server::bootstrap` and so both attach the simulator that
identify mode needs. Not built: SES auto-detect, custom labels, and the
Devices-page entry links (see the dated notes below). The Tauri shell still has
no saved store, persistent signing key or licence: a layout built there lasts
until the app quits.

Companion to [ADR-0002](../adr/0002-configurable-bay-topology.md) (the model)
and [ADR-0003](../adr/0003-station-config-persistence.md) (where a saved layout
lives).

## The gap

ADR-0002 made the topology data-driven, but every path to authoring one runs
through us: a JSON file, a CLI flag, or a preset compiled into the binary. The
presets exist as *starting points*, not a supported-hardware list — but with no
editor, a customer whose bench isn't one of the three has no move except hand-
editing JSON.

Every ITAD bench is hardware we have not seen: unknown bay counts, unknown bank
splits, unknown numbering, mixed form factors, several enclosures at once. The
layout has to be **the customer's to describe**.

## Principle

> The operator is drawing a picture of the bench in front of them. They should
> see the picture while they draw it, and the picture should be the same
> component that runs in production.

No separate "editor rendering" that drifts from the real bay map.

## Where it lives

New route `/bench-setup`, reached from:

- the Devices page header (**Configure bench**),
- the "Bench not configured" banner the bay map already shows,
- the "N attached devices not on the map" warning (deep-links to bindings).

> **As built (2026-11-08).** None of the three entry links is built. The route
> is reached only from the **Bench setup** sidebar item (`Layout.tsx`). The
> Devices page has no **Configure bench** header button; its "Bench not
> configured" banner names the `--bay-profile` / `--bay-topology` flags with no
> link; and the unplaced-devices warning says "Switch to Cards" with no deep
> link to bindings (`DevicesPage.tsx`).

## Screen

Two panes. Left is structure, right is the live bay map.

```
┌─ Bench setup ─────────────────────────────────────────────────────────┐
│ Bench label [Bench 1        ]   [Start from template ▾] [Import] [Export] │
│                                              [Discard changes] [Save]  │
├──────────────────────────────┬────────────────────────────────────────┤
│ ENCLOSURES              [+]  │  LIVE PREVIEW                          │
│                              │                                        │
│ ▾ ⠿ Supermicro 846      [🗑] │   ┌──────────────────────────────┐     │
│   Kind    [Rackmount   ▾]    │   │ Supermicro 846 — 24 bay      │     │
│                              │   │ ┌──┬──┬──┬──┬──┬──┐          │     │
│   ▾ Bank A              [🗑] │   │ │1 │2 │3 │4 │5 │6 │          │     │
│     Rows [4] × Cols [6]      │   │ ├──┼──┼──┼──┼──┼──┤          │     │
│     Form factor [3.5in  ▾]   │   │ │7 │8 │9 │10│11│12│          │     │
│     Trays   [Horizontal ▾]   │   │ └──┴──┴──┴──┴──┴──┘          │     │
│     Numbering                │   └──────────────────────────────┘     │
│       Start corner  ┌─┬─┐    │                                        │
│                     │●│ │    │   ┌────────────┐                       │
│                     ├─┼─┤    │   │ 2-bay dock │                       │
│                     │ │ │    │   │ ┌──┬──┐    │                       │
│                     └─┴─┘    │   │ │1 │2 │    │                       │
│       Run  (•) Across rows   │   │ └──┴──┘    │                       │
│            ( ) Down columns  │   └────────────┘                       │
│       Start at [1]           │                                        │
│       → 1,2,3 … 24           │  ── Click any bay to edit it ──        │
│       [Custom labels…]       │                                        │
│                       [+ Bank]│  ⚠ 2 problems  ▸                      │
│                              │                                        │
│ ▸ ⠿ StarTech 2-bay dock [🗑] │                                        │
│ ▸ ⠿ DiskClon NVMe-8     [🗑] │                                        │
└──────────────────────────────┴────────────────────────────────────────┘
```

Editing anything on the left re-renders the right immediately. Nothing is
written to the station until **Save**.

### Bay inspector

Clicking a bay in the preview opens an inspector for that bay:

- **Label** — free text, defaults to the numbering run. This is what is
  silkscreened on the metal; if a vendor labels bays `A1..A8`, type that.
- **Blanked off** — renders as a filler panel, excluded from auto-fill.
- **Form factor override** — for a 2.5" sled in a 3.5" caddy.
- **Binding** — see below.

### Numbering run control

The single most fiddly thing to get right, so it gets explicit controls rather
than a text field: a 2×2 **start corner** picker, an **across rows / down
columns** toggle, and a **start at** number (0- or 1-based falls out of it).
Under them, a live `→ 1,2,3 … 24` echo so the operator can check the run
against the metal without counting squares in the preview.

`[Custom labels…]` is the escape hatch for benches that follow no run at all —
a textarea, one label per line, applied in grid order.

> **As built (2026-11-08).** Custom labels are not built: there is no
> `[Custom labels…]` control and no bulk-label code in `BenchSetupPage.tsx` or
> `topologyEdit.ts`. A bench that follows no run is labelled bay by bay in the
> inspector's **Label** field.

## Binding: how a bay learns which drive is in it

This is where the product either earns trust or loses it. Positional guessing
is rejected (ADR-0002); the question is how a customer declares bindings for
hardware we know nothing about.

Four modes, in increasing order of trustworthiness:

1. **Auto (enumeration order)** — the current default. Fast to set up, and
   honest about what it is: the UI labels these bays *positions not verified*.
   Fine for a two-bay dock, actively misleading on a 24-bay rack.

2. **Manual** — pick a device from a dropdown per bay. The inspector states the
   trade-off in one line, because it is not obvious and it matters:
   > **By path** (`/dev/sdb`) pins the *port* — the bay keeps its identity when
   > you swap drives. **By serial** pins the *drive* — it follows that specific
   > disk between bays.
   >
   > Intake benches want **path**. Reference/golden drives want **serial**.

3. **Learn by hot-swap ("Identify bays")** — **BUILT.** The flagship flow, and
   the answer to "how do we map a chassis nobody has ever seen":

   > Enter identify mode. Pull or insert a drive in a real bay. The station's
   > device list changes; the UI catches the delta and asks
   > **"`WDS500G3X0E` just appeared — which bay did you put it in?"**
   > The operator clicks that bay in the preview. A `path` binding is written.
   > Repeat down the bench.

   No vendor data, no SES, no guessing — the operator's hands are the sensor.
   Removal works the same way ("`…` was removed — which bay did it leave?").
   A progress counter shows *k of N bays identified*, so a half-mapped bench is
   visibly half-mapped.

4. **SES auto-detect** — one button that fills every bay at once, available when
   a backend reports device slot numbers. `wipe-engine-linux` does not exist, so
   this ships disabled with an explanatory tooltip rather than being hidden;
   the `BayBinding::SesSlot` variant is already in the model for it.

   > **As built (2026-11-08).** Not built, and not shown disabled either: the
   > inspector offers only Auto, by path and by serial (`BenchSetupPage.tsx`),
   > with no SES (SCSI Enclosure Services) option, button or tooltip. Nothing
   > in the product detects slots through SES; only the
   > `BayBinding::SesSlot` variant exists (`crates/wipe-common/src/bay.rs`).

### As built

Identify mode watches the station's own device list (polled hard, 1s, while
running — the operator pushes a tray home and expects to be asked at once) and
diffs snapshots **by device id, not path**: a port can be reused by the next
drive, and treating that as "still the same device" would silently skip a bay
the operator is waiting to map. The first snapshot after entering seeds the
baseline, so drives already in the bench are not queued as if they had just
appeared.

Answering writes `{ by: "path" }` for the clicked bay and clears any other bay
that claimed the same path — two bays claiming one port would make the map
ambiguous, and the most recent answer is the one the operator just gave with
their hands.

Because the mock backend has no hands, `DeviceSimulator` stands in for them,
and the panel says so rather than pretending the drives are real. The
`/api/sim/*` routes are always mounted (`crates/wipe-server/src/app.rs`); they
answer only when the host has called `AppState::with_simulator`, and return 404
otherwise. `wipestation serve` and the Tauri shell both attach their mock
backend that way, through the shared `wipe_server::bootstrap::build_station`.

## Templates

**Start from template ▾** offers the built-in presets plus **Empty bench**.
Picking one seeds the editor and everything stays editable. The menu says so:

> Templates are starting points, not a hardware compatibility list. Pick the
> closest one and change it, or start empty.

> **As built (2026-11-08).** The menu (`TemplateMenu` in `BenchSetupPage.tsx`)
> is its own list built in TypeScript — "Empty bench", "Rackmount · 24 bay
> (4 × 6)", "Hot-swap dock · 2 bay", "NVMe carrier · 8 socket" — not the Rust
> built-in presets
> (`preset_names()` in `bay.rs`: `arma-4u-32`, `dock-2bay`, `nvme-carrier-8`).
> The two lists differ; the 32-bay reference chassis is not offered as a
> template. It is still reachable as the catalog enclosure model
> `arma/industrial-4u-32` (`crates/wipe-common/data/catalog.json`) through the
> builder's **From catalog** model picker, and as `--bay-profile arma-4u-32`.
> The menu's shipped sentence differs from the design's wording: "Starting
> points, not a hardware compatibility list. Pick the closest and change it, or
> start empty."

## Validation

Shown live in the preview pane, never blocking typing, blocking only **Save**:

| Severity | Condition |
| --- | --- |
| Error | Duplicate bay label within a bank; duplicate bay id; bank with 0 rows or 0 columns; unparseable custom-label list |
| Warning | Bay bound to a device that is not currently attached; attached device no bay claims; bench has zero bays; auto-fill on for a bench larger than ~8 bays (the point where unverified positions start to mislead) |

> **As built (2026-11-08).** The builder runs a client-side subset
> (`validateLocal` in `apps/desktop/src/bench/topologyEdit.ts`: duplicate
> enclosure id, empty grid, duplicate bay label within a bank, zero bays,
> auto-fill on more than 8 bays) plus a server dry run. The full rule set is
> Rust `BayTopology::validate` (`bay.rs`), which also checks schema version,
> duplicate bay id and a bay outside its bank's grid. Not built: the
> custom-label parse error (there are no custom labels) and both
> device-attachment warnings (bound device not attached; attached device no bay
> claims). Unclaimed devices surface only as the Devices page's "not on the
> map" warning, not as a validation problem.

## Persistence

Per ADR-0003: **Save** does `PUT /api/bay-topology`, which validates, writes the
JSON atomically, and hot-reloads without a restart. **Export** downloads the same
document — the path for read-only/PXE stations and for rolling one layout out to
a fleet of identical benches. **Import** accepts it back.

> **As built (2026-11-08).** `PUT /api/bay-topology` goes through
> `AppState::save_bay_topology`. The file store writes beside the target and
> renames (`LocalFileStore::save` in `crates/wipe-server/src/store.rs`), so the
> write is atomic within one filesystem. It persists only when the station has
> a writable store tier; otherwise the save holds until restart and the page
> says so. The Tauri shell builds its station with `Persistence::InMemory`
> (`wipe_server::bootstrap`), so its store is in RAM only and a layout saved
> there is still lost when the app quits (story 2 moves it to an app-data
> directory).

## Model changes this needed (all four done)

When this was written, the model rendered a topology but could not round-trip
one through an editor. Four gaps, all confirmed against the code at the time.

> **As built (2026-11-08).** All four are done. Each item below gives the gap
> as it stood when this was written, then the symbol that closed it.

1. **Banks did not remember their numbering run.** `order`, `origin` and
   `label_start` were arguments to `grid_bank()` and were discarded once bays
   were generated, so a re-opened topology could not show — let alone change —
   how it was numbered. The design: add `Bank.numbering: Option<NumberingRun>`
   and keep the expanded `bays` authoritative so per-bay overrides survive
   regeneration. **Done:** `Bank.numbering: Option<NumberingRun>` (`bay.rs`).
2. **`BayId` embedded the label** — `"chassis.left.7"`. Renaming a bay changed
   its identity, silently orphaning anything that referenced it. Bay ids should
   be opaque and stable; the label becomes free text. **Done:** `grid_bank`
   mints `"{enclosure}.{bank}.r{row}c{col}"`, independent of the label
   (`bay.rs`; mirrored in `topologyEdit.ts`).
3. **The renderer ignored per-bay form factor.** The model stored
   `Bay.form_factor`, and `BayMap.cellSize()` read only `Bank.form_factor` —
   verified live at the time: a bay overridden to `2.5in` in a `3.5in` bank
   drew at full size. Fix: cell geometry from the bank, tray drawn inset per
   bay. **Done:** `traySize` in `apps/desktop/src/bench/BayMap.tsx`.
4. **There was no `revision` field** for the concurrent-edit guard ADR-0003
   wants. **Done:** `BayTopology.revision` (`bay.rs`), checked by
   `AppState::save_bay_topology`.

Two rendering issues were also noted, neither blocking; the first is done and
the second was not re-checked on 2026-11-08:

- Enclosures stacked vertically, one SVG each, so a three-enclosure bench was a
  tall column with dead space beside it; they should flow and wrap.
  **Done (as built 2026-11-08):** enclosures sit in a `flex-wrap` row in
  `BayMap.tsx`.
- Bay labels get cramped at `m2` size (as written; not re-checked on
  2026-11-08).

## Deliberately out of scope

- Pushing one layout to many stations (fleet rollout) — export/import covers it
  until the Lead has real responsibilities.
- Drag-and-drop repositioning of individual bays. Grid + numbering run + custom
  labels covers the hardware surveyed; freeform placement is a much larger
  editor and no surveyed bench needs it.
- Photographic or vendor-supplied chassis artwork (rejected in ADR-0002).
- Restricting who may edit — inherits RBAC (v0.2 #6/#7); noted in ADR-0003.
