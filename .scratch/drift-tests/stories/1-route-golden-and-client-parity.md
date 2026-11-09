---
title: 'Route golden and client parity'
type: 'feature'
created: '2026-11-09'
status: 'done'
baseline_revision: '3c6cfa8d4d3112ead5b4028c84e2215bc723405d'
review_loop_iteration: 0
followup_review_recommended: false
context: []
warnings: []
deferred:
  - summary: >-
      AGENTS.md test count reads 926; the workspace now has 929 tests.
    evidence: |-
      cargo test --workspace --no-fail-fast on 2026-11-09 summed 928 passed, 1 failed; the 926 figure predates the three matrix tests added in step-03 verify. Fix edits an agent-context file, so deferred per triage rules.
    location: >-
      AGENTS.md:25
    severity: low
  - summary: >-
      AGENTS.md Conventions bullet on new routes still names only app.rs and client.ts; it should say routes go in api_routes() and that the parity test enforces the client pairing.
    evidence: |-
      The "TypeScript domain types ... new routes with crates/wipe-server/src/app.rs and src/api/client.ts" bullet is unchanged by this diff; the Running-and-verifying bullet added here covers the mechanism. Agent-context file edit, so deferred.
    location: >-
      AGENTS.md Conventions that differ from defaults
    severity: low
---

<intent-contract>

## Intent

**Problem:** Nothing pins the station's HTTP routes: 91a0c98 fixed `POST /api/bay-topology/resolve`, which the desktop client called but the router never registered, and the gap passed every test. A removed or unrouted endpoint, or a client function for a route that does not exist, still ships silently.

**Approach:** Enumerate the `wipe-server` router's API routes (method + path) from the router itself into a committed golden, `docs/generated/http-routes.txt`, and fail `cargo test` on any difference; add a parity check that fails when a routed (method, path) has no call in the desktop client or the client calls one that is not routed. `UPDATE_GOLDENS=1` rewrites goldens (CAP-5).

## Boundaries & Constraints

**Always:** Route list comes from the code that builds the router, never a hand-kept list in the test; methods come from probing the built router, not from parsing Rust source. Golden is deterministic (sorted, one `METHOD /path` per line, `HEAD` implied by `GET` omitted). `UPDATE_GOLDENS=1` rewrites only goldens — the parity check still fails under it. A missing golden fails the test with the regeneration command in the message. Router behaviour (paths, methods, the JSON 404 catch-all, the static fallback) is unchanged.

**Never:** No Astro, `sites/`, xtask or CI config (D1). No new crate dependencies (no `regex`). Never port routing or matching logic to TypeScript. The hub's (`wipe-hub`) router is out of scope. Do not change `client.ts` or `ws.ts` behaviour.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| In sync | Router, golden, client agree | Both tests pass | — |
| Route deleted | A `.route(...)` line removed | Golden test fails naming the missing line | Panic message shows `UPDATE_GOLDENS=1 cargo test -p wipe-server --test route_golden` |
| Route added, no client | New `.route` with golden regenerated | Parity test fails naming `METHOD /path` with no client call | — |
| Client-only call | `client.ts` calls an unrouted path | Parity test fails naming it | — |
| Method added to a path | `.post(...)` chained onto an existing GET | Golden diff shows the new line | — |
| Regenerate | `UPDATE_GOLDENS=1` | Golden written (dirs created), golden test passes | Parity still enforced |
| Param naming | `:id` in Rust, `${id}` / `${encodeURIComponent(id)}` in TS | Treated as the same segment | — |
| Comments | `// ... "/api/x"` in client.ts | Ignored | — |

## Autonomous decisions

