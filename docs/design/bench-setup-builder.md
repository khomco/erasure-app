# Bench setup — customer-facing bay layout builder

**Status:** proposed (2026-08-06) — pending review. Nothing here is built yet.

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

3. **Learn by hot-swap ("Identify bays")** — the flagship flow, and the answer
   to "how do we map a chassis nobody has ever seen":

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

## Templates

**Start from template ▾** offers the built-in presets plus **Empty bench**.
Picking one seeds the editor and everything stays editable. The menu says so:

> Templates are starting points, not a hardware compatibility list. Pick the
> closest one and change it, or start empty.

## Validation

Shown live in the preview pane, never blocking typing, blocking only **Save**:

| Severity | Condition |
| --- | --- |
| Error | Duplicate bay label within a bank; duplicate bay id; bank with 0 rows or 0 columns; unparseable custom-label list |
| Warning | Bay bound to a device that is not currently attached; attached device no bay claims; bench has zero bays; auto-fill on for a bench larger than ~8 bays (the point where unverified positions start to mislead) |

## Persistence

Per ADR-0003: **Save** does `PUT /api/bay-topology`, which validates, writes the
JSON atomically, and hot-reloads without a restart. **Export** downloads the same
document — the path for read-only/PXE stations and for rolling one layout out to
a fleet of identical benches. **Import** accepts it back.

## Model changes this needs

The current model renders a topology but cannot round-trip one through an
editor. Four gaps, all confirmed against the code:

1. **Banks do not remember their numbering run.** `order`, `origin` and
   `label_start` are arguments to `grid_bank()` and are discarded once bays are
   generated. Re-open a saved topology and the editor cannot show — let alone
   change — how it was numbered. Add `Bank.numbering: Option<NumberingRun>` and
   keep the expanded `bays` authoritative so per-bay overrides survive
   regeneration.
2. **`BayId` embeds the label** — `"chassis.left.7"`. Renaming a bay changes its
   identity, silently orphaning anything that referenced it. Bay ids should be
   opaque and stable; the label becomes free text.
3. **The renderer ignores per-bay form factor.** The model stores
   `Bay.form_factor`, and `BayMap.cellSize()` reads only `Bank.form_factor` —
   verified live: a bay overridden to `2.5in` in a `3.5in` bank draws at full
   size. Fix: cell geometry from the bank, tray drawn inset per bay.
4. **No `revision` field** for the concurrent-edit guard ADR-0003 wants.

Two rendering issues also want attention, neither blocking:

- Enclosures stack vertically, one SVG each, so a three-enclosure bench is a
  tall column with dead space beside it. They should flow and wrap.
- Bay labels get cramped at `m2` size.

## Deliberately out of scope

- Pushing one layout to many stations (fleet rollout) — export/import covers it
  until the Lead has real responsibilities.
- Drag-and-drop repositioning of individual bays. Grid + numbering run + custom
  labels covers the hardware surveyed; freeform placement is a much larger
  editor and no surveyed bench needs it.
- Photographic or vendor-supplied chassis artwork (rejected in ADR-0002).
- Restricting who may edit — inherits RBAC (v0.2 #6/#7); noted in ADR-0003.
