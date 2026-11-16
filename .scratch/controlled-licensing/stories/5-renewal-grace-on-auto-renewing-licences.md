---
title: 'Renewal grace on auto-renewing licences'
type: 'feature'
created: '2026-11-16'
status: 'done'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/AGENTS.md'
  - '{project-root}/docs/adr/0013-licensing-controls-every-erasure.md'
  - '{project-root}/docs/adr/0008-station-enrollment-and-licence-fetch.md'
  - '{project-root}/.scratch/controlled-licensing/stories/4-station-credit-reserve-and-automatic-top-up.md'
warnings: ['oversized']
deferred:
  - summary: >-
      A refunded annual subscription's station licences keep their 30-day renewal grace.
    evidence: |-
      LICENCE_PAST_END excludes refunded entitlements by design (refunds never touch licences, a pre-existing policy); the licence already ran to term end before this story, so the grace adds at most 30 days.
    location: >-
      crates/wipe-hub/src/stations.rs (LICENCE_PAST_END)
    severity: low
baseline_revision: 'c62852d419ce09879ab4d167438e8001521b9ab9'
---

<intent-contract>

## Intent

**Problem:** A licence for an annual subscription ends exactly at the paid term's end. A renewal charged on the anniversary that is late, or that fails and is retried, stops every bench that day (ADR-0013 §4, amended 2026-11-16). A cancelled subscription has no way to pull back a licence it has already issued, and nobody is told that a renewal charge failed.

**Approach:** While an entitlement is active and set to renew, the hub signs its station licences to the term end + 30 days and records the term end in a new signed, optional `paid_through` date. Cancelling, or Stripe ending the subscription, reissues the organization's active licences on that entitlement to end at `paid_through`, without grace; connected stations install the shorter licence through the ADR-0008 fetch. A failed renewal charge (`invoice.payment_failed`) is recorded on the entitlement, shown in the portal and emailed to owners. The bench banner says when payment is overdue.

## Boundaries & Constraints