- **Which file counts as the client for the live-events WebSocket.** The desktop UI reaches every station route through `client.ts` except the live job feed (`GET /api/events`), which `ws.ts` opens as a WebSocket. Options: (a) exempt it in the test by name; (b) read `ws.ts` as a second client file; (c) add a dummy `client.ts` function. Chose (b): AGENTS.md already pairs the WebSocket with `ws.ts`, it needs no hand-kept exemption list, and (c) adds dead code.
- **Which router.** The intent cites 91a0c98, a station (`wipe-server`) route. The hub (`wipe-hub`) serves the portal and website, not `client.ts`. Chose the station router only; the hub is a later story if wanted.
- **Golden format and location.** `docs/generated/http-routes.txt`, sorted plain text, one `METHOD /path` per line, so it diffs cleanly and is a ready input for W2.2's site later.

</intent-contract>

## Code Map

- `crates/wipe-server/src/app.rs:455` -- `router()`: the `.route(...)` chain then `.route("/api/*rest", any(unknown_api_route))` catch-all (exclude it from the golden), static/landing fallback merged after. axum 0.7, params use `:id`.
- `crates/wipe-server/src/lib.rs` -- re-exports `router`, `AppState`; export the new path-enumeration fn here.
- `crates/wipe-server/tests/http_e2e.rs:13` -- how tests build a state: `MockBackend::with_catalog(default_devices_public(), MockTiming::fast())`, `AppState::new(backend, None, Arc::new(SigningKey::generate()))`.
- `apps/desktop/src/api/client.ts:140` -- `api` object: each fn calls `jsonFetch<T>("/api/..." | \`/api/${...}\`, { method: "POST" ... })`; default GET. One call (`previewCompletion`) has the path literal on the line after `(`. Covers every route except `/api/events`.
- `apps/desktop/src/api/ws.ts:35` -- the only client of `GET /api/events` (WebSocket), built as `` `${proto}//${window.location.host}/api/events` ``.
- `Cargo.toml:55` -- `tower` with `util` (gives `ServiceExt::oneshot`) is already a `wipe-server` dependency.
- `AGENTS.md` "Running and verifying" -- document `UPDATE_GOLDENS=1`; `docs/ROADMAP.md:222` W2.2 -- note the route test landed.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-server/src/app.rs` -- move the API `.route(...)` chain (minus the catch-all) into a small recording builder whose `.route(path, method_router)` both registers on the `Router` and records `path`; `router()` uses it then adds the catch-all; add `pub fn api_route_paths() -> Vec<&'static str>` -- single source for paths, keeps `.route(...)` lines as the edit surface.
- `crates/wipe-server/src/lib.rs` -- re-export `api_route_paths`.
- `crates/wipe-server/tests/route_golden.rs` -- new: (1) for each path, substitute params with a nil UUID, send a `TRACE` via `oneshot` to `router(state)`, assert 405, and read methods from its `Allow` header (drop `HEAD`); render sorted lines under a one-line `#` header naming the regen command; compare with `docs/generated/http-routes.txt` via an `assert_golden` helper honouring `UPDATE_GOLDENS=1`. (2) Parity: lex `client.ts` and `ws.ts` skipping `//` and `/* */` comments, collect string/template literals containing `/api/` (path = from `/api/` to end of literal), method from a `method: "X"` inside the same call (paren-depth scan after the literal; ws.ts literal = GET), normalise `:name` and `${...}` segments to `{}`, and assert set equality both ways with a message listing each side's extras. Unit-test the lexer on the matrix's param and comment rows.
- `docs/generated/http-routes.txt` -- generated with `UPDATE_GOLDENS=1`, committed.
- `AGENTS.md`, `docs/ROADMAP.md` -- document regeneration; annotate W2.2 that the route golden + client parity test exist (site generation still open). Update the AGENTS.md test count.

**Acceptance Criteria:**
- Given the committed tree, when `cargo test --workspace` runs, then the route golden and parity tests pass.
- Given the `.route("/api/bay-topology/resolve", ...)` line deleted, when `cargo test -p wipe-server` runs, then it fails.
- Given a new routed path with no `client.ts` call (golden regenerated), when `cargo test -p wipe-server` runs, then the parity test fails naming it.
- Given `UPDATE_GOLDENS=1 cargo test -p wipe-server --test route_golden`, when run on a changed router, then the golden file is rewritten and a plain re-run passes.

