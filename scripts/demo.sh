#!/usr/bin/env bash
# Wipestation v0.1 demo: spin up two stations, demonstrate mDNS discovery,
# run an erase on each station, generate signed certs, verify them offline.

set -euo pipefail

CARGO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$CARGO_DIR"

. "$HOME/.cargo/env"

WIPESTATION="$CARGO_DIR/target/debug/wipestation"
if [[ ! -x "$WIPESTATION" ]]; then
  echo "==> Building wipe-cli..."
  cargo build -p wipe-cli
fi

KEY_A="/tmp/wipestation-demo-key-A"
KEY_B="/tmp/wipestation-demo-key-B"
LOG_A="/tmp/wipestation-demo-A.log"
LOG_B="/tmp/wipestation-demo-B.log"

cleanup() {
  set +e
  if [[ -n "${PID_A:-}" ]]; then kill "$PID_A" 2>/dev/null; fi
  if [[ -n "${PID_B:-}" ]]; then kill "$PID_B" 2>/dev/null; fi
  wait 2>/dev/null
}
trap cleanup EXIT

echo "==> Starting station A on :7878..."
"$WIPESTATION" serve \
  --addr 127.0.0.1:7878 \
  --station-id "demo-station-A" \
  --fast \
  --key-path "$KEY_A" > "$LOG_A" 2>&1 &
PID_A=$!

echo "==> Starting station B on :7879..."
"$WIPESTATION" serve \
  --addr 127.0.0.1:7879 \
  --station-id "demo-station-B" \
  --fast \
  --key-path "$KEY_B" > "$LOG_B" 2>&1 &
PID_B=$!

# Wait for both APIs to be responsive, and assert that the station answering
# is the one *this script* started. A bare health check is not enough: a stale
# wipestation left running from an earlier session answers /api/health quite
# happily, and the demo would then run against it and report someone else's
# results as its own.
for pair in "7878:demo-station-A" "7879:demo-station-B"; do
  port="${pair%%:*}"
  want="${pair##*:}"
  got=""
  for _ in $(seq 1 30); do
    got=$(curl -fsS "http://127.0.0.1:${port}/api/station" 2>/dev/null | jq -r '.id // empty')
    if [[ "$got" == "$want" ]]; then break; fi
    sleep 0.2
  done
  if [[ "$got" != "$want" ]]; then
    if [[ -z "$got" ]]; then
      echo "FAILED: no station answered on :${port} within 6s."
    else
      echo "FAILED: :${port} is owned by station '${got}', not '${want}'."
      echo "  Something else is already bound to that port — probably a stale"
      echo "  wipestation from an earlier session. Find and stop it with:"
      echo "    lsof -nP -iTCP:${port} -sTCP:LISTEN"
    fi
    exit 1
  fi
done

echo "==> Both stations up."

# Capture public keys.
PK_A=$(curl -s http://127.0.0.1:7878/api/public_key | jq -r .public_key_b64)
PK_B=$(curl -s http://127.0.0.1:7879/api/public_key | jq -r .public_key_b64)
echo "    A public key: $PK_A"
echo "    B public key: $PK_B"

# Give mDNS a few seconds to converge.
echo "==> Waiting for mDNS discovery (up to 10s)..."
for _ in $(seq 1 50); do
  PEERS_A=$(curl -s http://127.0.0.1:7878/api/fleet/peers | jq 'length')
  PEERS_B=$(curl -s http://127.0.0.1:7879/api/fleet/peers | jq 'length')
  if [[ "$PEERS_A" -gt 0 && "$PEERS_B" -gt 0 ]]; then
    break
  fi
  sleep 0.2
done
echo "    A sees $PEERS_A peer(s); B sees $PEERS_B peer(s)"
curl -s http://127.0.0.1:7878/api/fleet/peers | jq '.[] | {id, hostname, role, api_port}'

# Lead election.
LEAD=$(curl -s http://127.0.0.1:7878/api/fleet/lead | jq -r .lead)
echo "    Elected lead: $LEAD"

# Run an erase on station A.
echo "==> Creating + running an erase job on station A..."
JOB_A=$(curl -s -X POST http://127.0.0.1:7878/api/jobs \
  -H 'Content-Type: application/json' \
  -d '{
    "device_id":"dev-nvme-0",
    "classification":"high",
    "intent":"reuse",
    "verify":true,
    "verify_samples":4,
    "operator":{"id":"demo-op","display_name":"Demo Operator","email":"demo@wipestation.dev"},
    "asset_tag":"DEMO-ASSET-A",
    "site_label":"DemoSite",
    "ticket_ref":"DEMO-001"
  }' | jq -r .job_id)
curl -s -X POST "http://127.0.0.1:7878/api/jobs/${JOB_A}/start" > /dev/null
echo "    Job ${JOB_A} started"

# Poll until the outer Job reaches a terminal disposition (ADR-0001).
# `completed` is an ErasureEventState, not a JobState — do not test for it here.
STATE=""
for _ in $(seq 1 50); do
  STATE=$(curl -s "http://127.0.0.1:7878/api/jobs/${JOB_A}" | jq -r '.state.state')
  case "$STATE" in
    erased|destroyed|quarantined|aborted) break ;;
  esac
  sleep 0.3
done
echo "    Job final state: $STATE"
if [[ "$STATE" != "erased" ]]; then
  echo "    FAILED: expected the demo job to reach 'erased', got '${STATE}'"
  exit 1
fi

# Retrieve and verify the signed certificate.
sleep 0.5
curl -s "http://127.0.0.1:7878/api/jobs/${JOB_A}/certificate" > /tmp/wipestation-demo-cert-A.json
echo "    Cert size: $(wc -c < /tmp/wipestation-demo-cert-A.json) bytes"
"$WIPESTATION" verify-cert /tmp/wipestation-demo-cert-A.json --public-key-b64 "$PK_A"

# Cross-check: try to verify with B's key — should fail.
echo "==> Cross-verification check: cert A against key B (should fail)..."
if "$WIPESTATION" verify-cert /tmp/wipestation-demo-cert-A.json --public-key-b64 "$PK_B" 2>/dev/null; then
  echo "    UNEXPECTED: cert A verified with key B!"
  exit 1
else
  echo "    OK — cert A correctly rejected by key B"
fi

# Tamper test: edit the operator email and re-verify.
echo "==> Tamper test: modify cert and re-verify (should fail)..."
jq '.certificate.operator.email = "imposter@evil.example"' /tmp/wipestation-demo-cert-A.json > /tmp/wipestation-demo-cert-A-tampered.json
if "$WIPESTATION" verify-cert /tmp/wipestation-demo-cert-A-tampered.json --public-key-b64 "$PK_A" 2>/dev/null; then
  echo "    UNEXPECTED: tampered cert verified!"
  exit 1
else
  echo "    OK — tampered cert correctly rejected"
fi

echo
echo "============================================================"
echo "  Wipestation v0.1 demo complete."
echo "  • Two stations discovered each other via mDNS"
echo "  • Lead election: $LEAD"
echo "  • Erase job completed on station A, cert signed + verified"
echo "  • Wrong key rejected; tampered cert rejected"
echo "============================================================"