**Always:**
- **Signed field** (`wipe-license/src/entitlement.rs` `Entitlements`, beside `not_after`): `paid_through: Option<OffsetDateTime>`, `#[serde(default, skip_serializing_if = "Option::is_none", with = "time::serde::rfc3339::option")]`. Every existing literal gains `paid_through: None` (42 sites, grep `Entitlements {` in `crates/`, tests included). The `v2-*.json` cert fixtures, the credit-grant fixture and every existing licence file still verify; add a test that a licence signed with no `paid_through` re-verifies after a parse/serialize round trip, and one that a tampered `paid_through` is `Tampered`. Portal `api/adminLicences.ts` mirror gains `paid_through?: string` (guard accepts absent or string).
- **The grace rule lives once in the hub** (`entitlements.rs`): `RENEWAL_GRACE: time::Duration = 30 days`, plus a SQL fragment `LICENCE_END_SQL` = `e.term_end + CASE WHEN e.status = 'active' AND e.renews THEN interval '720 hours' ELSE interval '0 hours' END` (hours, never `'30 days'`, so the session time zone cannot shift it). The entitlement is "in grace" exactly when `status = 'active' AND renews`: only an annual subscription renews, so one-off, trial and refunded entitlements never are.
- **Issue** (`stations.rs` `issue_licence`, so register, reissue, enrollment approval and the station fetch all follow): in grace → `not_after = term_end + RENEWAL_GRACE`, `paid_through = Some(term_end)`; otherwise `not_after = term_end`, `paid_through = None`. `LockedEntitlement` gains `status` and `renews`. `vendor_issue` is unchanged (no grace, no `paid_through`). `licence.issued` audit detail gains `paid_through` (null when none).
- **Out of step with the term:** `LICENCE_BEHIND_TERM` becomes `l.status = 'active' AND l.not_after < LICENCE_END_SQL` (so a renewal, a resumed renewal, or a licence issued before this story on a renewing subscription shows `renewal_available` and is reissued to the grace end). New `LICENCE_PAST_END` = `l.status = 'active' AND l.not_after > LICENCE_END_SQL AND e.status <> 'refunded'`: a licence carrying grace that is no longer due.
- **Withdraw grace** (new `stations::withdraw_grace(conn, root, station_row, actor)`): for an unreleased station whose latest licence is `LICENCE_PAST_END`, with the station and its entitlement locked `FOR UPDATE` (station → entitlement order), supersede the licence (`licence.superseded {instance_key_id, reason: "grace_withdrawn", superseded_by}`) and sign a new one through `sign_and_store` for the same key, scope, quota and features with `not_after = term_end`, `paid_through = None`, no credit grant (`licence.issued {reissue_of, paid_through: null, …}`). It is the one place allowed to sign a `not_after` at or before now (a subscription Stripe ends after failed retries ends past its term); every other issue path keeps the `not_after <= now` refusal. Seat and registration kept. A station whose latest licence is not past its end is a no-op.
- **When grace is withdrawn:** (a) in the same transaction as an applied `RenewalChanged { renews: false }` or `SubscriptionEnded` event: `payment_events::receive` gains `root: Option<&VendorRoot>` (webhook and Fake routes pass the configured root) and, after `apply_event` returns `Applied`, calls `stations::withdraw_grace_for_subscription(conn, root, provider, subscription_id, actor "provider:<name>")` for every unreleased station on that entitlement. No root → `tracing::warn` and skip; the event still applies. (b) As a self-heal, the station fetch (`station_link::fetch_licence`) checks `LICENCE_PAST_END` after `renewal_available` and, with a root, withdraws grace (actor `station:<id>`) and answers the new licence. Refund events touch no licence (unchanged).
- **Station install of a shorter licence** (`wipe-server/src/licence_fetch.rs` `check`): today only a licence ending strictly later is installed. Also install a fetched licence ending earlier when the installed licence has `paid_through` set, the fetched one was issued strictly later (`issued_at`), and both have the same `customer_id`. Message: "Installed a licence from the platform at {o} that ends sooner, on {date} UTC: the subscription it was issued for will not renew." (then the existing saved-file sentence). A vendor-issued or file-installed licence with no `paid_through` is never shortened by a fetch.
- **Failed renewal** (new `EventKind::RenewalFailed { subscription_id, invoice_id }`): Stripe `invoice.payment_failed` with `billing_reason=subscription_cycle`, subscription and order read exactly as `invoice.paid` reads them; Fake event type `subscription.renewal_failed` (subscription + invoice). It is a subscription event in `waits_for_its_order`. `entitlements::renewal_failed`: none → `Unmatched`; not `active` or not `renews` → `Unchanged`; `renewal_failed_at` already set → `Unchanged` (not audited); else set `renewal_failed_at = coalesce(event time, now())`, audit `entitlement.renewal_failed {event_id, invoice_id}`, `Applied`. `renew` clears `renewal_failed_at` (in its existing UPDATE; its audit detail unchanged). Migration `0024_renewal_grace.sql`: `entitlements.renewal_failed_at timestamptz NULL`.
- **Portal entitlement** (`entitlements::Entitlement` + `api/orders.ts` mirror and guard): `renewal_failed_at: Option<String>` and `grace_ends: Option<String>` (`term_end + 30 days` RFC 3339 while in grace, else null). `SeatsPage` `entitlementState`: when `renews`, `status === "active"` and `renewal_failed_at` is set: "Renewal payment failed on {day(renewal_failed_at)}. Update your card from Billing: new jobs will be refused from {day(grace_ends)} unless it is paid."
- **Email:** after the webhook's (or Fake route's) transaction commits an `Applied` `RenewalFailed`, email every verified owner of the organization, best effort (a failed send is logged at warn, never undoes the event; outside the per-address auth mail limits, which guard user-triggered sends). `Mailer::send_renewal_failed(to, offering_name, grace_ends_day)`: subject `Your Wipestation renewal payment failed`; body says the renewal payment for {offering} failed, that Stripe will retry, that the stations keep working until {date}, that new jobs will be refused from {date} unless it is paid, and to update the card from Billing in the portal at `{public_base_url}/seats`. Sent once per failed renewal (the first failure; retries are `Unchanged`).
- **Bench banner** (`wipe-server/src/licensing.rs` `LicenseView`): `paid_through: Option<OffsetDateTime>` (from the installed licence, always shown) and `renewal_overdue: Option<String>`: on a real backend, when `paid_through < now <= not_after`, exactly "Renewal payment overdue since {paid_through date}. New jobs will be refused from {not_after date} unless it is paid." (dates as `OffsetDateTime::date()` prints them); `None` otherwise and always on a simulated backend. Desktop `types.ts` `LicenseInfo` mirrors both (`paid_through: string | null`, `renewal_overdue?: string | null`); `api/licence.ts` `licenceBanner`: after `new_jobs_refused` and the non-valid branches, a valid licence with `renewal_overdue` shows heading "Renewal payment overdue", that line plus `expiryConsequence`, tone `danger` at ≤ `EXPIRY_FINAL_DAYS` else `warning`. Expiry warnings keep counting down to `not_after` (unchanged).
- Paired changes in one commit: Rust serde shapes and TS mirrors; `README.md` (Stripe section: nine events with `invoice.payment_failed`; under Failed payments, the retry schedule must end within 30 days of the failed charge, because licences carry 30 days' grace; hub paragraph: grace, withdrawal, failed renewal notice; the "none of these touches an issued licence" sentence corrected), `CONTEXT.md` (§9, §10 shipped row, §14 **Renewal grace**, **Paid-through date**), `docs/ROADMAP.md` (strike or update the renewal-grace item if listed), `entitlements.rs`/`stations.rs`/`station_link.rs`/`licence_fetch.rs` module docs (the "renewals arrive 30 days early" line in `licence_fetch.rs` is now wrong), `AGENTS.md` test counts.

**Never:**
- Charge early, change checkout, or touch the checkout notice text (`renewal_notice`); change Stripe subscription creation.
- Give grace to one-off, trial, vendor-issued or refunded licences; revoke or shorten licences on refund.
- Refuse or interrupt a start because of payment state or the hub: the station decides only from its installed licence (ADR-0013 §1). No override.
- Edit an applied migration, the `v2-*.json` fixtures, or the credit-grant fixture; change the station routes (`docs/generated/http-routes.txt`) or CLI help.
- Unused credits at renewal (D17), revocation distribution (A2.3).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Grace issued | Annual Fake purchase, renewing; register station | Licence `not_after` = term_end + 30 d, `paid_through` = term_end | — |
| No grace | One-off, trial, vendor-issued, cancelled-at-period-end | `not_after` = term_end (or vendor days), no `paid_through` | — |
| Renewal | Fake renew paid | term_end + 12 months from the old end (anniversary); `renewal_available`; reissue → new term_end + 30 d | — |
| Cancel | Fake cancel on renewing entitlement with 2 stations | Both licences superseded `grace_withdrawn`, new ones end at term_end with no `paid_through`; audited; same transaction as `entitlement.renewal_stopped` | No root: event applies, warn, no reissue |
| Stripe ends after retries | `SubscriptionEnded` after term_end passed | Licences reissued to end at term_end (already past) | — |
| Fetch after cancel | Station holding the grace licence fetches | Gets the trimmed licence; installs it (ends sooner, issued later, installed had `paid_through`) | — |
| Self-heal | Cancel applied while hub had no root; later fetch with root | Fetch withdraws grace and answers trimmed licence | — |
| Offline at cancel | Station never fetches | Keeps its grace licence; starts allowed until term_end + 30 d | — |
| Resume | Owner resumes after cancel | `renewal_available` true; reissue → grace again | — |
| Old licence | Licence on renewing entitlement ending at term_end (pre-story) | `renewal_available` true; fetch reissues with grace | — |
| Renewal fails | Fake `subscription.renewal_failed` | `renewal_failed_at` set, audited, owners emailed once; portal line; second failure `Unchanged`, no mail | Mail failure: warn only |
| Renewal paid after failure | Fake renew | `renewal_failed_at` cleared | — |
| Overdue banner | Real backend, licence with `paid_through` past, `not_after` future | `renewal_overdue` exact text; desktop banner "Renewal payment overdue" | Simulated: `None` |
| Vendor file on connected station | Installed licence without `paid_through`; hub offers an earlier-ending one | Not installed ("Up to date …") | — |
| Old file | Licence JSON with no `paid_through` | Verifies; `paid_through` null | — |

## Autonomous decisions

- **Where the paid-through date is signed.** *About:* the licence file a bench installs. *Problem:* the spec says "licence body"; it could sit on the outer body or inside the entitlements beside `not_after`. *Options:* (A) `Entitlements.paid_through`; (B) `LicenseBody.paid_through` with a new issue function. *Choice:* A. It is part of the licence's validity window, it is signed either way, and the hub already builds `Entitlements`. Cost: 42 mechanical `paid_through: None` edits.
- **Licences without grace omit `paid_through`** (the spec allows omitting it or setting it to `not_after`): omitting keeps every such licence byte-identical to today, and gives the station one signal, "this licence carries a renewal grace", which the shorter-licence install rule relies on.
- **How cancelled grace reaches stations.** *Problem:* the payment path must never fail for want of a vendor root, yet stations must collect the shorter licence. *Choice:* withdraw grace in the same transaction as the cancel/end event when a root is configured (atomic and audited together), skip with a warning when not, and let the station fetch repair any licence still past its end. Refunds stay untouched, since the spec names only cancel and end.
- **When a station accepts a shorter licence.** *Problem:* the station installs only licences ending later (ADR-0008 §3), so a trimmed licence would be ignored. A rule that is too broad would let a fetch shorten a vendor licence an operator installed by hand. *Choice:* install an earlier-ending licence only when the installed one carries `paid_through` (it was issued with grace), the new one was issued later, and both name the same organization. That is the safe default: only grace is ever withdrawn.
- **Withdrawal may sign an already-ended licence.** When Stripe ends the subscription after failed retries, the term has already ended. Signing a licence that ends at the paid-through date (in the past) is the only way for connected benches to stop accepting new starts before the grace runs out, as ADR-0013 §4 intends. Only `withdraw_grace` may do this.
- **Exact 30 days.** Postgres `interval '30 days'` depends on the session time zone across daylight-saving changes. The SQL uses `720 hours` to match Rust's `Duration::days(30)`, so the stored `not_after` equals the signed one.
- **Failed-renewal notice: once per failure, owners only.** The first failure since the last paid renewal sets `renewal_failed_at`, is audited and emails verified owners. Stripe retries do not repeat the email (Stripe sends its own retry emails). Owners hold billing (as `MSG_OWNER_ONLY_BILLING`). The email is best effort after commit, so a mail outage never makes Stripe redeliver the event.
- **Overdue line on a simulated bench.** "New jobs will be refused" would be false there (ADR-0013 §2), so `renewal_overdue` is omitted on a simulated backend, as `new_jobs_refused` is. The expiry countdown, which has a simulated wording, still shows.
- **Wording.** The bench banner text is the spec's verbatim. The portal line, the email and the station's "ends sooner" status are functional transactional text in the house style of stories 3 and 4, not marketing copy. The owner may revise them; they are listed in the Auto Run Result for review.
- **The database window check** *(implementation, 2026-11-16)*. Migration 0014's `licences_window_check` (`not_after > issued_at`) would refuse the licence `withdraw_grace` signs after a subscription ends past its term. Migration 0024 relaxes it for station licences only (`not_after > issued_at OR source = 'station'`); vendor licences keep it, and every other station issue path still refuses an already-ended licence in Rust. Back-dating the signed `issued_at` instead was rejected: it would put a false time in a signed document.
- **Lock order in the payment path** *(implementation, 2026-11-16)*. `apply_event` already holds the entitlement row lock when the grace is withdrawn, so locking stations then would invert the station → entitlement order and could deadlock with a concurrent fetch, top-up, reissue or release. `withdraw_grace_for_subscription` locks the subscription's station rows with `FOR UPDATE SKIP LOCKED`: a station busy at that instant is skipped and withdraws its own grace at its next fetch (the self-heal). The common case, tested, withdraws every station in the event's transaction.
- **Failed-renewal email link.** `Mailer::send_renewal_failed` also takes the Seats page URL (built from `--public-base-url`), since the body names it.
- **Fake provider.** Tests and the local demo need failed and ended subscriptions. The Fake provider gains event types `subscription.renewal_failed` and `subscription.ended`, posted to the webhook by tests. No new hub route is added.

</intent-contract>

## Code Map

**wipe-license**
- `src/entitlement.rs:129` `Entitlements` (+`paid_through`); `src/chain.rs:77` `LicenseBody` (unchanged; canonical re-verify relies on `skip_serializing_if`). Tests: `tests/attestation.rs`, `tests/end_to_end.rs` patterns.

**wipe-hub**
- `src/entitlements.rs`: module doc (":25 Nothing here touches a licence" stays true for this module, since withdrawal lives in `stations`), `Entitlement` :68 / `EntitlementRow` :105 / `select_entitlements` :166 (+`renewal_failed_at`, `grace_ends`), `renew` :368 (clear failure), `change_renewal` :483, `end` :567, new `renewal_failed`, `CURRENT_SQL`, add `RENEWAL_GRACE`, `LICENCE_END_SQL`.
- `src/payments.rs:123` `EventKind` (+`RenewalFailed`), `VerifiedEvent::new` required-field checks :212, Fake verify :545 (+2 types; constants beside `FAKE_SUBSCRIPTION_CANCELLED`). `src/stripe.rs:77` constants (+`INVOICE_PAYMENT_FAILED`), `parse_event` :586 (share the `invoice.paid` subscription/order extraction).
- `src/orders.rs:620` `apply_event` (+arm). `src/payment_events.rs:50` `receive` (+root, withdrawal after Applied), `waits_for_its_order` :97 (+`RenewalFailed`).
- `src/order_routes.rs:318` `webhook` (pass root; post-commit email), `:393` `receive_fake`, `:486` `fake_subscription_event`. Root: `state.config.vendor_root` (`config.rs:213`, check how `station_routes.rs` obtains a `VendorRoot` from it).
- `src/stations.rs`: module doc; `LICENCE_BEHIND_TERM` :290, `LockedEntitlement` :470 / `locked_entitlement_columns` :487 (+status, renews), `licence_entitlements` :510 (+paid_through), `sign_and_store` :597 (audit detail), `issue_licence` :692 (grace), `reissue` :935 (template for `withdraw_grace`), `vendor_issue` :1195 (`paid_through: None`).
- `src/station_link.rs:258` `fetch_licence` (+past-end branch; `LatestState` +`past_end`).
- `src/mail.rs` (+`RENEWAL_FAILED_SUBJECT`, `send_renewal_failed`); owners: `users WHERE organization_id = $1 AND role = 'owner'` with verified email (see `accounts.rs`/`team.rs` for the verified column).
- `migrations/0024_renewal_grace.sql` (new). Tests: `tests/support/hub.rs` (`spawn_hub_with_root`, `published_offering` (annual), `published_one_off_offering`, `published_trial_offering`, `audited`, `mail_texts`), `tests/stations.rs` fixture (`renew`, `term_end`), `tests/entitlements.rs` (`action`), `tests/station_licence.rs` (signed fetch helpers), `tests/stripe.rs` (signed Stripe bodies, `tests/fixtures/*invoice.paid*`), `tests/migrations.rs`.

**wipe-server**
- `src/licence_fetch.rs:193` install rule; module doc :1–30. `src/licensing.rs:392` `view`, `:902` `LicenseView` (+2 fields). Tests: `tests/licence_fetch.rs` (`fake_hub`, `licence(root, key)`), `tests/http_e2e.rs` (real-backend wrapper) for `GET /api/license`.

**Frontends**
- `apps/desktop/src/api/types.ts:358` `LicenseInfo`; `src/api/licence.ts` `licenceBanner` (+tests in `licence.test.ts`).
- `apps/portal/src/api/orders.ts:96` `Entitlement` + guard; `pages/SeatsPage.tsx:15` `entitlementState` (+`SeatsPage.test.ts`); `api/adminLicences.ts` entitlements mirror + guard (+test).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-license/src/entitlement.rs` + every `Entitlements {` literal + new tests -- `paid_through`, old files verify, tamper detected.
- `crates/wipe-hub/migrations/0024_renewal_grace.sql`, `src/{entitlements,payments,stripe,orders,payment_events,order_routes,stations,station_link,mail}.rs` -- grace issue, out-of-step SQL, withdrawal on cancel/end and on fetch, failed-renewal event, portal fields, email.
- `crates/wipe-hub/tests/renewal_grace.rs` (new) + updates to `stations.rs`, `entitlements.rs`, `stripe.rs`, `migrations.rs`, `station_licence.rs`, `isolation.rs` if needed -- matrix rows: grace on active subscription; none on one-off, trial, vendor; renewal keeps the anniversary; cancel trims connected stations (two stations, audit rows, same transaction); Stripe end past term; no-root cancel then self-heal fetch; resume; pre-story licence reissued; failure recorded, audited, emailed once (Mailpit), cleared by renewal; Stripe `invoice.payment_failed` parse.
- `crates/wipe-server/src/{licence_fetch,licensing}.rs` + `tests/licence_fetch.rs`, `tests/http_e2e.rs` -- shorter-licence install rule (installed with grace: installed; vendor licence without `paid_through`: not), overdue view on real and simulated backends.
- `apps/desktop/src/api/{types,licence}.ts` (+tests) -- mirror and banner.
- `apps/portal/src/api/{orders,adminLicences}.ts`, `pages/SeatsPage.tsx` (+tests) -- mirrors, failure line.
- `README.md`, `CONTEXT.md`, `docs/ROADMAP.md`, `AGENTS.md` -- what shipped, Stripe settings.

**Acceptance Criteria:**
- Given a verified owner who bought an annual offering through the Fake provider and registered a station, when the station's licence is downloaded, then it verifies against the hub root with `not_after` = term_end + 30 days and `paid_through` = term_end, and a one-off purchase's licence has `not_after` = term_end and no `paid_through` (hub test over HTTP).
- Given that annual entitlement with two registered stations, when the owner cancels (Fake cancel), then each station's next signed `POST /api/station/licence` answers a licence ending at term_end with no `paid_through`, and the audit log holds `entitlement.renewal_stopped`, two `licence.superseded {reason: "grace_withdrawn"}` and two `licence.issued` rows.
- Given a real-backend station with a saved hub address holding a licence with `paid_through`, when its fetch receives a later-issued licence for the same organization ending sooner, then `GET /api/license` shows the new `not_after` and `paid_through: null`.
- Given a real-backend station whose installed licence's `paid_through` has passed and `not_after` has not, when `GET /api/license` is read, then `renewal_overdue` is the exact overdue sentence and the desktop banner heading is "Renewal payment overdue".
- Given `cargo test --workspace`, the desktop and portal suites, both route/help goldens, when run, then all pass.

## Spec Change Log

## Review Triage Log

### 2026-11-16 — Review pass
- verdicts: 33 findings — high 0, medium 4, low 20, false 9, maybe-false 0
- findings:
  - `[medium]` `[patch]` Blind: a late `invoice.payment_failed` after the same invoice's `invoice.paid` marks a paid renewal failed and emails owners — `renewal_failed` now returns `Unchanged` when the invoice has an `entitlement_renewals` row; test `a_failure_delivered_after_its_invoice_was_paid_changes_nothing`.
  - `[low]` `[reject]` Blind: the bench says "Renewal payment overdue" for up to a day after an on-time renewal — ADR-0013 §4 prescribes the banner once `paid_through` passes and the text is the spec's verbatim; the lag is bounded by the 24 h ADR-0008 check and refuses nothing. Recorded as a residual risk.
  - `[low]` `[reject]` Blind: the portal shows "Ended on" between the anniversary and `invoice.paid`, and the failure line keeps "will be refused from" after that date — a window of hours, or only with a retry schedule longer than the 30 days the README requires; a fix adds new branches and new copy.
  - `[false]` `[reject]` Blind: no bench can be registered during grace and nothing documents it — README (hub notes) states that a renewing entitlement past its term is not current and no station can be registered or reissued on it; the restriction is the existing `ENTITLEMENT_CURRENT` rule.
  - `[medium]` `[patch]` Blind: one station whose withdrawal fails (non-DB error) fails the whole cancel/end webhook for ever — each station is withdrawn in a savepoint; non-DB/audit errors are logged and the station skipped (its fetch self-heals).
  - `[low]` `[reject]` Blind: migration 0024 relaxes the window check for every station licence — the Rust refusal still guards every other issue path; a bounded check would turn a long-delayed Stripe end into a failed withdrawal.
  - `[false]` `[reject]` Blind: no rollout plan for pre-story licences (renewal_available burst, air-gapped files without grace) — nothing has been sold (ADR-0013 §5), so no customer holds a pre-story licence.
  - `[low]` `[patch]` Blind: the failure email names the same day as both "working until" and "refused from" — reworded to state the date once.
  - `[low]` `[reject]` Blind: the station's "ends sooner … will not renew" message on an already-ended trimmed licence — only after Stripe ends a subscription past its term; the sentence stays true.
  - `[low]` `[reject]` Blind: the SKIP LOCKED skip path is untested — needs a concurrency harness; the self-heal fetch it relies on is tested.
  - `[low]` `[patch]` Blind: `paid_through` is never shown on the bench before it passes (ADR-0013 §4) — LicencePage shows "Paid through {date} UTC"; test added.
  - `[low]` `[reject]` Blind: a crash between commit and SMTP send loses the failure email — rare window; the portal shows the failure and Stripe sends its own failed-payment emails; an outbox is new machinery.
  - `[low]` `[patch]` Blind: desktop mirror makes `renewal_overdue` optional though always serialized — now `string | null`, fixtures updated.
  - `[medium]` `[patch]` Edge: late `invoice.payment_failed` after `invoice.paid` (same as the first Blind row) — same fix.
  - `[low]` `[reject]` Edge: a failure after term_end + 30 days emails a past "working until" date — only with a retry schedule the README forbids.
  - `[medium]` `[patch]` Edge: a persistent `withdraw_grace` Issue error makes the webhook 500 for ever (same root cause as the Blind row) — same savepoint fix.
  - `[low]` `[defer]` Edge: a refunded annual subscription's licences keep their 30-day grace — refunds have never touched licences (pre-existing policy; the licence already runs to term end); deferred.
  - `[low]` `[patch]` Edge: no verified owner means a silent non-notice — a warn naming the organization is logged.
  - `[low]` `[reject]` Edge: the portal failure line uses future tense after the date passed — same as the Blind portal row.
  - `[low]` `[reject]` Edge: a fetch racing a cancellation keeps its grace until the next fetch — the recorded lock-order decision; bounded by the 24 h check.
  - `[low]` `[patch]` Verification gap: a refund never withdrawing grace is untested — test `a_refund_never_withdraws_the_grace`.
  - `[low]` `[patch]` Verification gap: only one verified owner tested for the email — second verified owner (one mail) and unverified owner (none) added.
  - `[low]` `[patch]` Verification gap: the failure date from the event time is untested — Stripe test sends `created` three days back and checks `renewal_failed_at`.
  - `[false]` `[reject]` Intent: trims only the cancelled subscription's stations, not "the organization's" — other entitlements' licences carry their own (or no) grace; trimming them would be wrong, and every licence on the cancelled subscription is trimmed.
  - `[low]` `[reject]` Intent: no test runs a real hub's trimmed licence through a real station — crate boundaries forbid it (story 4 recorded the same); both halves are tested on the shared signed document.
  - `[false]` `[reject]` Intent: the offline-at-cancel test never touches a station — an offline station does nothing; its licence file is unchanged by construction, which the test checks.
  - `[false]` `[reject]` Intent: the overdue banner is never fed a hub-issued licence — `http_e2e` uses a signed licence with `paid_through`, the same shape the hub signs; the hub test pins that shape.
  - `[false]` `[reject]` Intent: withdrawal is tested only with Fake events — withdrawal keys on `EventKind`, which is provider-agnostic; Stripe's mapping to those kinds is pre-existing and tested.
  - `[false]` `[reject]` Intent: registration is impossible during grace (whole-system success signal) — same as the Blind row: documented, and existing benches keep working, which is the signal's claim.
  - `[low]` `[reject]` Intent: the portal line, email and station message are not owner-approved text — functional transactional text as in stories 3–4; listed for owner review in the Auto Run Result.
  - `[false]` `[reject]` Intent: R1/R3/R4/R5 readings — the diff implements the spec's chosen reading; descriptive, no defect.
  - `[false]` `[reject]` Intent: the grace rule reads hub state, not Stripe's live state — the hub mirrors Stripe through webhooks by design (ADR-0007).
  - `[false]` `[reject]` Intent: failure notice once per episode vs every retry — the spec chose once; descriptive.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass (not piped through grep).
- `cargo clippy --workspace --all-targets` -- expected: no new warnings.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass.
- `pnpm --filter portal typecheck && pnpm --filter portal test` -- expected: pass.
- `rustfmt --edition 2021 <touched .rs files>` -- touched files only.

## Auto Run Result

Status: done

**Summary.** Licences for an active, renewing annual subscription now run 30 days past the paid term, and record the term's end as a signed, optional `paid_through`. One-off, trial and vendor-issued licences get no grace. Cancelling, or Stripe ending the subscription, reissues that subscription's station licences without grace in the same transaction. Connected stations install the shorter licence. A failed renewal charge is recorded once, shown on `/seats` and emailed to verified owners. The bench shows the paid-through date and, once it has passed, the overdue banner.

**Files changed**
- `crates/wipe-license/src/entitlement.rs` (+ every `Entitlements {` literal): the optional signed `paid_through`.
- `crates/wipe-hub/migrations/0024_renewal_grace.sql`: `renewal_failed_at`; the window check relaxed for station licences.
- `crates/wipe-hub/src/entitlements.rs`: the grace rule (`RENEWAL_GRACE`, `LICENCE_END_SQL`), `renewal_failed`, and the portal fields `renewal_failed_at` / `grace_ends`.
- `crates/wipe-hub/src/stations.rs`: grace at issue, `withdraw_grace` and `withdraw_grace_for_subscription` (one savepoint per station).
- `crates/wipe-hub/src/{payments,stripe,orders,payment_events,order_routes,station_link,mail}.rs`: the `RenewalFailed` event, Fake event types, withdrawal on cancel/end, the fetch self-heal, and the owner email.
- `crates/wipe-server/src/{licence_fetch,licensing}.rs`: the shorter-licence install rule; `paid_through` and `renewal_overdue` on `GET /api/license`.
- `apps/desktop` (types, licence banner, LicencePage) and `apps/portal` (orders and adminLicences mirrors, SeatsPage), with tests.
- `crates/wipe-hub/tests/renewal_grace.rs` (new, 12 tests) and updates to the stripe, stations, licence_admin, migrations and station_licence tests; station tests in `licence_fetch.rs` and `http_e2e.rs`.
- `README.md`, `CONTEXT.md`, `docs/ROADMAP.md` (L-R5), `AGENTS.md` (test counts).

**Review.** 33 findings: 4 medium, 20 low, 9 false. Patched: 4 medium and 9 low rows, which are 2 medium and 9 low root causes. Deferred: 1 (refunds keep their grace, low). Rejected: the rest, each with its reason in the triage log.

**Follow-up review recommended: true.** Patched: 2 medium root causes (late failure after payment; one bad station failing the cancel webhook), 9 low. The named unverified risk: the per-station savepoint skip path (a non-database withdrawal error) and the `SKIP LOCKED` skip path have no test that forces them.

**Verification.**
- `cargo test --workspace --no-fail-fast`: 1232 passed, 0 failed. An earlier run failed once on the environmental `health::silent_smtp_and_database_time_out_within_bound` timing test, which story 4 also recorded.
- Clippy: only the 4 existing `large_enum_variant` warnings; the new `explicit_auto_deref` was fixed.
- Route and help goldens: unchanged and passing.
- Desktop: typecheck and 430 tests pass. Portal: typecheck and 298 tests pass.

**Residual risks and owner review.**
- The step-03 subagent appended three implementation decisions to the intent contract (database window check, lock order, email link). They are additive records and change no rules.
- New text the owner may want to revise:
  - the `/seats` failure line;
  - the failure email (subject "Your Wipestation renewal payment failed");
  - the station's "ends sooner" check message;
  - the "Paid through {date} UTC" line on the bench.
- A bench can say "Renewal payment overdue" for up to a day after an on-time renewal, until its next licence check, as ADR-0013 §4's rule implies.
- No new station can be registered on a renewing entitlement during its grace days. This is documented in the README.