## Spec Change Log

## Review Triage Log

### 2026-11-09 — Review pass
- verdicts: 30 findings — high 0, medium 3, low 21, false 6, maybe-false 0
- findings:
  - `[medium]` `[patch]` (blind) A route added in `router()` outside `api_routes()` escapes golden and parity — added a comment at the `api_routes().router` call site in `router()` saying every route goes in `api_routes()`.
  - `[low]` `[reject]` (blind) Parity reads only client.ts/ws.ts; a fetch elsewhere would be missed — verification layer searched `apps/desktop/src`: no other `/api/` calls exist; adding a repo-wide scan is new guard logic for an unseen case.
  - `[low]` `[patch]` (blind) Query strings / suffix templates break normalisation — `normalise` now cuts at `?`/`#`; unit cases added.
  - `[low]` `[reject]` (blind) Non-literal `method:` silently defaults to GET — fails loudly (both directions listed); a resolver branch for an unused shape is not worth it.
  - `[low]` `[reject]` (blind) ws.ts literals forced to GET — ws.ts has only the WebSocket URL; a future fetch there fails loudly.
  - `[false]` `[reject]` (blind) No allowlist for server-only routes — CAP-1 requires every route to have a client call; a server-only route is a decision for its own story.
  - `[low]` `[patch]` (blind) CRLF checkout fails with empty diff — `check_golden` normalises `\r\n`; CRLF assertion added.
  - `[low]` `[reject]` (blind) Lexer mishandles regex literals — neither file has one; the failure would be loud (routes reported uncalled).
  - `[low]` `[defer]` (blind) AGENTS.md test count 926 unexplained — actual is 929; agent-context edit, deferred.
  - `[low]` `[reject]` (blind) Scratch dirs leak when a self-test fails — only on a failing test, in the OS temp dir.
  - `[low]` `[reject]` (blind) Routes probed twice per run — 37 in-process requests, ~10 ms.
  - `[low]` `[defer]` (blind) AGENTS.md Conventions route bullet not updated — agent-context edit, deferred.
  - `[low]` `[patch]` (edge) Query string in client literal — same fix as the blind query-string row.
  - `[low]` `[reject]` (edge) Path built by `+` concatenation — no such call; fails loudly.
  - `[low]` `[reject]` (edge) `/api/` literal nested inside `${...}` dropped — no such call; route would be reported uncalled (loud).
  - `[false]` `[reject]` (edge) Body field `method: "Shred"` read as HTTP verb — bodies are passed as `JSON.stringify(var)`, and an inlined object would sit at depth ≥2, which `call_method` ignores (depth == 1 only).
  - `[low]` `[reject]` (edge) Non-literal method defaults to GET — same as blind row; loud.
  - `[low]` `[reject]` (edge) Non-call `/api/` literal counts as a call — none exist outside calls (comments are stripped); a call-site guard adds parsing for an unseen case.
  - `[low]` `[reject]` (edge) Regex literal phantom string — same as blind row.
  - `[low]` `[patch]` (edge) Wildcard `*rest` segment not normalised — `normalise` maps `*` segments to `{}`; unit case added.
  - `[low]` `[patch]` (edge) Explicit HEAD-only route panics — HEAD now dropped only when GET is also allowed.
  - `[medium]` `[patch]` (edge) Route added outside builder — grouped with the blind row; same comment.
  - `[low]` `[patch]` (edge) CRLF golden — grouped with the blind row; same fix.
  - `[low]` `[reject]` (edge) Scratch-dir leak — same as blind row.
  - `[medium]` `[patch]` (verification-gap, other) Routes registered outside `api_routes()` are invisible — grouped with the blind row; same comment. No verification gaps filed.
  - `[false]` `[reject]` (intent) Failure modes only tested on synthetic data — the success signal was checked on the real tree: deleting the real `/api/bay-topology/resolve` line failed both `route_golden_matches` and `client_calls_match_routes`.
  - `[low]` `[reject]` (intent) `UPDATE_GOLDENS` env read itself never exercised by a test — a one-line read; setting env vars in parallel tests is racy; the regen path was run by hand and `check_golden(update=true)` is tested.
  - `[false]` `[reject]` (intent) Routes outside builder not enumerated — grouped observation; patched by comment above (counted there).
  - `[false]` `[reject]` (intent) Unrouted-and-uncalled handler not detected — the SPEC's success criteria are golden + client parity; handler enumeration is not asked for, and 91a0c98's shape (client calls unrouted path) is caught.
  - `[false]` `[reject]` (intent) Any `/api/` literal counts rather than an `api.*` function; ws.ts counted; hub excluded; per-file golden helper — all recorded readings: the call-level check and ws.ts are the story's autonomous decision, hub scope likewise, and story 1 owns only its own golden.

