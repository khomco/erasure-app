# Audit 2026-11-05 — shipped claims vs code

Read-only audit of `main` at c9a9214, after all feature branches merged.
Tests at the time: `cargo test --workspace` 136 passed, `pnpm test` 200 passed. Line
numbers are as of c9a9214. Items marked *unverified* were found by reading code, not by
running it.

## (a) Shipped claims (CONTEXT §10.2 / §11)

| Claim | Verdict | Evidence |
|---|---|---|
| Outer-Job composition | confirmed, with gaps | `job.rs:201-229`, `types.ts:86-94`. No retry: `retry_erasure` cited at `runner.rs:125,295` does not exist. `Quarantined` never entered. `station_id` always `None` (`erasure_event.rs:148`, `runner.rs:540`, `handlers.rs:300`) |
| Destroy path, "second independent signature" | **overstated** | Co-sign uses the station's own key (`handlers.rs:421-427`); `signer`/`role`/`manifest_ref` sit outside the signed bytes (`sign.rs:150-173`); `verify-cert` never checks co-signatures (`main.rs:571`); `manifest_ref`/`manifest_id` never stamped (`handlers.rs:376-385`, `job.rs:231`) |
| Cert schema v2, "v1 certs remain valid" | partial | `schema.rs:46-55`. The v1 claim is false (ADR-0001 addendum says so; §10.2 did not) |
| REST destroy/manifest routes | confirmed | `app.rs:325-335` ↔ `client.ts:116-134` |
| Timeline + Manifests page | confirmed, one bug | `ManifestsPage.tsx:24` filters on `!j.manifest_id`, which is never set — same Job could join two manifests (server-side guard added in c779361; the UI filter is still wrong) |
| Bench-status overlay | confirmed | `DevicesPage.tsx:413,465,531`; `slotStatus.ts` untested |
| Bay topology + SVG | confirmed | `bay.rs`, `BayMap.tsx`, `bay_topology.rs` (36 tests) |
| Tiered config store | partial | Tier 2 always fails: `ControlPlaneStore::probe` returns `Err` (`store.rs:210-214`). Tauri skips detection |
| Bench-setup builder + identify | CLI only | Unusable in the desktop app (see c). Stale copy at `BenchSetupPage.tsx:1216` ("Learn by hot-swap … land next") |
| Enclosure catalog + shells | partial | `with_catalog_overlay` (`app.rs:146`) wired by no host; no flag or overlay dir |
| Licensing by attestation | partial | Chain and in-payload `evaluation` real (`schema.rs:79`, `chain.rs:236-283`). `LeaseState` never persisted (`main.rs:752`, `licensing.rs:36`); `FileCounter` missing; `AllowedMethods::permits` and `feature_available` uncalled; no API route or UI |
| Diagnostic/HealthCheck schema-only | confirmed | `job.rs:202-203`, never constructed; `attributes` is `serde_json::Value` (`job.rs:141`) |

## (b) ADR conformance deviations

- **ADR-0001** — one ErasureEvent per Job (`runner.rs:296-356`); no `Quarantined` transition; no station id on events; the PendingCoSign cert is signed with a `Destroyed` disposition before any supervisor acts (`app.rs:239-245`).
- **ADR-0002** — Consequences still say "No persistence layer is added" (superseded by ADR-0003, not annotated). TS re-implements numbering/model expansion and validation (`topologyEdit.ts:58,163,235`) with no parity test. UI templates (`BenchSetupPage.tsx:476-497`) differ from Rust presets (`bay.rs:778-780`).
- **ADR-0003** — control-plane tier is a stub (`store.rs:210`), yet the tier-3 banner advises `--control-plane-url`. Corrupt stored config only logged (`app.rs:131-135`); `StoreStatus` has no error field. `LocalFileStore::load` skips the schema_version and duplicate-id checks the `--bay-topology` path gets (`store.rs:135` vs `main.rs:262-281`). First save from an unconfigured station skips the revision check (`app.rs:177-185`). Default store is ephemeral with `awaiting_decision=false` (`app.rs:92-95`).
- **ADR-0004** — §2 has `art` as an svg_template object; code has `art: Option<String>` (`catalog.rs:182`). No per-entry `schema_version`. Overlay unwired and does not fail closed on schema version (`catalog.rs:307-319`).
- **ADR-0005** — never-gates-erasure **conforms**; in-payload `evaluation` **conforms**. `HttpLicenseClient` always `Unreachable` (`client.rs:82-95`), never constructed. Watermark resets on every start (`main.rs:752`) — restart defeats rollback detection and `Count` quota; `lease.rs:21` claims persistence falsely. Root-only verification is not possible: the licence carries only `instance_public_key_id` (`chain.rs:45`, `sign.rs:53-56`) and `verify-cert` requires `--public-key-b64` (`main.rs:103`). `allowed_methods` unenforced. Free-tier count not tracked (`app.rs:272`). Machine fingerprint is `hash(station_id)` and `station_id` defaults to `standalone` (`main.rs:431`), so unflagged stations share a fingerprint. Destroyed Jobs consume quota (`app.rs:272-274`).

## (c) CLI vs Tauri parity

`apps/desktop/src-tauri/src/lib.rs:143` calls only `AppState::with_static_dir`: no topology store, licence, simulator (neither host wires the catalog overlay). Also: a fresh signing key every launch (`lib.rs:122`), random station id, slow mock timing, no CLI env vars honoured, `api_url_cmd` unused (`lib.rs:76`), window URL hard-coded to `http://127.0.0.1:7878` (`tauri.conf.json:14`).

Desktop user experience (*unverified*, code-read): bench setup warns "Configuration is not saved" and loses layouts on quit; identify mode is dead (`/api/sim/devices` 404); Devices always shows the unconfigured bench; every cert is evaluation with no way to license and no UI saying so; the cert key changes every launch; if port 7878 is taken the bind failure is only logged and the window shows the other process's UI.

## (d) Frontend exposure gaps

Every route has a `client.ts` function and vice versa. No licence or evaluation surface anywhere; TS `SignedCertificate` lacks `certificate.evaluation` and `attestation` (`types.ts:321-370`). TS `EnclosureModel` lacks `identity` (harmless today). Bay/store/resolved-topology and WS tags match. The UI never states that drive evidence is simulated, outside the Identify panel.

## (e) Doc corrections

Tracked as `.scratch/docs-truth-up/`. Status-critical fixes (CONTEXT §10.2, ROADMAP §0.4/T0, AGENTS.md counts and line refs) were made in the same commit as this audit.

## (f) Risks

1. **Uncertified destruction** — escalating a Job with no erasure activity gave a PendingCoSign Job with no cert, and co-sign marked it Destroyed anyway. **Reproduced and fixed (refusal) in c779361**; how to certify such a Job is open (D9).
2. A signed cert claims `Destroyed` before any supervisor action (`app.rs:239-245`), servable indefinitely if the manifest is never co-signed (D9).
3. Duplicate manifests / half-applied co-sign — **fixed in c779361** (one live manifest per Job; whole-manifest validation before mutation).
4. Licence state resets on restart (`.scratch/licensing-as-built`).
5. Desktop app misleads (`.scratch/desktop-parity`); stale "lands next" copy and impossible `--control-plane-url` advice.
6. Minor: `wipe-server` has an unused production dependency on `wipe-engine-mock` and lists `wipe-license` twice (`Cargo.toml:10,12,31`); runner "not found" maps to 500 (`handlers.rs:446`); `TpmCounter::is_hardware_backed()` returns `true` while doing nothing (`lease.rs:91`).
