# Licensing controls every erasure: checked at job start, credits from a pool, renewals charged early

**Status:** accepted (2026-11-09)

**Supersedes [ADR-0012](0012-opt-in-hard-stop-on-licence-lapse.md)** (the stop becomes universal,
not an opt-in). **Reverses ROADMAP §0.2 and amends [ADR-0005](0005-licensing-and-attestation.md)
§4 and §5**, stated explicitly: licensing now decides whether a new erasure may start. Resolves
DECISIONS D13. Amends D15's renewal timing and the approved auto-renewal wording.

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

### 4. Renewals are charged 30 days early, with no gap and no grace period

- **Charged early.** An annual subscription's renewal is charged 30 days before the current term
  ends. The new term starts the moment the old one ends, so the customer still gets 12 months
  for 12 months' price.
- **Delivered early.** When the charge succeeds, the hub issues each registered station its
  next-term licence, valid from the old licence's end. Connected stations collect it at once
  (ADR-0008 §3). A station holds its current and its next licence and switches when the next one
  starts. A Job's start decides by the licence valid at that moment (§1).
- **A failed charge** leaves 30 days to fix it: Stripe retries, the customer can update their
  card, and the portal and emails say so. If it is paid before the current term ends, nothing
  changes for the stations. If it is still unpaid at the end, the current licence expires and new
  starts are refused (§1).
- **Paid late**, after the term ended, the new term starts **on the day of payment** and runs 12
  months, so a customer never pays for days their benches could not run. The following renewal is
  charged 30 days before that new end.
- **No grace period** is added to licence files. Early charging is what prevents the gap.
- **Cancelling** before a renewal is charged stops future charges, and the current term runs to
  its end. **A renewal already charged is not refunded** (owner decision, 2026-11-09), because
  its licence may already sit on stations that cannot be reached. Exceptions go through support.
- **The checkout wording** (owner-approved 2026-11-09, replacing the 2026-11-08 text): "**Renews
  automatically every year at {price} until you cancel.** Your first year runs to {date}. We
  charge each renewal 30 days before your year ends, so your stations receive the new licence in
  good time, and we email you before we charge. Cancel from Billing in your account before a
  renewal is charged: your current year still runs to its end, and you will not be charged again.
  A renewal already charged is not refunded."
- The reminder email goes before the charge, not before the anniversary (Stripe's
  upcoming-renewal email, timed to the charge).

### 5. The lapse-policy setting is retired

Every licence now behaves as ADR-0012's `stop`. The portal's lapse-policy setting is removed. The
signed `lapse_policy` field is still read, so existing files parse, and is ignored. No customer
is affected: nothing has been sold yet.

## Consequences

- A bench can stop: on a missed renewal payment, an air-gapped site whose next licence file was
  not installed, or an empty credit reserve with no connection. The mitigations are early
  charging, early delivery, top-ups, warnings at 30, 7 and 1 days (ROADMAP L-R5), and refusal
  messages that say exactly how to fix it.
- "Licensing never blocks erasure" is no longer true. The marketing site, CONTEXT §9, ROADMAP
  §0.2 and L-R1, and the `bmad-build-auto` invariant change with this ADR.
- Trying the product needs an account, or a vendor-issued trial file.
- The free-tier evaluation count (`licensing-as-built` story 3) remains as a record but no longer
  matters for real backends.
- Built by `licence-visibility` story 5 (the start check, refusals, start-time decision and
  per-licence credits) and `.scratch/controlled-licensing` (trials, the pool and grants, early
  renewal, retiring the setting, copy).

## Not decided here

- What happens to unused credits when an annual offering that includes credits renews (DECISIONS
  D17). Until then, offerings with credits are one-off purchases only.
- Revocation distribution (ROADMAP A2.3), and the rest of usage reporting and inventory (A2.2,
  A2.4), stay under D6.