## Design Notes

axum's `Router` cannot list its routes, hence the recording builder. Methods are probed rather than recorded so `.get(..).put(..)` chains need no duplicate annotation: axum 0.7 answers a matched path with an unhandled method with 405 and an `Allow` header. Golden example:

```
# Generated by UPDATE_GOLDENS=1 cargo test -p wipe-server --test route_golden — do not edit.
GET /api/bay-topology
PUT /api/bay-topology
```

## Verification

**Commands:**
- `cargo test -p wipe-server --test route_golden` -- expected: pass
- `cargo test --workspace` (compose up) -- expected: all pass
- `cargo clippy -p wipe-server --all-targets` -- expected: no new warnings

## Auto Run Result

Status: done

**Summary.** The station's API routes are pinned in `docs/generated/http-routes.txt` (37 `METHOD /path` lines). Paths come from a recording builder that the router itself is built from, and methods come from probing the built router. A parity test fails when a routed method+path has no call in the desktop `client.ts`/`ws.ts`, or the client calls an unrouted one. `UPDATE_GOLDENS=1` rewrites the golden (CAP-5).

**Files.**
- `crates/wipe-server/src/app.rs` -- `ApiRoutes` recording builder, `api_routes()` holds the `.route(...)` chain, `pub fn api_route_paths()`; `router()` behaviour unchanged.
- `crates/wipe-server/tests/route_golden.rs` -- golden test, client parity test, small TS lexer, 6 unit tests covering every matrix row.
- `docs/generated/http-routes.txt` -- the committed golden.
- `AGENTS.md` -- regeneration documented; test count updated.
- `docs/ROADMAP.md` -- W2.2 annotated as partial.

**Review.** 30 findings: 7 patch rows (4 fixes: router-site comment, `?`/`#`/`*` normalisation, CRLF-tolerant compare, HEAD only dropped beside GET), 2 deferred (AGENTS.md wording/count, agent-context edits), 21 rejected with reasons in the triage log. Patched by verdict: medium 1 entry, low 3 entries. Follow-up review: not recommended (no high patched, one medium).

**Verification.**
- `cargo test -p wipe-server --test route_golden`: 8 passed. `cargo test -p wipe-server` after patches: all passed.
- Mutation check: deleting the real `/api/bay-topology/resolve` `.route` line failed `route_golden_matches` and `client_calls_match_routes`, naming the route; restored.
- `cargo test --workspace --no-fail-fast` (compose up, before the review patches, which touch only `wipe-server`): 928 passed, 1 failed -- `wipe-hub` `health::silent_smtp_and_database_time_out_within_bound` (took ~9.6 s), which fails alone too, also on the baseline per the implementer, and `wipe-hub` does not depend on `wipe-server`.
- `cargo clippy -p wipe-server --all-targets`: only the two existing large-enum-variant warnings.

**Residual risks.** Routes registered outside `api_routes()` are invisible to the tests (guarded by a comment only). The lexer handles no regex literals or concatenated paths; both would fail loudly. AGENTS.md test count (926 → 929) and its ~30 s timing claim (the run took >10 min here) are stale.
