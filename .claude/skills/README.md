# Wipestation Agent Skills

Skills installed from [mattpocock/skills](https://github.com/mattpocock/skills),
selected for this project's workflow. Invoke with `/skill-name` or by asking
Claude to use the skill by name.

## Workflow map — when to reach for each

### Before you code

| Skill | Use when |
| --- | --- |
| [`/grill-me`](grill-me/SKILL.md) | New idea or feature, need to nail down what you actually want before writing anything. Walks the entire decision tree. |
| [`/grill-with-docs`](grill-with-docs/SKILL.md) | Same as grill-me, but for changes to the **existing** codebase — references domain language, ADRs, and updates docs as decisions land. **Preferred once `CONTEXT.md` + `docs/adr/` exist.** |
| [`/zoom-out`](zoom-out/SKILL.md) | You're staring at unfamiliar code and need broader context. |
| [`/prototype`](prototype/SKILL.md) | Want to spike a design before committing. Routes to terminal app (logic) or runnable UI (interactions). |

### Turning ideas into work

| Skill | Use when |
| --- | --- |
| [`/to-prd`](to-prd/SKILL.md) | Conversation has crystallized into a feature — capture it as a formal PRD in the issue tracker. |
| [`/to-issues`](to-issues/SKILL.md) | Break a PRD or spec into independently-grabbable, vertically-sliced issues. |
| [`/triage`](triage/SKILL.md) | New bugs / FRs landing — categorize, prioritize, route. |

### While building

| Skill | Use when |
| --- | --- |
| [`/tdd`](tdd/SKILL.md) | Building a feature or fixing a bug. Strict red-green-refactor with vertical slices (one test → one impl → repeat). Discourages writing all tests up front. |
| [`/diagnose`](diagnose/SKILL.md) | Hard bug or perf regression. Reproduce → minimize → hypothesize → instrument → fix → regression-test. |
| [`/improve-codebase-architecture`](improve-codebase-architecture/SKILL.md) | Look for structural deepening opportunities. Reads `CONTEXT.md` + ADRs first so the suggestions are domain-aligned. |

### Setup & meta

| Skill | Use when |
| --- | --- |
| [`/setup-matt-pocock-skills`](setup-matt-pocock-skills/SKILL.md) | **Run this first.** Configures `AGENTS.md`/`CLAUDE.md` + `docs/agents/` so the engineering skills know our issue tracker (GitHub vs local files), our triage labels, and where docs live. |
| [`/setup-pre-commit`](setup-pre-commit/SKILL.md) | When we're ready to enforce lint/typecheck/test on every commit. |
| [`/git-guardrails-claude-code`](git-guardrails-claude-code/SKILL.md) | Install hooks to block destructive git ops (`reset --hard`, `clean -f`, `branch -D`, force-push to main, etc.). |
| [`/write-a-skill`](write-a-skill/SKILL.md) | If we identify a project-specific workflow worth turning into a reusable skill (e.g. `/run-mock-erase`, `/verify-cert-offline`). |

## How they compose

A typical flow on this project looks like:

```
/grill-with-docs   ← align on the change against CONTEXT.md + ADRs
   ↓
/to-prd            ← capture the agreed design as a PRD issue
   ↓
/to-issues         ← split the PRD into tracer-bullet issues
   ↓
/tdd               ← work each issue red-green-refactor
   ↓
/improve-codebase-architecture   ← periodic sweep for structural drift
```

For unknowns and bug work, swap `/grill-with-docs` for `/diagnose`.
For unfamiliar code, prepend `/zoom-out`.

## Recommended first action

Run **`/setup-matt-pocock-skills`** so the other skills know:

- We use **GitHub** for issues (or local files — your call when prompted)
- Where to write `CONTEXT.md` and `docs/adr/`
- What labels we use for `/triage`

This is a one-time setup that makes the others smarter.

## Skills installed

| Skill | Category | Purpose |
| --- | --- | --- |
| `diagnose` | engineering | Disciplined bug/perf diagnosis loop |
| `grill-with-docs` | engineering | Stress-test plan against existing docs |
| `improve-codebase-architecture` | engineering | Domain-aligned structural deepening |
| `prototype` | engineering | Throwaway design spike |
| `setup-matt-pocock-skills` | engineering | One-time configuration for other skills |
| `tdd` | engineering | Vertical-slice TDD with red-green-refactor |
| `to-issues` | engineering | PRD → independently-grabbable issues |
| `to-prd` | engineering | Conversation → PRD on issue tracker |
| `triage` | engineering | State-machine-driven issue categorization |
| `zoom-out` | engineering | Broader context / higher-level view |
| `grill-me` | productivity | General interview for new ideas |
| `write-a-skill` | productivity | Author new project-specific skills |
| `git-guardrails-claude-code` | misc | Block destructive git commands |
| `setup-pre-commit` | misc | Husky + lint-staged + typecheck + tests |

## Skills deliberately not installed

| Skill | Why skipped |
| --- | --- |
| `caveman` | Compressed-output mode; not relevant for this project's pace |
| `handoff` | Useful for long sessions; install if we start chaining them |
| `migrate-to-shoehorn` | TS test-migration tool; we don't use shoehorn |
| `scaffold-exercises` | Educational content; not applicable |
| `deprecated/*` | Superseded by current versions |
| `personal/*`, `in-progress/*` | Author's personal workflow / WIP |

## Source

These skills are vendored from [mattpocock/skills](https://github.com/mattpocock/skills)
at clone time. To update, re-run the install: clone the source repo and copy
the directories listed above into `.claude/skills/`. Skills are MIT-style
permissive in spirit (the source repo doesn't specify a license per skill;
treat as personal use unless we clarify with the author before redistributing).
