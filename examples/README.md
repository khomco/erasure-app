# Example configuration

Hand-written config files, useful as starting points and as worked examples of
what the schema supports. Not compiled into the binary — the built-in presets
(`wipestation bay-presets`) are separate.

| File | What it shows |
| --- | --- |
| `bay-topology-mixed-bench.json` | One bench composing three enclosures at once — a 24-bay rackmount (4x6, row-major), a 2-bay dock, and an 8-socket NVMe carrier (4x2, column-major) — plus a per-bay form-factor override (a 2.5" sled in bay 3 of a 3.5" bank) and a blanked-off bay (bay 6). |

```bash
wipestation serve --fast --bay-topology examples/bay-topology-mixed-bench.json
```
