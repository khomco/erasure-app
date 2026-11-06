#!/usr/bin/env bash
# Thin wrapper: run the bmad-build-auto driver under uv (PEP 723 inline deps).
# See docs/agents/autonomous-driver.md.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
uv_bin="$(command -v uv || true)"
[[ -z "$uv_bin" && -x /opt/homebrew/bin/uv ]] && uv_bin=/opt/homebrew/bin/uv
if [[ -z "$uv_bin" ]]; then
  echo "bmad-driver: uv not found (brew install uv)" >&2
  exit 30
fi
exec "$uv_bin" run --quiet --script "$here/bmad-driver.py" "$@"
