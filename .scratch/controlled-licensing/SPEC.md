---
id: SPEC-controlled-licensing
companions: [../../docs/adr/0013-licensing-controls-every-erasure.md, ../../docs/adr/0008-station-enrollment-and-licence-fetch.md, ../../docs/adr/0005-licensing-and-attestation.md, ../../docs/adr/0007-hub-accounts-payments-persistence.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# Licensing controls every erasure

## Why

Owner decision, 2026-11-09 (ADR-0013): control is the baseline. No station erases without a licence, evaluation included. Erasure quotas are a pool the whole organization draws on, held on each station as a reserve so a start never waits on the network. Renewals are charged 30 days early so no bench ever stops on renewal day. `licence-visibility` story 5 builds the station's start gate; this epic builds everything around it at the hub, the portal, the station's credit reserve and the public copy.

## Capabilities

- **CAP-1**
  - **intent:** A prospect evaluates through a free trial licence, not an unlimited unlicensed station.
  - **success:**
    - A vendor admin can publish an offering marked as a trial: price zero, a credit quota (25 by default in the admin form) and a term. Trials are one-off, machine or site scope as any offering.
    - A signed-in owner of a verified organization takes a trial without payment (no checkout session); the entitlement is granted at once and audited. An organization can take each trial offering once; a second attempt is refused with a message saying so.
    - The public offerings API and the pricing page list a trial with a "Start free trial" action instead of a price.
    - Hub tests cover take, repeat refusal, unverified account refusal and the audit row; portal and site vitest cover the new states.
- **CAP-2**
  - **intent:** The retired lapse-policy setting and the "licensing never blocks erasure" claims are gone everywhere a customer could read them.
  - **success:**
    - The portal's lapse-policy setting and its routes are removed; issued licences omit the field (stations already ignore it, `licence-visibility` story 5). Existing rows and audit history stay.
    - The marketing site's "Licensing never blocks erasure" capability and its copies on the product, pricing and enterprise-IT pages are replaced with: "**Licensing decides at the start, never mid-erasure.** A station checks its licence before each job starts. A job that started covered is always certified, however long it runs, and a station with a valid licence never needs a network connection to keep working." The capability's status in `capabilities.ts` matches what has shipped at that commit.
    - README and CONTEXT match.
  - **checkpoint:** the replacement site wording is drafted by the agent, not yet owner-approved; the owner approves the finished story before it counts as done.
- **CAP-3**
  - **intent:** An organization's erasure quota is one pool at the hub, handed to stations as signed credit grants.
  - **success:**
    - `wipe-license` gains a credit grant: grant id, organization, instance key id, credits, issued time, signed under the vendor root and verified offline against the same roots as licences; a v-tagged, canonical body like the licence.
    - The hub tracks, per entitlement with a quota, credits purchased, granted per station, and returned; the portal shows the pool and each station's last reported balance.
    - Issuing or reissuing a licence for an entitlement with a quota lets the owner choose how many credits to place in the file as a grant (air-gapped stations); the default is the offering's target reserve, never more than the pool holds.
    - Releasing a station returns its last reported balance to the pool.
    - Offerings with a quota can only be one-off until D17 is decided; the offering admin refuses annual plus quota with a message naming why.
    - Every grant and return is audited; compose-backed tests cover pool arithmetic, a grant larger than the pool, release, and concurrency (two grants racing for the last credits).
- **CAP-4**
  - **intent:** A station draws its starts from a local reserve of credit grants and tops it up automatically while connected.
  - **success:**
    - The station's lease state holds verified grants and the credits spent from each; a grant id seen before is ignored; a grant for another key or an untrusted root is refused with a reason.
    - The start gate (`licence-visibility` story 5) reserves from the grant reserve when the licence's entitlement is a pool, with the same reserve, spend and return rules.
    - With a hub address configured, the station tops up over the ADR-0008 signed link when its reserve falls below the offering's threshold (default 10), to its target (default 50), reporting its balance and credits spent since the last top-up. The hub flags a station whose reports do not add up.
    - A top-up failure never refuses a start while credits remain; with none left, the refusal says to connect the station or install a licence file carrying credits.
    - Tests cover a grant in a licence file, duplicate grants, a foreign grant, top-up at threshold, an empty pool, an unreachable hub, and a restart keeping the reserve.
- **CAP-5**
  - **intent:** Annual renewals are charged 30 days before the term ends, and the next term's licences are issued as soon as the charge succeeds.
  - **success:**
    - The Stripe subscription is set up so each renewal invoice is issued and charged 30 days before the entitlement's term end, while the first purchase is charged at checkout and the first term runs 12 months from purchase. The Fake provider simulates the same timing, and the local Stripe mock covers it.
    - A paid renewal extends the entitlement by 12 months from the current term end, and issues each registered station a next-term licence valid from that end. Connected stations collect it (ADR-0008 §3); the portal offers it as a file for air-gapped stations.
    - A failed renewal charge is shown in the portal and emailed, with the date new starts will be refused. If paid before the term ends, nothing changes for stations. If paid after it ended, the new term runs 12 months from the payment date and the next renewal is charged 30 days before that end.
    - Cancelling before a renewal is charged stops future charges; a charged renewal is not refunded by the hub.
    - The checkout notice is the ADR-0013 §4 wording, verbatim, on `/buy`, the test checkout and Stripe's checkout page; `renewal_notice` and its tests change with it.
    - The README names the Stripe settings this needs (upcoming-renewal email timed to the charge, retry schedule within 30 days).
  - **checkpoint:** the Stripe mechanism for charging 30 days early (billing cycle anchor and how the first invoice is charged) must be confirmed against Stripe's documentation and the local mock before building.
- **CAP-6**
  - **intent:** A station holds its next-term licence and switches to it when it starts, with no gap.
  - **success:**
    - The station stores a next licence alongside its current one (installed by file or collected from the hub), verified the same way; installing a licence whose validity starts in the future stores it as next instead of replacing the current one.
    - At each start the gate uses whichever licence is valid at that moment; when the next licence becomes valid it becomes current, without a restart, and `GET /api/license` and the banner show both.
    - Tests cover a Job started on the last minute of the current term, a start on the first minute of the next, a next licence for another machine, and a restart in between.

## Constraints

- ADR-0013 §1's rules hold everywhere: refuse only at the start, never mid-run, never because of connectivity, no override, simulated backend never gated.
- Credits and licences are verified only against the station's configured vendor roots; the hub cannot introduce a root.
- Every hub change is audited (ADR-0007 §6). Rust serde shapes and TS mirrors change in the same commit. Customer-visible wording is the approved text verbatim.

## Non-goals

- Unused credits at renewal (D17), revocation distribution (A2.3), station inventory (A2.4).
- A separate evaluation build of the station.

## Success signal

- A prospect signs up, takes the trial, registers a bench and erases 25 drives; the 26th start is refused with a message pointing at the pricing page. A paying customer's annual renewal is charged a month ahead, every connected bench shows the next term's dates within a day, and no bench ever refuses a start on renewal day.
