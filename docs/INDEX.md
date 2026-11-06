# Docs index

Pointers only — each document below owns its content; nothing here is a copy.

- **[`../CONTEXT.md`](../CONTEXT.md)** — product context and the domain glossary (§14). Read this
  first. Lives at the repo root by convention (`agents/domain.md`), which is why this index
  exists: tools that scan `docs/` would otherwise miss it.
- **[`adr/`](adr/)** — Architecture Decision Records, the canonical record of load-bearing
  decisions. Numbered `ADR-000n`. Contradicting one is allowed; doing it silently is not.
- **[`ARCHITECTURE.md`](ARCHITECTURE.md)** — how the crates layer, the trait seams, job
  lifecycle, certificate flow.
- **[`ROADMAP.md`](ROADMAP.md)** — product surfaces not yet built, with stable W/A/O/L item
  numbers. Shipped items are struck here and recorded in `../CONTEXT.md` §10.
- **[`design/`](design/)** — per-feature design notes (bench setup builder, enclosure art
  pipeline).
- **[`agents/`](agents/)** — conventions for AI agents working this repo. The always-loaded
  rules are in `../AGENTS.md`; these files hold the detail behind them.
