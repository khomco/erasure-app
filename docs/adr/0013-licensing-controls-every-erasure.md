# Licensing controls every erasure: checked at job start, credits from a pool, renewal grace

**Status:** accepted (2026-11-09)

**Supersedes [ADR-0012](0012-opt-in-hard-stop-on-licence-lapse.md)** (the stop becomes universal,
not an opt-in). **Reverses ROADMAP §0.2 and amends [ADR-0005](0005-licensing-and-attestation.md)
§4 and §5**, stated explicitly: licensing now decides whether a new erasure may start. Resolves
DECISIONS D13. Amends D15 with a renewal grace period (§4, amended 2026-11-16).

## Context

ADR-0005 and §0.2 held that a station always erases and that licensing only changes the
certificate's marking (`evaluation`). ADR-0012 let a customer opt in to a hard stop. On
2026-11-09 the owner set a different baseline: **control first**. Nobody should be able to run a
station without limit, evaluation included, and a licence that does not cover an erasure should
stop it before it starts, clearly, rather than let it produce an uncovered certificate.

The owner's second concern is timing. A licence checked when the certificate is signed can reject
an erasure that started while the licence was valid, and a renewal that lands a few hours late can
stop a bench for no real reason. Neither should ever happen.

## Decision

### 1. The licence is checked when a job starts, and that answer is final

- **A start** is starting a Job (`start_job`) or recording drives for destruction (from a device
  or scanned). At that moment the station decides whether its licence covers the work, and
  records the decision on the Job: the licence chain, and the credit reserved (§3) if the licence
  counts credits.
- **The certificate uses the start's decision**, however long the erasure or destruction takes.
  A Job started at 23:59 on the licence's last day is certified as licensed. No erasure that
  started covered is ever rejected or marked `evaluation` because time passed.
- **A start the licence does not cover is refused** with a message naming the reason and the fix
  (install or collect a licence, top up credits, renew). The Job stays queued and can be started
  once the problem is fixed, without a restart.
- **A Job already running is never interrupted.**
- **Reasons to refuse:** no licence (§2); the licence has expired or is not yet valid; it belongs
  to another machine; the station's clock was moved back before its recorded watermark; a
  revocation the station knows about; no credit available on a licence that counts credits (§3);
  the profile's method is outside the licence's `allowed_methods` (resolving D13); or, on a
  licence that counts credits, the reservation cannot be saved to disk.
- **Never a reason:** an unreachable hub or licence server. A station holding a valid licence,
  with credits where they apply, keeps working with no network forever.
- **No override,** as ADR-0012 §7.

### 2. No licence, no erasures: evaluation is a free trial licence

- A station with no licence refuses every erasure and destruction on a real device backend. No
  separate evaluation build is needed: the ordinary binary is already controlled.
- **Evaluation is a trial licence.** A vendor admin publishes a trial offering: price zero, a
  fixed number of credits (25 at launch) and a term. A signed-in, verified organization can take
  it once. Taking it needs no payment. The station registers against it like any other
  entitlement (ADR-0007 §5, ADR-0008).
- **Air-gapped evaluations** get a trial licence file issued by a vendor admin (the existing
  vendor-issued licence, `stations::vendor_issue`), with its credits inside the file.
- **The simulated backend (`wipe-engine-mock`) is exempt.** It erases nothing, and the commerce
  demo, the tests and sales demos run on it. Its certificates stay marked `evaluation` and
  synthetic.
- Every evaluation erasure is therefore counted at the hub against a named account, and a
  reinstall resets nothing.

### 3. Credits come from an organization pool at the hub, held locally as a reserve

- **The pool.** An entitlement with an erasure quota is a pool for the whole organization, not a
  separate count in each station's licence.
- **The reserve.** Each station holds a reserve of credits as **credit grants**: documents signed
  under the vendor root, each with a unique id, bound to the station's instance key and carrying a
  number of credits. The station verifies a grant against the vendor root it already trusts
  (ADR-0008 §4) and ignores a grant id it has seen before. A start uses the local reserve, so it is
  instant and works through an outage.
- **Reserved, spent, returned.** A start reserves one credit and saves that before the Job leaves
  `Queued`. A certificate spends it. A failed or aborted Job returns it to the reserve. A Job
  that destroys several drives reserves one credit per drive.
- **Connected stations top up automatically** over the ADR-0008 signed link when the reserve
  falls below a threshold, reporting their remaining balance and the credits spent since the last
  top-up. That report is the one piece of usage reporting (ROADMAP A2.2) decided here. Defaults:
  a target reserve of 50 and a top-up below 10, both set per offering by a vendor admin.
