# Configurable bay topology, declared per station and rendered as vector artwork

**Status:** accepted (2026-08-06)

## Context

The bench-status overlay (v0.2 #3) gave operators per-device status, but
it renders as a responsive grid of cards in device-enumeration order.
That order has nothing to do with where the drive physically is. An
operator who sees "MX500 1TB — needs attention" still has to read the
serial off the screen and then hunt along a 32-tray chassis for a
matching label. The walk-away workflow the overlay was built for is
exactly the workflow where that hunt is most expensive: come back after
two hours, six bays need attention, and each one is a linear search.

The reference bench is an ARMA Industrial 4U chassis with two banks of
front-loading hot-swap trays either side of a ventilation column, and
NVMe carriers sitting next to it. Nothing about "two banks of vertical
trays, separated by a gap" is derivable from `/api/devices`.

Surveying the hardware ITAD benches actually use, the shapes are varied
but not unbounded:

- **Rackmount multi-bank hot-swap** — 8–36+ front trays in banks; 4U
  designs commonly put 2 columns per bank with the banks separated by
  I/O or ventilation. Trays are tool-less and take 3.5" or 2.5".
- **Standalone duplicator/eraser** — 1–4 top-loading bays with an OSD
  (e.g. StarTech SATDOCK4U3RE).
- **Open / "toaster" dock** — 2 vertical top-loading slots.
- **NVMe carrier / duplicator** — a row of M.2 sockets (e.g. 8-up).
- **Multi-interface sanitizer** — 12+ mixed SAS/SATA/NVMe positions.
- **Single-drive USB caddy** — one bay.

The invariants across all of them: bays are grouped into one or more
grids; a grid has a form factor, an orientation, and a numbering run
whose origin and direction vary by vendor; and the operator-facing label
is whatever is silkscreened on the metal, which is frequently 1-based,
sometimes 0-based, and occasionally not a number at all.

## Decision

**Model the bench as `BayTopology → Enclosure → Bank → Bay`, declare it
as station-scoped configuration, resolve bay→device server-side, and
render it as generated SVG.**

Four decisions, each with a rejected alternative worth remembering.

### 1. A grid-of-banks model, not a per-chassis template

A `Bank` declares `rows`, `cols`, `form_factor`, `orientation`, and a
numbering run (`order`, `origin`, `label_start`, optional explicit
labels). An `Enclosure` composes Banks left-to-right with a declared
gap. Everything else — the ARMA chassis, a 2-bay dock, an 8-up NVMe
carrier — is a parameterisation of that.

*Rejected: a library of named chassis templates* (`arma-4u`,
`supermicro-846`, …) *baked into the binary.* It looks friendlier and is
a trap: the template set can never keep up with what ITAD shops actually
have on the floor, and the first bench that doesn't match forces either
a code change or a lie on screen. Named profiles still exist — but as
**presets that expand into the general model**, not as a closed set. A
customer with an unlisted chassis writes twelve lines of JSON, not a
support ticket.

### 2. Bay→device binding is a declared rule, not positional luck

`BayBinding` is one of: `ses_slot`, `path`, `serial`, `wwn`,
`device_id`, or `unbound`. Unbound bays are filled from the device
enumeration in order, and a topology can disable that fallback.

`ses_slot` is the one that matters for real hardware and is why binding
is an enum rather than a string. SAS-3 expanders and SES enclosure
services report a device slot number (0–255, 255 meaning "no slot"),
which is the only mechanism that reliably answers "which physical tray
is `/dev/sdq` in" — the same mechanism `sg_ses --dev-slot-num=N
--set=ident` uses to blink the locate LED. `wipe-engine-linux` does not
exist yet, so nothing populates it today; the variant exists so that
landing SES support later is a backend change, not a topology-model
change.

*Rejected: bind by array position — nth device to nth bay.* It is
correct exactly once, on a bench where every bay is populated, nothing
is hot-swapped, and enumeration order happens to match physical order.
The moment an operator pulls a drive from bay 3, every bay after it
silently shifts and the screen starts pointing at the wrong metal. A
bay map that is confidently wrong is worse than no bay map: the
operator's whole reason to trust it is that they stop double-checking.

### 3. Resolution server-side; the frontend is a renderer

`GET /api/bay-topology` returns the geometry **with each bay's resolved
`device_id` already filled in**. The frontend joins that against the
`/api/devices` and `/api/jobs` data it already fetches for the
bench-status overlay.

*Rejected: ship raw config to the frontend and resolve there.* Matching
a WWN or an SES slot to a device is domain logic, it needs the device
list, and it needs tests. Rust already has both. Resolving client-side
would either duplicate that logic in TypeScript or force a second
resolution pass when the TUI and any future consumer arrive.

### 4. Generated SVG, not images or CSS boxes

Bays render as vector artwork produced from the model — crisp at any
zoom, themeable, status-colourable, and diffable in review.

*Rejected: photographs or vendor artwork of each chassis.* They cannot
carry live status without overlay gymnastics, they are a licensing
question, and they go stale. *Rejected: plain CSS/flex boxes.* They are
easier, but the whole value is physical resemblance — an operator
recognising the shape of their own hardware at a glance. A grid of
rounded rectangles is what we already have on the Devices page.

### Default is an honest fallback

A station with no bay configuration does **not** get a plausible-looking
chassis. It gets a single auto-sized bank explicitly labelled as
unconfigured, with bays bound by enumeration order. A bay map that
invents hardware the station doesn't have would undermine the one
property that makes it useful.

## Consequences

- `Empty` joins the slot-status vocabulary. The bench-status overlay had
  no notion of it because a card only exists if a device does; a Bay
  exists whether or not anything is in it, and "which slots are free for
  fresh intake" was one of the three questions v0.2 #3 set out to answer.
- Bay labels are operator-facing strings, never our indices. Vendors
  number from 0 or 1, top-left or bottom-left, row-major or
  column-major. `order`/`origin`/`label_start` cover the common runs and
  explicit per-bay labels cover the rest.
- The topology is station-scoped, so a roaming tablet pointed at station
  B renders B's hardware. This follows the existing one-origin rule: the
  station serves its own UI and its own truth.
- Nothing here touches the cert. A Bay is where a drive sat on a bench,
  not evidence about the erasure — if bay provenance ever belongs in the
  audit chain, that is a separate decision.
- No persistence layer is added. Topology is read at startup from a
  file, consistent with the PXE-ephemeral discipline in CONTEXT §7.
