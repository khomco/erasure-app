---
status: draft
---

# Fulfilment into entitlements, renewal and cancellation

## Owner decisions

Decided 2026-11-08, answering the halt recorded below. These are binding intent: carry them into `<intent-contract>` when planning.

- **Renewal: (B) Stripe subscriptions** (DECISIONS D15).
  - Annual offerings check out with `mode=subscription` and a yearly price. One-off offerings keep `mode=payment`.
  - Each renewal payment Stripe reports (`invoice.paid` for a renewal) extends the entitlement's term by 12 months from its current end, exactly once per invoice. These events may arrive out of order relative to the checkout events.
  - An owner cancels in Stripe's billing portal. The term runs to its end and is not renewed again.
  - A failed renewal charge is handled by Stripe's retries and dunning emails. If Stripe then cancels the subscription, that is recorded like a cancellation. Neither ever touches a licence already issued.
  - The Fake provider gets "renew now" and "cancel" actions so local tests cover the whole path.
- **Invoices** (D16, settled by D15): subscriptions carry Stripe's invoices at no add-on fee. One-off purchases keep receipts only.
- **The defaults listed below** (fulfilment creates exactly one entitlement; a refund or cancellation never revokes an issued licence; a refunded order's unused seats are withdrawn) are accepted as written.
- **Auto-renewal notice.** Customer-facing copy, shown on the offering's buy page and on the checkout confirmation before payment, verbatim, with `{price}` and `{date}` filled in:

  > **Renews automatically every year at {price} until you cancel.** Your first year runs to {date}. Cancel any time from Billing in your account: your current year still runs to its end, and you will not be charged again. We email you before each renewal.

  Approved by the owner 2026-11-08; ship it verbatim. "We email you before each renewal" depends on Stripe's upcoming-renewal reminder emails, a Stripe dashboard setting that the README's Stripe setup must name.
- **Out of scope here:** a connected station fetching its renewed licence by itself waits on D6 (ADR candidate 0008). Until then, renewed licences reach stations through `licence-delivery` story 2 (renewal reissue). The customer's opt-in hard stop on lapse is ADR-0012, built in `licence-delivery` story 4 and `licence-visibility` story 5.

## Auto Run Result

Status: blocked (resolved 2026-11-08, see `## Owner decisions`)
Blocking condition: intent gap — the renewal mechanism for annual offerings is open owner decision D15 (`.scratch/DECISIONS.md`), whose "Unblocks" column names this story's renewal design. Choosing whether customers' cards are charged automatically every year is a pricing and product decision, which AGENTS.md and the decide-by-default rule reserve for the owner. Nothing was planned in detail or implemented; the working tree is unchanged apart from this file.

### Evidence gathered

- SPEC CAP-4: "annual renewals extend the term; a refund or cancellation is recorded and stops renewal (existing licences are not silently killed — ROADMAP §0.2)". "Stops renewal" assumes something renews by itself.
- Story 3, decision 1, and `.scratch/DECISIONS.md` D15: annual offerings are charged once through Stripe (`mode=payment`) "until story 4", and story 4 chooses the recurring mechanism. D15's reason for charging once ("the hub cannot yet grant a renewed year") goes away once this story builds fulfilment, so what is left is a pure product choice.
- `CONTEXT.md` §9 describes Tier 1 as an "annual subscription per licensed wipestation", which points towards automatic renewal but does not decide the billing mechanism.
- D16 (Stripe invoices) is linked: Stripe subscriptions produce invoices with no add-on fee, while one-off payments need the paid invoicing add-on.
- The next epic, `licence-delivery` (CAP-3), expects "when an entitlement's term is extended, the portal offers renewed licence files". It works with either mechanism, as long as a term can be extended.

### Defaults this story will apply whatever you choose (object if any is wrong)

These follow ROADMAP §0.2 ("never block a wipe on licensing") and need no answer:

- Fulfilment: a paid order creates one **entitlement** for the buying organization. An entitlement is the record of what the organization may license: scope, seats, features, quota, and a term from the payment time to that time plus the offering's term. The paid event creates it exactly once, in the same transaction, and audits it.
- A refund or cancellation is recorded on the order and the entitlement, and is audited. It never revokes or shortens a licence that has already been issued, which keeps working until its own expiry. It stops the term being extended again.
- A refunded order's unused seats stop being available for new stations. A cancelled subscription keeps its seats until the current term ends.

### Question for the owner

**What it is about.** An organization owner buys an annual offering, such as "Station — annual, 1 seat", in the customer portal. They pay through Stripe Checkout, or through the Fake provider on a local hub. Today the year is charged once, and nothing happens when the year ends. This story makes a paid order grant the organization an entitlement (seats, features, a 12-month term). Licences for stations are issued from that entitlement later, in `licence-delivery`. The story must also decide how the second year is paid for.

**The problem.** Take an ITAD (IT asset disposition) business that buys one Station seat on 1 March. Its licence runs to 1 March next year. When the term ends, the station keeps erasing, but its certificates are marked `evaluation` instead of licensed to the customer. Someone has to decide whether the next year is charged automatically, so that it continues unless the customer cancels, or only when the owner comes back and buys again. The SPEC's wording "a cancellation… stops renewal" fits the first. The decision recorded in D15 kept the second "until story 4".

**The options.**

- **(A) Renew by buying again.** Annual offerings stay a single charge. Buying the same annual offering again while the organization holds an active or lapsed entitlement from it extends that entitlement by 12 months, from its current end or from the payment if it has lapsed.
  - *What customers see:* no card is ever charged without a click. The portal shows "expires on <date>, buy again to renew". There is nothing to cancel.
  - *Cost:* the smallest build. It adds the Stripe refund event (`charge.refunded`) and a Fake refund action.
  - *Rules out:* automatic renewal. Customers who forget to renew lapse to `evaluation` certificates. "Cancellation" in CAP-4 comes down to a refund. Invoices still need the paid Stripe add-on (D16).
- **(B) Stripe subscriptions.** Annual offerings check out as Stripe subscriptions (`mode=subscription`, a yearly price). Each renewal payment Stripe reports (`invoice.paid`) extends the term by 12 months. An owner can cancel in Stripe's billing portal: the term runs to its end and is not renewed. The Fake provider gets "renew now" and "cancel" test buttons.
  - *What customers see:* the year renews and is charged automatically until they cancel, and they get an invoice from Stripe for each payment.
  - *Cost:* a larger build: the subscription and invoice event family, those events arriving out of order relative to the checkout events, and the Fake simulation.
  - *Ongoing:* failed renewal charges (Stripe retries, then cancels) need handling. The buy page and checkout need an "auto-renews yearly until cancelled" notice. Auto-renewal laws in several US states and the EU require that notice, so its wording is copy you sign off.
  - *Rules out:* nothing. One-off offerings stay one charge, and a "buy again" path could still be added later.
- **(C) Split the story.** Build fulfilment and refunds now under the defaults above, with no renewal of either kind. Annual entitlements simply end after 12 months. Renewal becomes a new story once D15 is answered.
  - *What customers see:* the same as today, plus their seats in the portal.
  - *Cost:* the smallest build, and it unblocks `licence-delivery` and the SPEC's success signal (one available seat after buying) at once.
  - *Rules out:* nothing, but CAP-4's renewal and cancellation stay open, and this epic does not finish as specified.

**Recommendation:** B. `CONTEXT.md` §9 already sells Tier 1 as an annual subscription. Automatic renewal is the B2B (business-to-business) norm. It also settles D16, because subscription invoices carry no add-on fee. The reason for charging once in D15, that the hub could not grant a renewed year, is what this story removes. If you are not ready to sign off auto-renewal copy, choose C, which unblocks the next epic without committing to either.

**Default if nobody decides:** story 4 stays blocked, and `licence-delivery`, which needs entitlements, waits behind it. Story 5 (the public offerings API and pricing page) does not depend on story 4 and can run.

Should annual offerings renew by buying again (A), as Stripe subscriptions that renew automatically until cancelled (B), or should this story ship fulfilment and refunds only and leave renewal to a later story (C)?