- **Air-gapped stations** receive a grant inside their licence file. The owner chooses how many
  credits to move from the pool when the file is issued or reissued.
- **The pool's balance** is credits purchased minus credits granted, plus balances returned when a
  station is released and reports what it held.
- **Honest limit:** a station's reserve lives in its local lease state. Wiping that state loses
  credits; it never creates them. Restoring an old copy of it could re-spend a grant until a
  hardware counter exists (ADR-0005, D12). The hub flags a station whose reported spending does
  not add up.

### 4. Renewals are charged when due, and licence files carry a 30-day grace period

*Amended 2026-11-16 (owner decision). The first version of this section charged renewals 30 days
early. Stripe Checkout cannot take a full first-year payment and also move the renewal date, so
every way of doing it needed a post-checkout workaround visible to customers (a "free trial"
label, a hub-created subscription, or a reduced first charge). Charging when due with a grace
period gives the same result with nothing unusual in Stripe.*

- **Charged on the anniversary,** as Stripe subscriptions do by default (D15). Nothing about
  checkout or Stripe's renewal changes.
- **Grace in the licence file.** While an annual subscription is active and set to renew, every
  licence issued for it is valid until **30 days after the paid term ends**. The signed licence
  also records the paid-through date (an optional field, serde default, so older files still
  verify), so the bench can say when renewal is due as well as when the licence ends.
- **A successful renewal** extends the entitlement 12 months from the anniversary and makes a
  renewed licence (again with 30 days' grace) available at once. Connected stations collect it
  within a day (ADR-0008 §3). Air-gapped stations have the 30-day grace to install the file.
  The term always stays on its anniversary: benches kept working through the grace days.
- **A failed charge** leaves the 30-day grace to fix it: Stripe retries, the customer can update
  their card, and the portal and emails say so, with the date new starts will be refused. Stripe's
  retry schedule must end inside 30 days. If Stripe gives up, the subscription ends; buying again
  starts a new year from that day.
- **Cancelling** stops the next renewal, and the current term runs to its end. The hub then
  reissues the organization's licences to end at the paid-through date, without the grace, and
  connected stations collect the shorter licence. A station that is offline keeps the grace it
  already holds: the accepted exposure is at most 30 unpaid days.
- **Not a grace for anything else.** One-off purchases, trials and vendor-issued licences end
  exactly when their term ends.
- **The checkout wording** stays the owner-approved 2026-11-08 text, already live: "**Renews
  automatically every year at {price} until you cancel.** Your first year runs to {date}. Cancel
  any time from Billing in your account: your current year still runs to its end, and you will
  not be charged again. We email you before each renewal." The 2026-11-09 wording about early
  charging is withdrawn.
- **Bench warnings** (30, 7 and 1 days) count down to the licence's own end, and while the
  paid-through date has passed, the banner says the renewal payment is overdue and when new
  starts will be refused.

### 5. The lapse-policy setting is retired

Every licence now behaves as ADR-0012's `stop`. The portal's lapse-policy setting is removed. The
signed `lapse_policy` field is still read, so existing files parse, and is ignored. No customer
is affected: nothing has been sold yet.

## Consequences

- A bench can stop: on a missed renewal payment, an air-gapped site whose next licence file was
  not installed, or an empty credit reserve with no connection. The mitigations are the
  30-day renewal grace, automatic licence fetch, top-ups, warnings at 30, 7 and 1 days (ROADMAP L-R5), and refusal
  messages that say exactly how to fix it.
- "Licensing never blocks erasure" is no longer true. The marketing site, CONTEXT §9, ROADMAP
  §0.2 and L-R1, and the `bmad-build-auto` invariant change with this ADR.
- Trying the product needs an account, or a vendor-issued trial file.
- The free-tier evaluation count (`licensing-as-built` story 3) remains as a record but no longer
  matters for real backends.
- Built by `licence-visibility` story 5 (the start check, refusals, start-time decision and
  per-licence credits) and `.scratch/controlled-licensing` (trials, the pool and grants, the
  renewal grace, retiring the setting, copy).

## Not decided here

- What happens to unused credits when an annual offering that includes credits renews (DECISIONS
  D17). Until then, offerings with credits are one-off purchases only.
- Revocation distribution (ROADMAP A2.3), and the rest of usage reporting and inventory (A2.2,
  A2.4), stay under D6.
