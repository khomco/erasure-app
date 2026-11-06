---
id: SPEC-desktop-parity
companions: [../audit-2026-11-05/shipped-claims.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# The desktop app is a real station

## Why

A pain to solve. The Tauri shell builds its server with only `AppState::with_static_dir`, so a desktop user loses bench layouts on quit, has a dead identify mode, gets a new signing key every launch, cannot license, and can silently end up looking at another process's UI if port 7878 is taken. The CLI station already does all of this correctly; the two hosts drifted because station bootstrap is written twice.

## Capabilities

- **CAP-1**
  - **intent:** Both hosts build the station from one shared bootstrap, so a capability added to one is in the other.
  - **success:** The CLI `serve` path and the Tauri shell call the same function (in `wipe-server` or a crate both depend on) that wires signing key, topology store detection, licence, simulator and static dir; a test exercises it.
- **CAP-2**
  - **intent:** A desktop user's bench layout, signing key and licence persist across launches.
  - **success:** The desktop shell uses a per-user app-data directory for the store and key; relaunching shows the saved layout and the same public key id.
- **CAP-3**
  - **intent:** Identify mode works in the desktop app with the mock backend.
  - **success:** `/api/sim/devices` responds 200 in the desktop build.
- **CAP-4**
  - **intent:** The desktop window always talks to its own embedded server.
  - **success:** If the configured port is busy the shell fails visibly (or picks a free port and points the window at it); the window URL follows the actual bound address.

## Constraints

- Frontend-bundle resolution stays consistent between hosts (AGENTS.md); ideally it moves into the shared bootstrap.
- The frontend talks HTTP/WS only, never Tauri commands.
- The CLI's flags and env vars keep working unchanged.

## Non-goals

- Real hardware backend; packaging/signing the desktop app.

## Success signal

- A desktop user configures a bench, quits, relaunches, and sees the same bench and the same certificate key.
