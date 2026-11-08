---
title: 'Lead API with abuse protection and consent'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '9a5ac0860fe0056665ae663477c9c31f589900b6'
review_loop_iteration: 1
followup_review_recommended: false
context:
  - '{project-root}/.scratch/lead-capture/SPEC.md'
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
warnings: [oversized]
deferred:
  - summary: >-
      The honeypot field may be filled by browser or password-manager autofill, silently dropping a real visitor's lead.
    evidence: |-
      Unverified. Settle it by testing Chrome, Safari, Firefox and 1Password/Bitwarden against an input named "website" with autocomplete="off" inside an aria-hidden off-screen wrapper. If any of them fills it, rename the field and log honeypot hits.
    location: >-
      sites/www/src/components/LeadForm.astro (honeypot input)
    severity: medium (unverified)
  - summary: >-
      Consent rows do not record which privacy-notice version the person agreed to.
    evidence: |-
      lead_submissions stores the label text and consented_at only. The notice is still a placeholder, and production builds refuse open forms until it is published, so add notice versioning in the story that publishes it.
    location: >-
      crates/wipe-hub/migrations/0008_leads.sql
    severity: medium
  - summary: >-
      AGENTS.md states a stale workspace test count (209) and has one over-long wrapped line.
    evidence: |-
      The story adds tests across wipe-hub; AGENTS.md is an agent-context file, so its edits are deferred.
    location: >-
      AGENTS.md (Running and verifying)
    severity: low
  - summary: >-
      The unsubscribe page's "act only on button press, never on load" rule has no component test.
    evidence: |-
      The portal has only api/*.test.ts and no DOM or testing-library setup; MagicLinkPage has the same gap. Adding a useEffect that mutates on mount would pass every test.
    location: >-
      apps/portal/src/pages/UnsubscribePage.tsx
    severity: low
  - summary: >-
      No automated test crosses the browser-to-hub joint (credentialed CORS and the SameSite=Lax CSRF cookie sent from localhost:4321 to localhost:7900).
    evidence: |-
      The hub tests use reqwest with injected cookies, and the site tests use a fake fetch. Only the manual acceptance criterion exercises a real browser. Settle it with a Playwright-style smoke test once a browser harness exists.
    location: >-
      sites/www/src/lib/lead-submit.ts / crates/wipe-hub/src/cors.rs
    severity: medium
  - summary: >-
      No policy on whether sales may answer, by hand, a direct contact message from a suppressed (unsubscribed) sales lead.
    evidence: |-
      The site tells every submitter "We will reply by email", while the lead stays suppressed. Story 2's admin list must show suppression and needs this product decision.
    location: >-
      .scratch/lead-capture story 2
    severity: low
---

<intent-contract>

## Intent

**Problem:** The marketing site's early-access and contact forms post nowhere, so interested visitors cannot become sales leads the vendor can follow up. This covers lead-capture CAP-1, CAP-2 and the storage and unsubscribe halves of CAP-4.

**Approach:** Add `POST /api/leads` to `wipe-hub`, behind credentialed CORS limited to one configured site origin and the existing double-submit CSRF token. It runs a per-IP throttle, a honeypot, validation and the email-send throttle, then upserts one lead per address, records each submission with its exact consent text and time, and emails a confirmation carrying an unsubscribe link. The link opens a portal page, and pressing its button marks the lead unsubscribed. The marketing site's forms submit with a small script, but only in builds given a hub origin.

## Boundaries & Constraints

**Always:**
- Term: a **sales lead** is a person who submitted the early-access or contact form. It is stored as a `leads` row and is never the elected **Lead** station. Add the term to CONTEXT §14 and use "sales lead" in prose. Code identifiers stay `lead`.
- CSRF (ADR-0007 §3): `POST /api/leads` keeps the existing double-submit check, with no exemption. The site fetches `GET /api/auth/csrf` and then posts, both with `credentials: "include"`.
- CORS: config `--site-origin` / `WIPE_HUB_SITE_ORIGIN`, default `http://localhost:4321` (Astro's dev port). It must be an http(s) origin with no path other than `/`, no query, fragment or userinfo; anything else is a start-up error naming the value. CORS headers go only on `GET /api/auth/csrf` and `POST /api/leads`, and on their `OPTIONS` preflights, and only when `Origin` equals the configured origin exactly. The headers are `Access-Control-Allow-Origin: <origin>`, `Access-Control-Allow-Credentials: true`, `Vary: Origin` and `Access-Control-Expose-Headers: Retry-After`. Preflights also get `Allow-Methods: GET, POST`, `Allow-Headers: content-type, x-csrf-token` and `Max-Age: 600`. The CORS layer wraps the CSRF middleware, so the site can read a 403, 400 or 429.
- Request body: `{kind: "early-access"|"contact", name, email, company, role?, segment?, message?, consent, consent_text, website?}`.
  - `name`, `company` and, for early access, `role` pass `accounts::validate_name`. `email` passes `accounts::validate_email`. Early access needs a `segment` in `itad|enterprise-it|government|other`; its message is optional.
  - Contact needs a message and must not carry a non-blank `role` or `segment`. A message is trimmed, at most 5000 characters, with no control characters other than `\n`, `\r` and `\t`.
  - `consent` must be `true`, else 400 `consent is required`. `consent_text` must equal the hub's text for that `kind` exactly, else 400 `the consent text has changed; reload the page and try again`.
- Order of work in the handler:
  1. Validate the body (400).
  2. Charge `lead-form:ip:<peer>` against `LEAD_IP_LIMIT = 10` per `LEAD_WINDOW = 1 h`. The charge is final; reaching the limit locks the key for 1 h, audited once as `lead.submit_locked`. A refusal is 429 `too many submissions; try again later` with `Retry-After`.
  3. A non-blank honeypot `website` gets 202 `{}`; nothing else is stored, charged, sent or audited.
  4. Charge the email-send throttle with `auth::charge_mail(address, peer)`; a refusal is its existing 429.
  5. In one transaction, upsert the lead, insert the submission and audit.
  6. Answer 202 `{}`.
  7. Send the email from a spawned task; a send failure is logged at `warn` and never returned.
- Lock summaries for `lead-form:` keys are audited as `lead.submit_lock_summary`. `auth::lock_summary_action` must map that prefix, because `purge_stale` returns every key.
- Upsert: there is one lead per `lower(email)`, enforced by a unique index. A new submission updates `email` (as typed this time), `name` and `company`. It updates `role` and `segment` only when it carries them, sets `updated_at` and never changes `status` (`new` on insert; CHECK `new|contacted|qualified|closed`). It **never clears `unsubscribed_at`**: an unsubscribe is a suppression that only a future confirmed opt-in may lift (decision 4). Every submission gets its own `lead_submissions` row: kind, message, consent text, `consented_at` (`now()`), and the SHA-256 of its own 32-byte OS-RNG unsubscribe token. The response is the same whether the address was new, known or unsubscribed.
- Audit (`audit::record`, in the same transaction): `lead.submitted` with actor `anonymous`, subject `lead:<id>` and detail `{kind, created, suppressed}`, where `suppressed` means the lead is unsubscribed so no email was sent. Rows *about a sales lead* (`lead.submitted`, `lead.unsubscribed`) carry no name, email, company, message or peer IP, so a later erasure leaves no personal data behind. Per-IP throttle rows are not about a lead: they follow the existing throttle convention. `lead.submit_locked` has the throttle key as its subject, and `lead.submit_lock_summary` (through `auth::audit_lock_summary`) adds `peer`. Both contain the peer IP and are never linked to a lead id. The throttle key prefix is `lead-form:ip:`, never `lead:`, so the subject `lead:<uuid>` always means a sales lead. A honeypot submission is charged like any other, so it can engage, and audit, that lock.
- Confirmation email: subject per kind, with a plain-text body naming the kind, saying the vendor will reply by email, and containing `{public_base_url}/unsubscribe?token=…` (built with `auth::portal_link`), plus a "not you? use this link" line. It never echoes any submitted text. **It is never sent to an unsubscribed lead.** The email-send throttle is still charged for every non-honeypot submission, so throttling cannot reveal an unsubscribed address.
- Unsubscribe: `POST /api/leads/unsubscribe {token}`. A token from any of the lead's submissions sets `unsubscribed_at` if it is null and audits `lead.unsubscribed` (`{}`), only when that changes. It answers 200 `{"unsubscribed": true}`, also when the lead is already unsubscribed. A malformed or unknown token is 404 `this unsubscribe link is not valid`. The portal page `/unsubscribe?token=` acts only on a button press, never on load (as `MagicLinkPage` does).
- Marketing site: `PUBLIC_HUB_ORIGIN` is unset by default. Unset, the forms stay exactly as they are now: no script, disabled button, "not open yet". Set, the form carries `data-hub-origin`; its button is disabled in the HTML and enabled by the script; it shows a `<noscript>` note; and it has a labelled, off-screen, `aria-hidden`, `tabindex=-1` honeypot `website`.
- `astro build` (not dev) fails when `PUBLIC_HUB_ORIGIN` is set while `site.privacyNoticePublished` is `false`, because the published privacy page promises the forms will not open before the notice exists.
- The consent wording lives once, in `sites/www/src/data/consent.ts`. The stored text is the label's whole text, `"<statement> See the privacy notice."`, and a Rust unit test pins the hub constants to that file with `include_str!`.

**Never:**
- No third-party form or CRM service, no double opt-in, no RFC 8058 one-click `List-Unsubscribe-Post` (it would need a CSRF exemption), no admin list, export or erasure (story 2), no email campaigns.
- No state change on a GET, no `X-Forwarded-For`, no editing an applied migration, no consent text taken from the client as authority.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| New early-access lead | valid body, CSRF, Origin = site | 202 `{}` with CORS headers; 1 lead (`status new`), 1 submission with consent text and time; `lead.submitted {created:true}`; mail in Mailpit with an unsubscribe link | — |
| Duplicate address | same email, different case, contact form | 202; still 1 lead, name and company updated, role and segment kept; 2 submissions; `created:false` | — |
| Resubmit after unsubscribe | lead unsubscribed | 202 `{}`; `unsubscribed_at` unchanged; new submission stored; email-send throttle charged; **no email**; `suppressed:true` | — |
| Honeypot | `website` non-empty | 202 `{}`; no lead, submission or mail; no `lead.submitted` | — |
| SMTP down | Mailpit unreachable | 202 `{}`; lead and submission stored; send failure logged at `warn` | — |
| Concurrent first submissions | two at once for one new address | both 202; 1 lead, 2 submissions, exactly one `created:true` | — |
| Lock expires after refusals | 11th refused, lock elapsed, one more | that one is 202; one `lead.submit_lock_summary` with subject `lead-form:ip:127.0.0.1`, `refused:1` | — |
| Bad email / missing field / consent false / stale consent text | — | 400 with the message; nothing stored or charged | — |
| 11th submission from one IP within 1 h | — | 429 + `Retry-After`; nothing stored | — |
| Mail throttle refuses | 6th to one address within 1 h | 429 `too many emails requested; try again later`; no new submission | — |
| Preflight from the site origin / from another origin | `OPTIONS /api/leads` | site origin: 2xx with the CORS headers; other origin: no `Access-Control-*` headers | — |
| No CSRF token | — | 403 `missing or invalid CSRF token`, with CORS headers for the site origin | — |
| Unsubscribe | valid token, twice | 200 both times; `unsubscribed_at` set; one `lead.unsubscribed` | unknown token → 404 |

## Autonomous decisions

1. **How the static site posts through the hub's CSRF check.** ADR-0007 §3 requires a CSRF token on every state-changing request, and the marketing site is on another origin. The options were exempting `/api/leads` and relying on an Origin check, or credentialed CORS that fetches the token. The choice is credentialed CORS, which keeps the ADR intact. Its cost is that the site and hub must be the same *site* (same registrable domain: `localhost:4321` → `localhost:7900` works, `localhost` → `127.0.0.1` does not), so hosting must keep them that way. This goes in CONTEXT §10 as a limitation.
2. **When the forms open.** The published privacy placeholder promises "the forms will not open before that notice is published", and the notice is legal copy for the vendor. The safe default is to keep the forms closed unless the build is given a hub origin, and to make a production build refuse an open form while the privacy notice is a placeholder. Development and local demos still work.
3. **How unsubscribe works.** Mail scanners open GET links, so the link leads to a portal page with a button, like magic links. RFC 8058 one-click unsubscribe (a header that lets the mail client unsubscribe directly) is deferred: it needs a POST without a CSRF token.
4. **What a resubmission does** (revised at review, pass 1). The first plan let a resubmission clear the unsubscribe. That would let anyone who knows an address re-subscribe a person who opted out, and send them up to 5 confirmations an hour. Recognised email practice (suppression lists; CAN-SPAM opt-out honouring) is that an unsubscribe sticks until the mailbox owner confirms otherwise. So a resubmission is stored, with its own consent text, and the vendor sees it, but `unsubscribed_at` stays set and no email goes to the address. Re-subscribing needs a confirmed opt-in (an emailed confirm link), which is deferred to a later story and recorded in CONTEXT §10. CAP-2's "duplicate email updates" still holds: name, company, role and segment update, and status never changes.
5. **Personal data in the audit log.** Lead audit rows hold ids and flags only, never names, emails or IPs, so story 2's erasure can leave "an audit entry without the personal data" (CAP-4).
6. **Spam relaying.** The confirmation email echoes no submitted text, so the form cannot be used to send arbitrary text to an arbitrary address.
7. **Throttle limits.** Lead submissions get their own `lead-form:ip:` key, 10 per hour; emails reuse the existing email-send throttle, so a lead confirmation counts against the same per-address budget as sign-in links.

</intent-contract>

## Code Map

- `_bmad-output/implementation-artifacts/lead-capture-story-1-attempt-1.patch` -- review pass 1's reverted attempt. Start from it (`git apply`), then make the changes in the Spec Change Log. Everything the KEEP list names is already right there.
- `crates/wipe-hub/src/lib.rs:70-99` -- `router()`; merge the leads routes and wrap CORS outside `.layer(csrf::require)`, for the two paths only.
- `crates/wipe-hub/src/csrf.rs:221` -- `require`: OPTIONS passes as safe; `get_token` (`/api/auth/csrf`) is the endpoint the site calls.
- `crates/wipe-hub/src/auth.rs:493` `email_key`, `:516` `ip_key` (`ip:<v4>` or `ip:<v6>/64`), `:537` `lock_summary_action`, `:547` `audit_lock_summary`, `:566` `locked_response`, `:636` `charge_mail` (reuse as is), `:297` `portal_link`; `request_link` at `:967` is the template for validate → charge → 202 → spawn the send.
- `crates/wipe-hub/src/throttle.rs` -- `charge` / `confirm_failure` / `purge_stale`; follow `charge_mail`'s confirm-at-once pattern for `lead-form:ip:`. Add the `LEAD_*` constants and pin them in its tests.
- `crates/wipe-hub/src/accounts.rs:143` `validate_email`, `:179` `validate_name`, `:266` token generation, `:277` `hash_token` (base64url 32 bytes → SHA-256).
- `crates/wipe-hub/src/mail.rs` -- add the subjects and `send_lead_confirmation`, with a body unit test (no echo; link present).
- `crates/wipe-hub/src/config.rs` -- `Args` (`:46`) and `Config` (`:137`) gain `site_origin`; the constant `DEFAULT_SITE_ORIGIN`; update the `clap_defaults_match_the_constants` and env-prefix tests.
- `Args { … }` literals that must gain the field: `crates/wipe-hub/src/config.rs` tests, `tests/support/hub.rs:43`, `tests/{health,team,portal,auth}.rs`.
- `crates/wipe-hub/migrations/0006_invitations.sql` -- style template (uuid PK, `gen_random_uuid()`, `bytea` hash CHECK 32, comment header). The next migration is `0008`.
- `crates/wipe-hub/tests/isolation.rs:40-115` -- `ROUTES`: the completeness guard fails until both new routes are classified `Anonymous(...)`.
- `crates/wipe-hub/tests/support/{mod.rs,hub.rs}` -- `spawn_hub`, `Hub::post` (adds the CSRF cookie and header), `count`, `audited`, `mail_texts`, `token_in(text, "unsubscribe")`, `unique_address`, `error_of`.
- `apps/portal/src/main.tsx:112-130` -- route pattern with `validateSearch` for `token`; `pages/MagicLinkPage.tsx` is the page template; `api/auth.ts` has the CSRF-aware POST helper and `CSRF_ERROR`, and `api/auth.test.ts` is the test pattern.
- `sites/www/src/components/LeadForm.astro` -- current form (field names already match). `src/config.ts` is the site config. `src/integrations/site-check.ts` is the `astro:build:done` hook (add the guard at build start). `test/pages.test.ts:56-170` holds the form and no-JS assertions to split into closed and open modes.
- Docs: `CONTEXT.md` §10.2 (new row; the marketing row's "forms post nowhere" gap) and §14; `README.md:51,186`; `sites/www/README.md:33-40`; and `AGENTS.md`'s marketing and hub lines (the `PUBLIC_HUB_ORIGIN` and `--site-origin` flags).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0008_leads.sql` -- create the `leads` table (unique `lower(email)` index; CHECKs on segment and status; `created_at`, `updated_at`, `unsubscribed_at`) and `lead_submissions` (FK `ON DELETE CASCADE`, kind CHECK, message, non-blank `consent_text`, `consented_at`, unique 32-byte `unsubscribe_token_hash`, index on `lead_id`) -- storage for CAP-1 and CAP-4.
- `crates/wipe-hub/src/config.rs` -- the `site_origin` flag, env var, validation and tests -- CORS origin.
- `crates/wipe-hub/src/leads.rs` (new) -- validation, consent constants with the `include_str!` pin test, upsert, submission and token, unsubscribe, audit -- the data layer and its unit tests.
- `crates/wipe-hub/src/lead_routes.rs` (new) + `lib.rs` -- the handlers in the order above, path-scoped CORS outside CSRF, the spawned send -- the public API.
- `crates/wipe-hub/src/auth.rs`, `throttle.rs`, `mail.rs` -- the `lead:` lock-summary action, the `LEAD_*` constants, the confirmation email -- reuse.
- `crates/wipe-hub/tests/leads.rs` (new) + `tests/isolation.rs` + the `Args` literals -- one integration test per I/O-matrix row -- CAP-2 needs a test for each defence.
- `apps/portal/src/api/leads.ts` (+ `.test.ts`), `pages/UnsubscribePage.tsx`, `main.tsx` -- the unsubscribe page.
- `sites/www/src/data/consent.ts`, `src/lib/lead-submit.ts` (+ `test/lead-submit.test.ts`), `src/components/LeadForm.astro`, `src/config.ts`, `src/integrations/site-check.ts`, `test/pages.test.ts`, and the early-access and contact pages' copy and meta (factual: the forms are open only when the build enables them) -- wire the forms.
  - `lead-submit.ts` builds the JSON body from the form; does the CSRF fetch and POST, retrying once on a CSRF 403; and maps 202, 400, 429 and network errors to the status text shown in an `aria-live` region.
  - The success text must not claim an email was sent: the hub sends after answering, and never sends to a suppressed address.
  - A 429 shows the hub's own `error` text (sentence-cased) plus "Try again in N minutes." from `Retry-After`, because the per-IP limit and the email-send limit say different things.
- `sites/www/test/lead-form-dom.test.ts` (new; `happy-dom` dev dependency, with a per-file `// @vitest-environment happy-dom`) -- check that `initLeadForms` enables the button, that submit calls `preventDefault` and posts to `${hub}/api/leads` with a fake `fetch`, that the status region shows the outcome, and that the form resets only on success. Closed forms (no `data-hub-origin`) stay untouched -- the script is the only thing between a visitor and a native submit that would leak their details.
- Docs listed in the Code Map -- the docs ship in the same commit as the code (AGENTS.md).

**Acceptance Criteria:**
- Given the hub, Mailpit and `PUBLIC_HUB_ORIGIN=http://localhost:7900 pnpm --filter www dev`, when a visitor submits the early-access form at `http://localhost:4321/early-access/`, then the page shows a success message, a `leads` row exists, and Mailpit holds a confirmation whose link opens the portal's `/unsubscribe` page, where pressing the button sets `unsubscribed_at`.
- Given a default build (`pnpm --filter www build` with no `PUBLIC_HUB_ORIGIN`), when the pages are rendered, then the forms ship no `<script>` and stay disabled and "not open yet". Given `PUBLIC_HUB_ORIGIN` is set, then `astro build` fails, naming the unpublished privacy notice.
- Given `wipe-hub serve --site-origin https://x.example/path`, when it starts, then it exits non-zero naming the value.

## Spec Change Log

### 2026-11-08 — Review pass 1 (bad_spec loopback)
- **Triggering findings:** a resubmission cleared the unsubscribe and re-sent confirmations to people who had opted out (blind hunter 2 and 3). The spec claimed "lead audit rows carry no peer IP", but `lead.submit_locked` and `lead.submit_lock_summary` did, under a `lead:` subject prefix shared with sales-lead rows (blind hunter 9; edge-case hunter 1-3; verification-gap "other"). The success copy claimed an email "has been sent", and a mail-throttle 429 was shown as "too many submissions from here" (blind hunter 1 and 6; edge-case hunter 5). Tests were missing for the lock summary, the concurrent upsert, SMTP down and `initLeadForms` (verification gap 1-3; blind hunter 11).
- **Amended:** intent-contract decision 4 and the upsert, audit, email, throttle-key and matrix rules, under the owner's decide-by-default rule. Now an unsubscribe is sticky, no email goes to a suppressed lead, the audit detail is `suppressed` instead of `resubscribed`, the throttle key is `lead-form:ip:`, and the lock rows' IP is stated. Matrix rows added for SMTP down, concurrent first submissions and lock expiry. The Tasks section gains the copy rules and the DOM test.
- **Known-bad state avoided:** a third party re-subscribing and mailing an opted-out address, and doc comments claiming no IP is stored where one is.
- **KEEP** (everything below is right in the attempt-1 patch and must survive):
  - `cors.rs`: path-scoped, wraps CSRF, exact single-`Origin` match, with its unit test.
  - The `--site-origin` config, its validation and tests, and every updated `Args` literal.
  - Migration `0008_leads.sql`: same tables, no `resubscribe` column needed.
  - `leads.rs`: validation, the error constants, the consent constants with the `include_str!` pin test, the race-safe `upsert` loop, `unsubscribe`, and its unit tests.
  - `lead_routes.rs`: the handler order, `charge_submission`, and the spawned send with its `warn` log.
  - `mail.rs`: `send_lead_confirmation` and its no-echo test.
  - `throttle.rs`: the `LEAD_*` constants, including `LEAD_LOCKOUT`.
  - The `tests/isolation.rs` classification, `tests/cli.rs` and the `tests/migrations.rs` additions.
  - The `tests/leads.rs` structure.
  - The portal's `api/leads.ts` with its tests, `UnsubscribePage.tsx` and the route.
  - The www side: `consent.ts`, `parseHubOrigin`, `leadBody`, `submitLead`, `tryAgainIn` and their tests; `LeadForm.astro`'s closed and open modes, honeypot, `<noscript>` and the conditional `<script>`; the `config.ts` `privacyNoticePublished` and `hubOrigin()`; the site-check Vite-plugin build guard; the page copy and meta switching.
  - The docs edits (CONTEXT §10.2 row and §14 "Sales lead", README, `sites/www/README.md`, AGENTS.md). Update them to the new resubscribe and throttle-key behaviour, and list in CONTEXT §10 the limitations "re-subscribing needs a confirmed opt-in (not built)", "consent rows do not record the privacy-notice version; add it before the notice is published" and "the throttle lock rows keep the peer IP".

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 27 findings — high 0, medium 7, low 17, false 1, maybe-false 2
- findings:
  - `[medium]` `[bad_spec]` (blind 1) Success copy claims a confirmation "has been sent" before the spawned send, and also on a honeypot or SMTP failure — amended Tasks: the success text must not claim a send.
  - `[medium]` `[bad_spec]` (blind 2) A third party can clear a lead's unsubscribe by resubmitting the address — amended intent decision 4 and the upsert rule: an unsubscribe is sticky.
  - `[medium]` `[bad_spec]` (blind 3) Confirmations keep going to an unsubscribed address, up to 5 an hour — grouped with blind 2; amended the email rule: never sent to a suppressed lead.
  - `[maybe-false]` `[defer]` (blind 4) Autofill may fill the `website` honeypot and silently drop a real visitor — would need browser and password-manager behaviour checked against `name="website" autocomplete="off"`; if true, medium.
  - `[medium]` `[defer]` (blind 5) Consent rows do not record which privacy-notice version was agreed to — real, but the notice does not exist yet and production builds cannot open the forms until it does; versioning belongs with publishing the notice (CONTEXT limitation added by amendment).
  - `[low]` `[bad_spec]` (blind 6) Every 429 is shown as "too many submissions from here", including the email-send limit's — amended Tasks: show the hub's error text.
  - `[low]` `[reject]` (blind 7) The default `--site-origin` is localhost even on a production hub, with no start-up warning — an attacker would need to run code on the victim's own `localhost:4321`, and a cross-site request from there gets no `SameSite=Lax` cookie; a warning adds a branch for a near-unreachable case.
  - `[low]` `[reject]` (blind 8) The same-site rule between site and hub is not checked at start-up — a correct check needs Public Suffix List logic (the list of registrable domains), which is more than a direct fix; the rule is documented in CONTEXT §10.
  - `[medium]` `[bad_spec]` (blind 9) `lead.submit_locked` and `lead.submit_lock_summary` carry the peer IP under a `lead:` subject prefix shared with sales-lead rows, contradicting the spec and the doc comment — amended the audit rule: key prefix `lead-form:ip:`, the lock rows' IP stated.
  - `[low]` `[reject]` (blind 10) The Rust pin test matches source formatting, so reformatting `consent.ts` breaks it — it fails loudly with a message naming the file and line to restore; a shared JSON file is more than a direct correction.
  - `[low]` `[bad_spec]` (blind 11) No tests for SMTP down, the concurrent upsert or the `lead:` lock summary — matrix rows added.
  - `[low]` `[defer]` (blind 12) AGENTS.md's "209 tests" count is stale and one wrapped line is ~150 characters — agent-context file, so deferred.
  - `[low]` `[reject]` (blind 13) Invalid bodies are not charged to the per-IP limit — this is the spec's deliberate order (validate first, so a 400 costs nothing), and validation is cheap; changing it would mean editing this build's spec.
  - `[low]` `[bad_spec]` (edge 1) The lead lock-summary detail carries the peer IP — grouped with blind 9.
  - `[low]` `[bad_spec]` (edge 2) A honeypot submission that reaches the IP limit writes `lead.submit_locked`, though the matrix said a honeypot writes no `lead.*` row — the matrix and audit rule now say the honeypot is charged and can audit the lock (grouped with blind 9).
  - `[low]` `[bad_spec]` (edge 3) `lead.submit_locked` has the IP in its subject — grouped with blind 9.
  - `[maybe-false]` `[defer]` (edge 4) Autofill may fill the honeypot — grouped with blind 4.
  - `[low]` `[bad_spec]` (edge 5) A 429 from the email-send throttle is shown with the per-IP text — grouped with blind 6.
  - `[low]` `[reject]` (edge 6) CORS headers are added for any method on the two paths, e.g. a 405 for `GET /api/leads` — the site origin can read nothing more than a 405 body; restricting by method adds a guard for no reachable harm.
  - `[low]` `[bad_spec]` (gap 1, pre-verified) `lead.submit_lock_summary` is never produced by any test — matrix row "Lock expires after refusals" added.
  - `[medium]` `[bad_spec]` (gap 2, pre-verified) `initLeadForms` is untested; dropping `preventDefault` would leak details through a native submit with every test green — Tasks gains `lead-form-dom.test.ts` (happy-dom).
  - `[low]` `[bad_spec]` (gap 3, pre-verified) The concurrent-insert retry in `upsert` never runs in tests — matrix row "Concurrent first submissions" added.
  - `[low]` `[defer]` (gap 4, pre-verified) The unsubscribe page's button-only rule has no component test — the portal has no component-test setup (`MagicLinkPage` shares the gap); filed as defer.
  - `[low]` `[bad_spec]` (gap other) Doc comment says lead audit rows never carry a peer, but lock rows do — grouped with blind 9.
  - `[false]` `[reject]` (intent 1) Production builds cannot take sales leads — the intent is to wire the forms, which this does. Opening them in production waits on the privacy notice, because the published placeholder promises the forms will not open before it exists, so the gate honours an existing public commitment and is not a defect (decision 2).
  - `[medium]` `[defer]` (intent 2) No automated test crosses the browser ↔ hub joint (credentialed CORS, `SameSite=Lax` cookie across ports) — needs a browser harness, which widens scope; the manual acceptance criterion is the check.
  - `[low]` `[defer]` (intent 3) Opening the emailed link and pressing the button is not tested end to end — grouped with gap 4.

### 2026-11-08 — Review pass
- verdicts: 26 findings — high 0, medium 1, low 17, false 7, maybe-false 1
- findings:
  - `[low]` `[reject]` (blind 1) A resubmission for a known address overwrites name and company, even on a qualified or unsubscribed lead — CAP-2 requires "duplicate email updates rather than duplicates"; abuse needs someone to know the address and want to deface a lead, the vendor sees the new submission, and the fix would mean per-submission profile history (schema and code), not a direct correction.
  - `[low]` `[defer]` (blind 2) No policy on whether sales may answer a direct contact message from a suppressed lead — a product question for story 2's admin list; deferred.
  - `[low]` `[patch]` (blind 3) The unsubscribe page said it stops contact "about this request", but it suppresses the whole address for good — reworded to "stop all email from us to this address".
  - `[low]` `[patch]` (blind 4) The "only the hash is stored" assertion compared a 43-byte string with a 32-byte column, so it could never fail — it now asserts the stored hash equals `accounts::hash_token(token)` and that no column holds the raw token.
  - `[low]` `[reject]` (blind 5) A cross-site misconfiguration shows "HTTP 403" with no hint — this only happens with an operator misconfiguration (documented in CONTEXT §10), and a dedicated message adds a branch.
  - `[low]` `[reject]` (blind 6) A 5xx from `GET /api/auth/csrf` is shown as a network error — that route never touches the database, so only a proxy failure reaches it; the fix adds an error type and a branch.
  - `[low]` `[patch]` (blind 7) The privacy placeholder's "sets no cookies" becomes false once forms open, and nothing tracked that — CONTEXT §10 now says the published notice must disclose the `wipehub_csrf` cookie and the cross-origin data flow.
  - `[false]` `[reject]` (blind 8) `Vary: Origin` is sent only on a matching origin, so a cache could serve the wrong copy — `GET /api/auth/csrf` is `Cache-Control: no-store` and `POST /api/leads` responses are not cacheable, so no shared cache stores either.
  - `[low]` `[patch]` (blind 9) Only one `--site-origin` can be configured — recorded as a CONTEXT §10 limitation (apex and `www.` cannot both host the forms).
  - `[low]` `[patch]` (blind 10) Unsubscribing did not bump `updated_at` — the UPDATE now sets it when it changes the row.
  - `[low]` `[defer]` (blind 11) AGENTS.md's over-long line comes from this diff — carried: same location and claim as pass 1's blind 12; an agent-context edit, so deferred.
  - `[low]` `[reject]` (blind 12) A suppressed submission stores an unsubscribe token that is never emailed, and this has no test — harmless (the column is NOT NULL and unique; an unsent token is never usable), and the suppression test covers the path.
  - `[low]` `[reject]` (edge 1) A CSRF-fetch HTTP error is shown as a network error — grouped with blind 6.
  - `[maybe-false]` `[defer]` (edge 2) Autofill may fill the honeypot — carried: same claim as pass 1's blind 4 and edge 4; already deferred.
  - `[low]` `[patch]` (edge 3) A textarea submits CRLF line breaks, so a message within `maxlength` could exceed the hub's 5000 characters — `leadBody` now turns `\r\n` into `\n`, with a unit test.
  - `[low]` `[reject]` (edge 4) An unsubscribe that commits between the submission's commit and the spawned send still gets that one confirmation — a millisecond window right after the person's own submission; re-checking adds a query and a branch for one email.
  - `[low]` `[patch]` (gap 1, pre-verified) The HTTP concurrency test does not force the insert-race retry in `upsert` — added `an_insert_that_loses_the_race_retries_as_an_update`, which uses two transactions and waits on `pg_stat_activity`.
  - `[false]` `[reject]` (intent a) Production builds cannot open the forms — carried: same as pass 1's intent 1 (the gate honours the published privacy promise, decision 2).
  - `[medium]` `[defer]` (intent b) No automated browser ↔ hub test — carried: same as pass 1's intent 2.
  - `[low]` `[defer]` (intent c) The email-link ↔ unsubscribe-page loop is untested end to end — carried: same as pass 1's intent 3 and gap 4.
  - `[false]` `[reject]` (intent d) The SPEC's "reflected in the portal" is not met — the admin list is CAP-3, which the story split assigns to story 2; this story's intent names CAP-1, CAP-2 and the storage and unsubscribe halves of CAP-4.
  - `[false]` `[reject]` (intent e) The diff implements the sticky-unsubscribe reading of "duplicate email updates" — this is the recorded decision 4; descriptive, not a defect.
  - `[false]` `[reject]` (intent f) Only the lead confirmation carries an unsubscribe link — CAP-4's "every email" concerns email to a sales lead, and the confirmation is the only one.
  - `[low]` `[reject]` (intent g) Behind a reverse proxy, every visitor shares one per-IP key — the spec forbids `X-Forwarded-For` (the existing throttle convention, ROADMAP §0.5); changing it means editing this build's spec.
  - `[false]` `[reject]` (intent h) "Store only what the form asks for" versus IPs in the lock rows — the code matches the amended spec: the lock rows are throttle records under `lead-form:`, never linked to a lead, and documented in CONTEXT §10.
  - `[false]` `[reject]` (intent i) CORS does not stop a POST reaching the handler — by design, the `SameSite=Lax` CSRF cookie and token are the cross-site block; CORS only decides whether a browser lets the site read the response.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including `tests/leads.rs` and the isolation guard
- `cargo test --workspace` -- expected: passes (never piped through grep)
- `cargo clippy -p wipe-hub --all-targets -- -D warnings` -- expected: clean
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: pass
- `pnpm --filter www typecheck && pnpm --filter www test && pnpm --filter www build` -- expected: pass (default build: forms closed)
- `rustfmt --edition 2021 <touched .rs files>` -- only the touched files

## Auto Run Result

**Status:** done (2 review passes; 1 bad_spec loopback)

**Summary:** `wipe-hub` now takes sales leads from the marketing site.
- `POST /api/leads` checks the double-submit CSRF token with no exemption, behind credentialed CORS limited to `--site-origin`. It runs, in order: validation, a per-IP limit (10 an hour, key `lead-form:ip:`), a honeypot, then the existing email-send throttle.
- It keeps one lead per `lower(email)`. Each submission is stored with its exact consent text and time and its own hashed unsubscribe token, and the visitor gets a confirmation email with a portal `/unsubscribe` link, where a button press unsubscribes.
- An unsubscribe is sticky: a resubmission is stored, but no email goes to that address again.
- The marketing site's forms post through a small script, but only when the build is given `PUBLIC_HUB_ORIGIN`. A production `astro build` refuses that while the privacy notice is still a placeholder.

**Files changed:**
- Hub storage and API:
  - `crates/wipe-hub/migrations/0008_leads.sql` — `leads` and `lead_submissions`.
  - `crates/wipe-hub/src/leads.rs` — validation, consent constants pinned to the site, upsert, unsubscribe and audit.
  - `crates/wipe-hub/src/lead_routes.rs` — the two routes.
  - `crates/wipe-hub/src/cors.rs` — path-scoped credentialed CORS outside the CSRF check.
  - `crates/wipe-hub/src/lib.rs` — wiring.
- Hub config and reuse:
  - `crates/wipe-hub/src/config.rs` — `--site-origin` / `WIPE_HUB_SITE_ORIGIN`.
  - `crates/wipe-hub/src/throttle.rs` — the `LEAD_*` constants.
  - `crates/wipe-hub/src/auth.rs` — the lock-summary action for `lead-form:`.
  - `crates/wipe-hub/src/mail.rs` — the confirmation email.
  - `crates/wipe-hub/src/csrf.rs` — the doc comment.
- Hub tests:
  - `crates/wipe-hub/tests/leads.rs` — 14 integration tests, one per matrix row plus a deterministic race test.
  - `tests/isolation.rs` — route classification.
  - `tests/cli.rs` and `tests/migrations.rs` — new checks.
  - `tests/{support/hub,auth,health,portal,team}.rs` — the `Args` field.
- Portal: `apps/portal/src/api/leads.ts` (with tests), `pages/UnsubscribePage.tsx` and the `/unsubscribe` route in `main.tsx`.
- Marketing site:
  - `sites/www/src/data/consent.ts`.
  - `src/lib/lead-submit.ts` (with `test/lead-submit.test.ts`, plus `test/lead-form-dom.test.ts` under happy-dom).
  - `src/components/LeadForm.astro` (closed and open modes).
  - `src/config.ts`.
  - `src/integrations/site-check.ts` (build guard).
  - The early-access and contact pages.
  - `test/pages.test.ts` and `test/site-check-integration.test.ts`.
  - `package.json` and `pnpm-lock.yaml` (happy-dom).
- Docs: `CONTEXT.md` (§10.2 row with limitations, the marketing row, §14 "Sales lead"), `README.md`, `sites/www/README.md`, `AGENTS.md`.
- `_bmad-output/implementation-artifacts/lead-capture-story-1-attempt-1.patch` — the reverted first attempt, kept for the record.

**Review findings:**
- **Pass 1:** 27 findings. These were routed to bad_spec and re-derived:
  - the sticky unsubscribe;
  - IPs in the lock rows under a `lead:` prefix;
  - the success and 429 copy;
  - missing tests.

  Deferred: autofill on the honeypot (unverified), privacy-notice versioning, the AGENTS.md test count and line length, the unsubscribe page's component test, and a browser ↔ hub test.

  Rejected:
  - the production default site origin, because the threat is near-unreachable;
  - the same-site start-up check, because it needs Public Suffix List logic;
  - the pin test's formatting coupling, because it fails loudly;
  - charging invalid bodies, because the spec order is deliberate;
  - CORS on non-spec methods, because a 405 exposes nothing.

  One finding was false: the production forms gate honours the privacy promise.
- **Pass 2:** 26 findings. Patches applied, all `low`:
  - the unsubscribe page wording;
  - the hash assertion that could never fail;
  - two CONTEXT limitations (cookie disclosure and the single site origin);
  - `updated_at` on unsubscribe;
  - CRLF line breaks in messages;
  - the deterministic upsert race test.

  Deferred: the policy for replying to a suppressed lead, for story 2; the carried defers are unchanged.

  Rejected, with reasons in the triage log:
  - overwriting on a duplicate (CAP-2 mandates it);
  - the misconfiguration 403 message;
  - a 5xx on the CSRF fetch;
  - unsent tokens stored on suppressed submissions;
  - the unsubscribe-versus-send window;
  - the per-IP key behind a proxy.

  Seven were false.
- **Patched counts (pass 2):** high 0, medium 0, low 7.

**Follow-up review recommendation:** false. No high-verdict patch, and no two medium-verdict patches.

**Verification:**
- `cargo test --workspace`: exit 0. `tests/leads.rs`: 14 passed. The isolation guard passed.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean. Without `--no-deps` it fails on `wipe-common/src/job.rs` (`large_enum_variant`), which also fails at the baseline revision and is not part of this story.
- Portal: typecheck, tests and build pass.
- www: typecheck, 216 tests and the default build pass. The built form pages contain no `<script>`.
- With `PUBLIC_HUB_ORIGIN` set, `astro build` fails, naming the unpublished privacy notice.
- `wipe-hub serve --site-origin https://x.example/path` exits 1, naming the value.
- `rustfmt --edition 2021 --check` is clean on the touched files.

**Residual risks:**
- Nothing has been tested in a real browser. That the browser sends the `SameSite=Lax` CSRF cookie from `localhost:4321` to `localhost:7900` is untested; the manual acceptance criterion is the check.
- The forms cannot open in a production build until the vendor publishes a privacy notice. That notice must disclose the hub cookie and the cross-origin data flow.
- Re-subscribing needs a confirmed opt-in, which is not built.
- `sites/www`'s lockfile is sensitive: an unfrozen `pnpm add` or `pnpm update` re-resolves `@emnapi/runtime` to a beta that crashes `astro check`.
