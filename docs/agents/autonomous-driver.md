# Autonomous story driver (`scripts/bmad-driver.py`)

`bmad-build-auto` builds **one** story per invocation and never moves on by
itself. `scripts/bmad-driver.py` is the caller that walks a spec folder's
`stories.yaml` top to bottom, dispatches one non-interactive `claude -p` run per
story, and enforces the caller-only fields (`spec_checkpoint`,
`done_checkpoint`, `invoke_dev_with`; schema in
`.claude/skills/bmad-spec/assets/stories-schema.md`). Project facts and the
commit-on-finish rule it relies on are in `_bmad/custom/bmad-build-auto.toml`.

## Quick start

```sh
git switch -c feat/my-epic              # build-auto judges the branch against the epic
scripts/bmad-driver.sh .scratch/my-epic --dry-run   # plan only, never calls claude
scripts/bmad-driver.sh .scratch/my-epic             # run until done / checkpoint / halt
scripts/bmad-driver.sh status                       # queue + driver state
scripts/bmad-driver.sh approve my-epic 2 --spec     # sign off a planned spec
```

`--all` processes every `.scratch/*/stories.yaml`; if `.scratch/QUEUE` exists
(one folder per line, `#` comments) it is the authoritative order and list.
`--once` runs a single `claude` invocation and exits.

Requirements: `uv` (deps are inline, PEP 723), `git`, the `claude` CLI logged
in, and the `bmad-build-auto` skill plus `_bmad/scripts/` present in the
checkout the driver runs in.

## What it does per story

Status comes from `<folder>/stories/<id>-*.md` frontmatter (no file = not
started). `done` is skipped; `blocked`, an ambiguous match or an unknown status
stops. Anything else is dispatched as

```
claude -p "/bmad-build-auto spec folder <folder>, story id <id>. <invoke_dev_with>" \
  --output-format stream-json --verbose --permission-mode <mode> [--model <m>]
```

with the full transcript in `.scratch/.driver/logs/<ts>-<folder>-<id>.log`.
After the run the status is re-read; a non-terminal status is retried
(`--max-retries`, default 2; build-auto resumes from the status), then the
driver halts as stalled.

Before a **first** dispatch the tree must be clean (build-auto's own rule). A
resume of an existing story tolerates dirt only if it is that story's partial
work: the previous dispatch was the same story, or every dirty path is inside
the spec folder. `--allow-dirty` overrides.

## Checkpoints

| Field | Behaviour |
|---|---|
| `spec_checkpoint: true` | Until `stories/<id>.spec.approved` exists, the story is dispatched only with build-auto's `Halt after planning.` directive, so it stops at `ready-for-dev`. The driver then pauses that folder (others continue). Review/edit the story file, then `approve <folder> <id> --spec`. A story found past `ready-for-dev` without approval halts the driver. |
| `done_checkpoint: true` | After the story reaches `done`, the whole queue pauses until `stories/<id>.done.approved` exists (`approve <folder> <id> --done`). |

Markers are gitignored so they never dirty the tree.

## Usage limits

A Claude subscription limit is never a story failure and never consumes a
retry. After each run the driver inspects the CLI's own output (never tool
results, so code that mentions these phrases cannot trip it):

1. The last `rate_limit_event` in the stream-json output: `status: "rejected"`
   with `resetsAt` (epoch seconds) is a hit; a later `allowed` clears it (the
   CLI can wait out a limit in-process).
2. The `result` text, assistant messages carrying an API `error`, and the last
   plain stderr lines, matched against the CLI's wording — current
   `You've hit your 5-hour limit · resets 3pm (America/New_York)` (also weekly,
   Opus, Sonnet, `Nov 7, 3pm`, `Nov 7 at 3pm`, `Jan 2, 2027, 3pm`), legacy
   `Claude AI usage limit reached|<epoch>`, `… limit reached ∙ resets 3pm`,
   `resets in 2h 15m`, `resets 2026-08-08 00:00 UTC`, bare `rate_limit_error`.

A bare clock time means its next occurrence in the stated zone (local zone if
none). The driver sleeps until reset + `--limit-buffer-sec` (120) and
re-dispatches the **same** story. If no reset time parses, it backs off
exponentially from `--backoff-initial-sec` (300) to `--backoff-max-sec` (1800)
and probes again. Resets beyond `--max-sleep-hours` (8 days) halt instead.

To leave headroom for interactive work:

- `--window-budget N --window-hours 5`: at most N runs per rolling window.
- `--max-dispatches N`: at most N runs in this driver invocation.
- `--quiet-hours 09:00-18:00[,22:00-07:00]`: start no run in those local hours.
- `--no-wait`: exit 75 instead of sleeping (for cron/launchd); the pending
  sleep is persisted and honoured by the next invocation.

## State, safety, exit codes

`.scratch/.driver/state.json` holds the current story, per-story attempts,
`sleeping_until`, the last result/limit and the dispatch history, so a killed
driver resumes cleanly. A flock on `.scratch/.driver/lock` prevents two
drivers; a still-running `claude` child from a killed driver is detected by pid.
On halt or checkpoint it posts a macOS notification (`--no-notify` to silence).

Exit codes: `0` done (or `--once`/`--max-dispatches` reached), `10` paused at a
checkpoint, `20` blocked/stalled/invalid story, `30` precondition (dirty tree,
missing claude or skill, auth error), `73` another driver holds the lock, `75`
would sleep (`--no-wait`, or reset beyond `--max-sleep-hours`).

Configuration via flags or env: `BMAD_DRIVER_CLAUDE` (executable, used by the
tests to inject a fake), `BMAD_DRIVER_MODEL`, `BMAD_DRIVER_PERMISSION_MODE`
(default `bypassPermissions`, needed for unattended Bash/Edit; set `''` to omit
the flag), `BMAD_DRIVER_MAX_RETRIES`, `BMAD_DRIVER_WINDOW_BUDGET`,
`BMAD_DRIVER_MAX_DISPATCHES`, `BMAD_DRIVER_QUIET_HOURS`, `BMAD_DRIVER_NOTIFY=0`.
`--claude-arg` passes extra flags through (e.g. `--claude-arg "--effort high"`).

## Tests

```sh
uv run --script scripts/tests/test_bmad_driver.py
```

Never calls the real CLI: end-to-end cases drive a throwaway git repo with
`scripts/tests/fake_claude.py`, including simulated limit hits.
