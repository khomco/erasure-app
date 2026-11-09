---
title: 'Count free-tier erasures'
type: 'feature'
created: '2026-11-08'
baseline_revision: 'b2fb3ff91eee1991240209100d9e5208d49ab6a3'
status: 'done'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0005-licensing-and-attestation.md']
warnings: ['oversized']
deferred:
  - summary: >-
      AGENTS.md still says `cargo test --workspace` runs 814 tests; the workspace now runs about 850.
    evidence: |-
      This story added 6+ tests; AGENTS.md is an agent-context file, which build-auto does not edit.
    location: >-
      AGENTS.md
    severity: low
  - summary: >-
      No test checks that a licensed station's destroyed drive consumes its Count quota (record_erasure on the Destroyed path).
    evidence: |-
      Pre-existing: every licensed-count assertion drives Jobs only to Erased. The new http_e2e tests cover the evaluation count on the destroy path only.
    location: >-
      crates/wipe-server/src/app.rs
    severity: low
---

<intent-contract>

## Intent

**Problem:** ADR-0005 §5 says a free-tier station's erasures are counted and surfaced, but the certificate hook calls `record_erasure` only for licensed certificates, so an unlicensed station counts nothing (ADR-0005 As built item 5, CAP-4).

**Approach:** Add an `evaluation_erasures` count to `LeaseState`, kept apart from the licensed `erasures_used`. The certificate hook records an evaluation erasure for every `evaluation` certificate of a Job that holds an erasure event, persisted through the same lease store and writer thread as the licensed count. `wipestation serve` prints both counts at start-up.

## Boundaries & Constraints

**Always:** Counting never blocks, delays or fails an erasure or a signature. It runs in the same `spawn_blocking` pattern as `record_erasure`, and a save failure is only logged (ROADMAP §0.2). The counting rule for *which* certificates count is the hook's existing one (`job.latest_erasure().is_some()`), split by `decision.evaluation`. `evaluation_erasures` is `#[serde(default)]`, so lease files written before this story still load with 0. It survives a licence-id change and an unlicensed start, like the watermark. `erasures_used` and the `Count` quota check are unchanged. Docs change in the same commit.

**Never:** No free-tier volume limit, threshold or warning text tied to the count. ADR-0012 decided the free tier withholds nothing but the marking, and a number would be a pricing decision. No new HTTP route, TS type or UI (`licence-visibility` owns that). No change to `LeaseStatus` variants or `evaluate`. No change to `wipe-engine*`, `wipe-common` or `wipe-cert`. No change to whether destroyed Jobs count. No hardware-sanitization claim.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Unlicensed station | no licence, erase 2 drives | `evaluation_erasures` 2, `erasures_used` 0, both certs `evaluation` | none |
| Licensed past quota | `Count { 1 }`, erase 2 drives | `erasures_used` 1, `evaluation_erasures` 1 | none |
| Restart | unlicensed, 2 evaluation erasures, rebuild from same dir | `evaluation_erasures` 2 | none |
| Old lease file | JSON without `evaluation_erasures` | parses, count 0 | none |
| Licence change | stored licence "old", 5 used, 4 evaluation; open with "new" | `erasures_used` 0, `evaluation_erasures` 4 | none |
| Save fails | unlicensed, store `save` errors | count still incremented in RAM; certificate signed `evaluation` | logged |

## Autonomous decisions

- **Which certificates count as evaluation erasures.** A licensed station whose lease has expired or whose quota is used up also issues `evaluation` certificates. Options: (a) count only stations with no licence installed; (b) count every `evaluation` certificate of an erased drive, whatever the reason. Chosen (b). The story's own wording is "record evaluation erasures separately", and (b) makes the two counts partition every counted certificate. It changes nothing a customer sees beyond the start-up line.
- **Station lifetime, not per licence.** On a licence change the licensed count restarts (story 2 decision). The evaluation count is a fact about the station's unlicensed output, so it is kept, like the watermark.
- **Where it is surfaced.** No licence route or UI exists yet (`licence-visibility` is queued). Chosen: `LicenseContext::lease_state()` and a start-up line from `wipestation serve`, as story 2 did for the persistence tier.

