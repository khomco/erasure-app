# Issue tracker: Local Markdown

Issues and PRDs for this repo live as markdown files in `.scratch/`.

## Conventions

- One feature per directory: `.scratch/<feature-slug>/`
- The PRD is `.scratch/<feature-slug>/PRD.md`
- Implementation issues are `.scratch/<feature-slug>/issues/<NN>-<slug>.md`, numbered from `01`
- Triage state is recorded as a `Status:` line near the top of each issue file (see `triage-labels.md` for the role strings)
- Comments and conversation history append to the bottom of the file under a `## Comments` heading

## BMAD spec folders (unattended work)

Work meant for the unattended loop uses the BMAD layout in the same place:
`.scratch/<feature-slug>/SPEC.md` + `stories.yaml`, with each story's plan and result written by
`bmad-build-auto` to `stories/<id>-<slug>.md` (its `status:` frontmatter is the triage state).
`.scratch/QUEUE` orders the folders for `scripts/bmad-driver.py` (see `autonomous-driver.md`).
`.scratch/STATUS.md` is the current where-we-are; `.scratch/DECISIONS.md` lists what blocks the rest.

## When a skill says "publish to the issue tracker"

Create a new file under `.scratch/<feature-slug>/` (creating the directory if needed).

## When a skill says "fetch the relevant ticket"

Read the file at the referenced path. The user will normally pass the path or the issue number directly.
