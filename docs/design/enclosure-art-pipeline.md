# Adding an enclosure model to the catalog

**Status:** accepted (2026-08-13). The pipeline and the starter set are built.

Implements the storage and rendering halves of
[ADR-0004](../adr/0004-enclosure-model-catalog.md). Read that first for *why*
model-aware artwork exists and what it is not allowed to cost.

## What this document is for

The catalog is meant to grow, mostly by people who are not us: a field
engineer with a chassis in front of them, a customer with fifty of something we
have never seen. That only works if adding a model is a **half-hour job with a
checklist**, and if the checklist cannot be got subtly wrong.

So the process is deliberately boring, and the parts that matter are enforced
by tests rather than by review:

| Concern | Enforced by |
| --- | --- |
| Catalog JSON parses, ids unique, grids non-empty | `crates/wipe-common/tests/catalog.rs` |
| Every model expands to a savable topology | same |
| Artwork exists, matches the kind, is reachable | `apps/desktop/src/bench/shells/catalog.test.ts` |
| Artwork stays legible (the three rules) | `apps/desktop/src/bench/shells/contract.test.ts` |
| The editor's expansion agrees with the Rust one | `shells/catalog.test.ts` |

If those pass, the model is safe to ship. Nothing else about it is a judgement
call a reviewer has to make correctly every time.

## Where things live

```
crates/wipe-common/data/catalog.json     the models (bundled, include_str!)
crates/wipe-common/src/catalog.rs        the schema, matching, expansion
apps/desktop/src/bench/shells/
  types.ts        the ShellDef contract + the three rules
  tokens.ts       the only colours artwork may use
  models.tsx      the per-model artwork
  GenericShell.tsx  the labelled fallback
  registry.ts     lookup + fitting the bank layer into a shell
  contract.test.ts  the rules, as machine checks
  catalog.test.ts   catalog ↔ artwork agreement
```

## Step 1 — add the catalog entry

Append to `crates/wipe-common/data/catalog.json`:

```json
{
  "id": "vendor/short-model",
  "vendor": "Vendor",
  "product": "Model Name",
  "aliases": ["what people call it on the bench"],
  "kind": "rackmount",
  "identity": { "scsi_inquiry": [{ "vendor": "VENDOR", "product": "MODEL" }] },
  "spec": {
    "banks": [
      { "rows": 8, "cols": 2, "form_factor": "3.5in", "orientation": "horizontal",
        "order": "column_major", "origin": "top_left", "label_start": 1 }
    ],
    "connectors": ["SFF-8643 x2"],
    "notes": null
  }
}
```

Rules that are actually rules, not style:

- **`label_start` continues across banks** when the chassis numbers straight
  through (bank B starting at 17 on a 32-bay). Bay labels are what is
  silkscreened on the metal; getting them wrong sends an operator to the wrong
  tray, which is the whole failure this feature exists to prevent.
- **Omit `capabilities` unless you have verified them on the hardware.** Absent
  means "we don't know". It must not render the same as "no" — and a
  `locate_led: true` we guessed produces a button that does nothing.
- **`verified_by` is required if you claim capabilities.** Name and date. Wrong
  catalog data is worse than absent catalog data.
- **`identity` may be empty.** Nothing populates enclosure identity today (see
  ADR-0004 §5); an entry with no identity is still fully usable from the
  builder's model picker.
- **`art` is optional and comes later.** Ship the spec first.

Then:

```bash
cargo test -p wipe-common
```

At this point the model is already useful: it appears in the builder's *From
catalog* picker, expands to the right grid, and renders on the generic shell
labelled `generic <kind> outline`. **That is a complete, supported outcome.**
Most models should stop here.

## Step 2 — add artwork, only if it earns its place

Artwork is worth adding when the chassis has a silhouette an operator would
recognise across a room: a toaster dock, a duplicator with a keypad, an open
cage. If the answer is "a dark rectangle with bays in it", the generic shell
already draws that, and better.

Add a `ShellDef` to `models.tsx`:

```tsx
const W = 320, H = 200;

export const myChassis: ShellDef = {
  key: "vendor-shortname",          // must equal the model's `art`
  title: "Human name — what it is",
  kinds: ["rackmount"],             // kinds this art is honest for
  viewBox: { w: W, h: H },
  baySlot: { x: 20, y: 16, w: W - 40, h: H - 60 },
  render: () => (
    <g>
      <rect x={0.5} y={0.5} width={W - 1} height={H - 1} rx={6}
            fill={T.body} stroke={T.edge} />
      {/* front panel, buttons, vents — anywhere but the bay slot */}
    </g>
  ),
};
```

Register it in `MODEL_SHELLS`, set `"art": "vendor-shortname"` on the catalog
entry, and run:

```bash
cd apps/desktop && npx vitest run src/bench/shells
```

### The three rules the tests enforce

1. **Housing tones only** — every `fill`/`stroke` must come from
   `SHELL_TOKENS`. Shapes may use the six tones; only `<text>` may use the two
   ink tones.
2. **Under the luminance ceiling** — shape tones stay below
   `MAX_SHELL_LUMINANCE`, so every status colour keeps its contrast against
   whatever housing is behind it.
3. **No detail in the bay slot** — a shape must either span the whole slot (a
   flat backing panel) or stay clear of it. Half-intruding logos, vent holes
   and markings show through the gaps between trays.

Two consequences worth knowing before you start drawing:

- **No `<path>`, no `transform`.** Not stylistic: the slot checker measures
  bounding boxes, and either of those would let artwork sit inside the slot
  unseen. Compose from `rect`, `circle`, `ellipse`, `line`, `polygon`, `text`.
- **The shell renders *before* the status layer**, always. An early draft of
  the toaster dock drew its housing last and clipped the bay labels; the order
  is now fixed in `BayMap` and is not a shell's decision.

### Sizing

`baySlot` is where the bank grid goes. `fitToSlot` scales the bank layer
uniformly to fit and centres it, then `renderWidth` grows the whole canvas so
bays come out at their natural pixel size regardless of how much bezel
surrounds them. So the slot's *aspect ratio* matters more than its absolute
size — make it roughly the shape of the bank grid the model declares, or the
chassis will render with large empty margins.

## Step 3 — look at it

```bash
cargo run -p wipe-cli -- serve --port 7878 --bay-profile arma-4u-32
```

Open `/bench-setup`, add the model from *From catalog*, and check the preview
with **Show live drives** on. Then check the same layout on `/devices` — same
renderer, so if it reads there it reads everywhere.

## What is deliberately not in this pipeline

- **No auto-discovery step.** `EnclosureDiscovery` exists as a trait with no
  implementation; nothing probes hardware today, so nothing can auto-select a
  model. The identity fields are populated by hand and matched only when a
  backend eventually reports identity (ADR-0004 §5).
- **No capability wiring.** `EnclosureControl` is a seam. Declaring
  `locate_led: true` records a fact about the hardware; it does not light
  anything.
- **No per-model behaviour hooks.** A model changes what is *drawn* and what is
  *pre-filled*. If a model ever needs to change what the station *does*, that
  is a new ADR, not a catalog field.

## Site-local overlays

A customer does not have to wait for a release to fix a wrong entry. An overlay
catalog merges over the bundled one by id — replacing an entry or adding a new
one — through `AppState::with_catalog_overlay`, and the UI reads the merged
result from `GET /api/enclosure-catalog`. Overlay entries with no matching
artwork render generic, like any other unlisted chassis.