</intent-contract>

## Code Map

- `crates/wipe-license/src/lease.rs:22-58` -- `LeaseState` (add `evaluation_erasures`, `#[serde(default)]`, init 0 in `new`); doc says `erasures_used` is licensed-only.
- `crates/wipe-server/src/licensing.rs:266-277` -- `record_erasure`; add `record_evaluation_erasure` (saturating add + `persist`). `open` (:124-147) resets only `erasures_used` on licence change, so it needs no change, but test it.
- `crates/wipe-server/src/app.rs:338-347` -- hook counts only `!decision.evaluation && job.latest_erasure().is_some()`; branch on `decision.evaluation` under the same erasure condition.
- `crates/wipe-cli/src/main.rs:500-508,870-876,883-925` -- `lease_store_line` pattern; add a `lease_counts_line(&LeaseState)` printed after the `lease store:` line, with unit tests near :1150-1245.
- `crates/wipe-server/tests/lease_store.rs:500-600` -- `station`, `erase_to_certificate`, `chain(.., Quota::Count{..}, &key.public_key_id())` helpers to reuse for end-to-end rows; `stored()` helper at :68. The unlicensed variant needs `LicenseContext::open(.., None, ..)`.
- `crates/wipe-license/tests/attestation.rs:420-430` -- old-JSON parse test; extend or add one for the new field.
- No `LeaseState { .. }` literal initializers exist in `crates/*/tests`; all go through `LeaseState::new`.
- Docs: `CONTEXT.md:918` ("free-tier count not tracked"), `CONTEXT.md:~1000-1010` (§10 item 8 shipped list), `CONTEXT.md:1390-1395` (lease glossary: count of licensed erasures); `docs/adr/0005-licensing-and-attestation.md` As built item 5 (annotate the free-tier half resolved 2026-11-08, keep the original text; say the destroy-path half is unchanged); `README.md:401-403` test inventory rows and counts.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-license/src/lease.rs` -- add the field and docs -- CAP-4.
- `crates/wipe-server/src/licensing.rs` -- `record_evaluation_erasure`, documented as best-effort like `record_erasure` -- CAP-4.
- `crates/wipe-server/src/app.rs` -- record evaluation erasures in the hook through `spawn_blocking` -- CAP-4.
- `crates/wipe-cli/src/main.rs` -- `lease_counts_line`, printed at serve start-up; unit test the wording for an unlicensed context -- surfacing.
- `crates/wipe-server/tests/lease_store.rs` -- one test per matrix row; unlicensed, quota and restart rows drive real erasures through `AppState` with the mock backend -- CAP-4.
- `crates/wipe-license/tests/attestation.rs` -- old JSON without the field parses to 0.
- `CONTEXT.md`, `README.md`, ADR-0005 -- state that evaluation erasures are counted and persisted, shown only at start-up, with no limit attached.

**Acceptance Criteria:**
- Given an unlicensed station with a writable config directory, when two drives are erased to certificates and the station is rebuilt from that directory, then its lease shows 2 evaluation erasures and 0 licensed ones.
- Given a licensed station with `Count { erasures: 1 }`, when two drives are erased, then the lease shows 1 licensed and 1 evaluation erasure.
- Given the workspace, when `cargo test --workspace` runs with compose up, then it passes.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 23 findings — high 0, medium 0, low 16, false 7, maybe-false 0
- findings:
  - `[low]` `[patch]` (blind) `lease_counts_line` says "under the current licence" on an unlicensed start that kept a previous licence's count — the line now names the licence id the count belongs to, or says there is none; test extended.
  - `[low]` `[reject]` (blind) Counts line prints 0/0 as fact when the lease file failed to load — rare (corrupt file); the `lease store:` line printed right above already names the fault, and a fix needs a new fault parameter.
  - `[low]` `[patch]` (blind) Lease store details (`LocalFileLeaseStore::detail`, RAM-only texts) omit the evaluation count — each now mentions it, keeping the asserted reset substring.
  - `[false]` `[reject]` (blind) A failed evaluation-count save makes the next licensed certificate `evaluation` — `status()` persists before `with_fault`, so a later successful save clears the fault before the verdict; only a second failing save marks it, which is correct.
  - `[low]` `[patch]` (blind) `LicenseContext::open` doc says "the count restarts" — now says the licensed count restarts and the evaluation count is kept.
  - `[low]` `[patch]` (blind) CONTEXT.md §10 item 8 still says "erasure count" — now "erasure counts".
  - `[false]` `[reject]` (blind) Desktop shell loses the count, undocumented — the same CONTEXT.md row and the lease glossary already say lease state is RAM-only in the desktop shell.
  - `[low]` `[defer]` (blind) AGENTS.md test count stale (814) — agent-context file; deferred.
  - `[low]` `[patch]` (blind) No restart test with mixed counts or unlicensed start after a licensed run — quota test now rebuilds an unlicensed station and asserts 1 licensed, 1 evaluation.
  - `[low]` `[patch]` (blind) Docs don't say which destroyed drives count — grouped with the edge "failed erasure" rows; docs now state the rule precisely.
  - `[low]` `[patch]` (edge) A drive destroyed after a failed erasure counts as an evaluation erasure, though docs say "erased drives" — the rule is the hook's unchanged one (spec Never); `LeaseState` doc, CONTEXT glossary and ADR note now state it precisely.
  - `[low]` `[reject]` (edge) Load fault: evaluation erasures that boot stay in RAM with no new warning — story 2's sticky-fault rule; the fault is already logged and printed at start-up.
  - `[low]` `[reject]` (edge) Counts line on load fault — same as the blind 0/0 row.
  - `[low]` `[reject]` (edge) On a hung disk an unlicensed certificate is now stored up to 2 s later — same bounded delay story 2 accepted for licensed stations; only on a hung disk; reordering would race the count against certificate visibility.
  - `[low]` `[patch]` (edge) Claim "erased drives" vs failed-erasure counting — same root cause as the edge failed-erasure row.
  - `[low]` `[reject]` (edge) Claim "persisted through the same writer" not true under a load fault — the sticky-fault exception is documented in ADR-0005 item 1; the normal-path claim is accurate.
  - `[low]` `[patch]` (verification-gap, pre-verified) Destroy-path counting untested — added `http_e2e.rs` tests: a policy-`none` run of erased drives counts one evaluation erasure per drive and no licensed ones; a never-erased scanned run counts nothing.
  - `[low]` `[defer]` (verification-gap other) Licensed `record_erasure` on the destroy path untested — pre-existing; deferred.
  - `[false]` `[reject]` (intent) Surface is only the start-up line, not a route/UI (R2b) — recorded autonomous decision; the route is `licence-visibility`'s; the fix would edit the spec.
  - `[false]` `[reject]` (intent) §5 warning text not implemented (R3) — ADR-0012 / CONTEXT.md:1170 settled that the free tier withholds nothing but the marking; a limit would be a pricing decision.
  - `[false]` `[reject]` (intent) Counts licensed-but-invalid evaluation certificates too (R4b) — recorded autonomous decision; the unlicensed test meets the literal success line.
  - `[false]` `[reject]` (intent) Station-lifetime count (R5b) — recorded autonomous decision, pinned by a test.
  - `[false]` `[reject]` (intent) The `println!` in `cmd_serve` is not run by a test — the line is a one-line call of the tested pure function, the same pattern story 2's `lease store:` line uses.

## Verification

**Commands:**
- `cargo test -p wipe-server -p wipe-license -p wipe-cli` -- expected: all pass.
- `cargo clippy -p wipe-server -p wipe-license -p wipe-cli --all-targets -- -D warnings -A clippy::large_enum_variant` -- expected: clean (the allowed lint is pre-existing in `wipe-engine`).
- `cargo test --workspace` (with `docker compose up -d`) -- expected: all pass.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.

## Auto Run Result

Status: done

**Summary:** Free-tier (evaluation) erasures are now counted. `LeaseState` gains `evaluation_erasures` (`serde(default)`, so older lease files load with 0), kept apart from the licensed `erasures_used` so no `Count` quota is touched. The certificate hook records every certified Job that holds an erasure event into one count or the other according to `decision.evaluation`, off the async workers, persisted through the story-2 lease store and writer thread. The count survives restarts, licence changes and unlicensed starts. `wipestation serve` prints a `lease counts:` start-up line naming the licence the licensed count belongs to. No limit, warning, route or UI is attached (ADR-0012; `licence-visibility` owns the route).

**Files changed:**
- `crates/wipe-license/src/lease.rs` — `evaluation_erasures` field and docs.
- `crates/wipe-server/src/licensing.rs` — `record_evaluation_erasure`; `open` doc names which count restarts.
- `crates/wipe-server/src/app.rs` — hook splits counting by `decision.evaluation` under the unchanged erasure condition.
- `crates/wipe-server/src/store.rs` — operator-facing lease store details mention the evaluation count.
- `crates/wipe-cli/src/main.rs` — `lease_counts_line` printed at serve start-up; 2 unit tests.
- `crates/wipe-server/tests/lease_store.rs` (+4 tests, quota test extended with an unlicensed rebuild), `crates/wipe-server/tests/http_e2e.rs` (+2 destroy-path count tests), `crates/wipe-license/tests/attestation.rs` (+1 old-JSON test).
- `CONTEXT.md`, `README.md`, `docs/adr/0005-licensing-and-attestation.md` (item 5 free-tier half annotated resolved 2026-11-08).

**Review:** one pass, 23 findings (16 low, 7 false). Patched 9 low rows in 7 entries: counts-line licence wording, store detail wording, `open` doc, CONTEXT singular, precise counting-rule wording (failed-erasure-then-destroyed counts; never-erased does not), mixed-count restart test, destroy-path count tests. Deferred: AGENTS.md test count (agent-context file); the licensed destroy-path quota count is untested (pre-existing). Rejected: counts line on a load fault (rare; the fault is printed on the line above), load-fault RAM-only counting (story-2 sticky-fault rule), 2 s delay on a hung disk for unlicensed certificates (the bound story 2 accepted), writer claim under a load fault (documented exception). False: save-fault leakage into licensed decisions, the desktop shell being undocumented, and five intent readings settled by recorded autonomous decisions (surface, warning text, who counts, lifetime, untested `println!`).

**Follow-up review recommended:** false. The first pass patched only low entries (0 high, 0 medium).

**Verification:** `rustfmt --edition 2021 --check` on all 8 touched `.rs` files is clean. `cargo clippy -p wipe-server -p wipe-license -p wipe-cli --all-targets -- -D warnings -A clippy::large_enum_variant` is clean. `cargo test --workspace --no-fail-fast` with compose up: 852 passed, 1 failed. The failure is `wipe-hub` `health::silent_smtp_and_database_time_out_within_bound` (5.8 s against its 3.5 s bound). It failed identically when rerun alone, and also on the untouched baseline with these changes stashed, so it is environmental and predates this story (story 1 deferred it). Every new and existing licensing, server and CLI test passed.

**Residual risks:**
- The evaluation count includes licensed-but-invalid stations' evaluation certificates, by decision.
- A corrupt lease file means that boot's counts stay in RAM.
- The count is visible only at start-up.
- The desktop shell keeps it in RAM.
- The `wipe-hub` health timing test currently fails on this machine regardless of this change.
