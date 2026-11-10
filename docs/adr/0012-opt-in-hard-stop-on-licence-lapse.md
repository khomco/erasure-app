# A customer may opt in to a hard stop when its licence lapses

**Status:** superseded (2026-11-09) by [ADR-0013](0013-licensing-controls-every-erasure.md): the
stop is no longer an opt-in. Every licence refuses new starts it does not cover, and the
lapse-policy setting is retired. §4, §5 and §7 carry over into ADR-0013 §1. Kept for its history.

**Amends [ADR-0005](0005-licensing-and-attestation.md) §5 and ROADMAP §0.2**, stated explicitly:
"never block a wipe on licensing" stays the default but is no longer absolute. A customer may
choose, per organization, that its stations refuse to *start* new jobs once their licence has
lapsed. Without that choice, nothing changes: the station keeps erasing and marks its
certificates `evaluation`. This also ratifies the evaluation stance (DECISIONS D3): evaluation
is the default, and it is not time-bombed. Numbered 0012 because 0008–0010 are reserved for
ROADMAP §6 candidates.

## Context

§0.2 exists so that **the vendor**, or a licence server having a bad day, can never stop a
customer from sanitizing drives. Erasure runs on someone else's schedule, and an air-gapped
facility cannot phone home.

The owner raised the opposite risk from the customer's side. An ITAD business or enterprise
under an audit regime (R2v3, NAID AAA, a contract that requires licensed tooling) may treat an
`evaluation` certificate as a compliance failure. If a station quietly drops to `evaluation` at
renewal time, for example on an air-gapped site whose new licence was never installed, weeks of
certificates can be issued before anyone notices. Such a customer would rather the station
stopped, so that someone installs a legitimate licence before work continues.

The two concerns do not conflict once the choice belongs to the customer. ADR-0011 already makes
a similar safeguard (destruction concurrence) the site's choice rather than the vendor's.

## Decision

A licence carries a **lapse policy**: `evaluation` (the default) or `stop`.

1. **Off by default.** Licences issued before this ADR, and licences without the field, behave
   as `evaluation`. The field deserializes with a default, so existing licence files keep
   verifying and keep their current behaviour.
2. **The customer chooses it in the portal.** An organization owner sets the policy for the
   organization. The change is audited, and it applies to licences issued or reissued after
   it. The portal says that existing licence files must be reinstalled to pick it up.
3. **It lives inside the signed licence, never in station configuration.** An operator under
   time pressure cannot switch it off to keep working on `evaluation` certificates, which is
   the outcome the customer chose to prevent. Being inside the signed file also makes it work
   on air-gapped stations, with no network.
4. **It refuses to start new jobs, and never interrupts one in progress.** A job already
   erasing when the licence lapses runs to completion and is certified as the licence state at
   signing dictates. Stopping halfway would leave a partly erased drive with no certificate,
   which is worse for compliance than either outcome. Destruction recording follows the same
   rule.
5. **It triggers only on the licence itself:** expiry, revocation the station knows about, or a
   detected clock rollback (treated as expired). It **never** triggers because a hub or licence
   server is unreachable. A station holding a valid licence keeps working with no network,
   forever, which preserves §0.2's original protection.
6. **The bench warns before it happens.** Expiry warnings appear at 30, 7 and 1 days for every
   licence. Under `stop`, the warning says that new jobs will be refused at expiry. After
   expiry, the station shows why it refuses and how to install a licence (by file, offline).
7. **No emergency override.** An override would bring back the `evaluation` output the customer
   opted out of. If customers ask for one, it is a new decision: for example, a supervisor
   approving a one-off run with a recorded reason.

## Consequences

- `wipe-license` gains a `lapse_policy` field in the signed licence body, with a serde default
  of `evaluation`. The hub's issuance (`licence-delivery`) writes it from the organization's
  setting.
- Starting a job (the `JobRunner` path behind `start_job`) consults the installed licence's
  status and lapse policy. Under `stop` with a lapsed licence, it refuses with a clear error.
  Nothing else is gated: enumeration, viewing and verifying certificates, and installing a
  licence all keep working.
- Automatic renewal (offerings-and-payments story 4, Stripe subscriptions) makes lapses rare
  for paying customers, but not impossible. A failed card stops renewal. An air-gapped station
  still needs the renewed file installed by hand until stations can fetch licences themselves
  (DECISIONS D6, ADR candidate 0008).
- ROADMAP §0.2, L-R1, L-R5, the `bmad-build-auto` invariant and the SPECs that cite §0.2 now
  carry this exception. Anything else that would gate erasure still needs its own explicit
  decision.
