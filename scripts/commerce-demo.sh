#!/usr/bin/env bash
# Wipestation commerce demo (ADR-0006 §3): one command that walks the whole
# customer journey against local services, the way scripts/demo.sh proves the
# station.
#
#   docker compose up -d && scripts/commerce-demo.sh
#
# It builds wipe-hub and wipestation, starts a hub with the Fake payment
# provider on its own fresh database (wipehub_demo_<run>) and a station on the
# mock backend, then, over the hub's public HTTP API and CLIs only: creates a
# vendor admin and a published offering, signs up a customer, verifies the
# email through Mailpit, buys the offering, publishes and downloads a release
# (checking its SHA-256), registers the station, downloads the licence and
# installs it by restarting the station (the ADR-0005 offline path), runs an
# erasure and verifies the certificate offline against the vendor root. Then a
# second station enrolls itself (ADR-0008): `wipestation enroll` prints a code,
# the customer (an owner) approves it through the hub API, and enroll verifies
# the licence against the vendor root and installs it into the running second
# station, whose certificate is verified the same way. No licence file is
# copied for the second station.
#
# Localhost only: the hub, the station, compose Postgres (127.0.0.1:55432) and
# Mailpit (127.0.0.1:8025). The first failed step prints
# `FAILED: <step>: <reason>`, keeps the work directory and tails both logs.
#
# Env: HUB_PORT (default 7910), STATION_PORT (default 7890), STATION2_PORT
# (default 7891).

set -Eeuo pipefail

# Settings inherited from a developer's shell that would change what the hub
# or station does (load a licence into the "unlicensed" station, pick Stripe,
# refuse to start on a bad portal dir...). Everything this run needs is passed
# explicitly below.
unset WIPESTATION_LICENSE WIPESTATION_STATIC_DIR WIPESTATION_BAY_TOPOLOGY \
  WIPESTATION_CONTROL_PLANE_URL WIPE_HUB_PORTAL_DIR WIPE_HUB_SITE_ORIGIN \
  WIPE_HUB_MAIL_FROM WIPE_HUB_PAYMENT_PROVIDER WIPE_HUB_STRIPE_SECRET_KEY \
  WIPE_HUB_STRIPE_WEBHOOK_SECRET WIPE_HUB_STRIPE_API_BASE WIPE_HUB_VENDOR_ROOT_SEED \
  WIPE_HUB_ADDR WIPE_HUB_DATABASE_URL WIPE_HUB_ARTIFACT_DIR WIPE_HUB_PUBLIC_BASE_URL

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"

if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

HUB_PORT="${HUB_PORT:-7910}"
STATION_PORT="${STATION_PORT:-7890}"
STATION2_PORT="${STATION2_PORT:-7891}"
HUB="http://127.0.0.1:${HUB_PORT}"
STATION="http://127.0.0.1:${STATION_PORT}"
STATION2="http://127.0.0.1:${STATION2_PORT}"
MAILPIT="http://127.0.0.1:8025"
PG_PORT=55432
STATION_ID="demo-bench-1"
STATION2_ID="demo-bench-2"
PASSWORD="demo-password-$(date +%s)"

# Set after the build, from cargo's own idea of the target dir
# (CARGO_TARGET_DIR / build.target-dir), so the fresh build is what runs.
WIPE_HUB_BIN=""
WIPESTATION_BIN=""

# Run-scoped names: unique per run so two runs in a row never collide.
RUN_EPOCH="$(date +%s)"
RUN_ID="${RUN_EPOCH}_$$_${RANDOM}"
DB_NAME="wipehub_demo_${RUN_ID}"
ADMIN_EMAIL="demo-admin-${RUN_ID}@wipehub.test"
CUSTOMER_EMAIL="demo-owner-${RUN_ID}@wipehub.test"
ORG_NAME="Demo Recyclers ${RUN_ID}"
RELEASE_VERSION="0.0.0-demo.${RUN_EPOCH}"
PLATFORM="$(printf '%s-%s' "$(uname -s)" "$(uname -m)" | tr '[:upper:]' '[:lower:]')"

STEP="start"
WORK=""
HUB_PID=""
STATION_PID=""
STATION2_PID=""
ENROLL_PID=""
DB_CREATED=""
FAILED_REPORTED=""
SUCCEEDED=""

# ---------------------------------------------------------------------------
# Helpers

step() {
  STEP="$1"
  echo "==> $1"
}

fail() {
  FAILED_REPORTED=1
  echo "FAILED: ${STEP}: $*" >&2
  exit 1
}

# A command failed at the top level under `set -e`: name it.
on_err() {
  local rc="$1" line="$2" cmd="$3"
  # Report from the main shell only; a subshell's failure surfaces there.
  [[ "${BASH_SUBSHELL:-0}" -eq 0 ]] || return 0
  fail "command failed (exit ${rc}) at line ${line}: ${cmd}"
}
trap 'on_err $? $LINENO "$BASH_COMMAND"' ERR

psql_admin() {
  # psql is not assumed on the host; the compose file already is. Connects to
  # the `postgres` maintenance database, never to the shared `wipehub` one.
  docker compose exec -T postgres psql -U wipehub -d postgres -v ON_ERROR_STOP=1 -Atqc "$1"
}

# Stop process $1 (TERM, then KILL after 5 s) and reap it. Never fails.
stop_pid() {
  local pid="$1"
  [[ -n "$pid" ]] || return 0
  if kill -0 "$pid" 2>/dev/null; then
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -9 "$pid" 2>/dev/null || true
  fi
  wait "$pid" 2>/dev/null || true
}

cleanup() {
  local rc=$?
  set +e
  trap - ERR
  if [[ $rc -ne 0 && -z "$FAILED_REPORTED" ]]; then
    echo "FAILED: ${STEP}: unexpected error (exit ${rc})" >&2
  fi
  stop_pid "$ENROLL_PID"
  stop_pid "$STATION2_PID"
  stop_pid "$STATION_PID"
  stop_pid "$HUB_PID"
  if [[ -n "$DB_CREATED" ]]; then
    if ! psql_admin "DROP DATABASE IF EXISTS \"${DB_NAME}\" WITH (FORCE)" >/dev/null; then
      echo "warning: could not drop database ${DB_NAME}; drop it by hand" >&2
    fi
  fi
  if [[ -n "$WORK" && -d "$WORK" ]]; then
    if [[ -n "$SUCCEEDED" ]]; then
      rm -rf "$WORK"
    else
      for log in "$WORK"/hub.log "$WORK"/station-unlicensed.log "$WORK"/station.log \
        "$WORK"/station2.log "$WORK"/enroll.log; do
        if [[ -f "$log" ]]; then
          echo "---- last lines of $log ----" >&2
          tail -n 25 "$log" >&2
        fi
      done
      echo "Work directory kept for inspection: $WORK" >&2
    fi
  fi
  exit "$rc"
}
trap cleanup EXIT

# Whether anything accepts a TCP connection on 127.0.0.1:$1.
port_open() {
  (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null
}

# wait_for CMD...: run CMD every 0.5 s until it succeeds, for up to 30 s
# (right after `docker compose up -d` Postgres and Mailpit are still starting).
wait_for() {
  for _ in $(seq 1 60); do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.5
  done
  return 1
}
mailpit_up() { curl -fsS "$MAILPIT/api/v1/info"; }
pg_ready() { docker compose exec -T postgres pg_isready -U wipehub -d postgres; }

# hub_request JAR METHOD PATH [JSON] EXPECT: one hub API call as the actor
# whose cookies live in JAR. Mutating requests carry the double-submit CSRF
# token and a JSON body. Sets BODY (and STATUS); fails the step on any other
# status than EXPECT.
BODY=""
STATUS=""
hub_request() {
  local jar="$1" method="$2" path="$3" json="$4" expect="$5"
  local out="$WORK/response.body"
  local args=(-sS -o "$out" -w '%{http_code}' -b "$jar" -c "$jar" -X "$method")
  if [[ "$method" != "GET" ]]; then
    local token
    token=$(curl -sS -b "$jar" -c "$jar" "$HUB/api/auth/csrf" | jq -r '.csrf_token // empty') || true
    [[ -n "$token" ]] || fail "GET /api/auth/csrf returned no token"
    args+=(-H "X-CSRF-Token: ${token}" -H 'Content-Type: application/json' --data "$json")
  fi
  STATUS=$(curl "${args[@]}" "$HUB$path") || STATUS="curl-error"
  BODY=$(cat "$out" 2>/dev/null || true)
  if [[ "$STATUS" != "$expect" ]]; then
    fail "$method $path returned ${STATUS} (expected ${expect}): ${BODY}"
  fi
}
hub_get() { hub_request "$1" GET "$2" "" "${3:-200}"; }
hub_post() { hub_request "$1" POST "$2" "$3" "${4:-200}"; }

# jq_field FILTER: the raw value of FILTER in BODY; fails the step if it is
# null, false or missing.
FIELD=""
jq_field() {
  FIELD=$(printf '%s' "$BODY" | jq -er "$1" 2>/dev/null) ||
    fail "unexpected response: no $1 in ${BODY}"
}

# mail_token ADDRESS PAGE: the 43-character token of the newest PAGE link
# (e.g. reset-password) mailed to ADDRESS, waiting up to 15 s. Sets TOKEN.
# Mail is isolated by the run-unique address; nothing is ever deleted.
TOKEN=""
mail_token() {
  local addr="$1" page="$2" id text
  TOKEN=""
  for _ in $(seq 1 75); do
    id=$(curl -sS -G "$MAILPIT/api/v1/search" --data-urlencode "query=to:\"${addr}\"" |
      jq -r '.messages[0].ID // empty') || true
    if [[ -n "$id" ]]; then
      text=$(curl -sS "$MAILPIT/api/v1/message/${id}" | jq -r '.Text // empty') || true
      TOKEN=$(printf '%s' "$text" | grep -oE "/${page}\?token=[A-Za-z0-9_-]{43}" | head -n 1 |
        sed 's/.*token=//') || true
      [[ -n "$TOKEN" ]] && return 0
    fi
    sleep 0.2
  done
  fail "no /${page}?token= link mailed to ${addr} within 15 s (Mailpit: $MAILPIT)"
}

sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    sha256sum "$1" | awk '{print $1}'
  fi
}

# launch_station PIDVAR PORT KEY CONFIG_DIR STATION_ID LOG [EXTRA ARGS...]:
# start a station on the mock backend, store its pid in the variable named
# PIDVAR at once (so cleanup stops it even if start-up fails), and wait until
# *this* station answers: its /api/public_key must be the key its log printed
# (with --no-fleet, /api/station only says "standalone", so a stale station on
# the port would pass that check).
launch_station() {
  local pidvar="$1" port="$2" key="$3" conf="$4" sid="$5" log="$6"
  shift 6
  # --ephemeral-config keeps configuration in RAM, but lease state is still
  # saved; keep it in the run's work directory, not ~/.config/wipestation.
  WIPESTATION_CONFIG_DIR="$conf" "$WIPESTATION_BIN" serve \
    --addr "127.0.0.1:${port}" \
    --station-id "$sid" \
    --key-path "$key" \
    --no-fleet \
    --fast \
    --ephemeral-config \
    "$@" >"$log" 2>&1 &
  local pid=$!
  printf -v "$pidvar" '%s' "$pid"
  local want="" got=""
  for _ in $(seq 1 75); do
    kill -0 "$pid" 2>/dev/null ||
      fail "the station exited during start-up (see $log): $(tail -n 5 "$log")"
    want=$(sed -n 's/^signing key public id: //p' "$log" | head -n 1) || true
    # `|| true` is load-bearing: under `set -e` the first connection refused
    # would end the run before the loop could retry.
    got=$(curl -fsS "http://127.0.0.1:${port}/api/public_key" 2>/dev/null | jq -r '.public_key_id // empty') || true
    [[ -n "$want" && "$got" == "$want" ]] && return 0
    sleep 0.2
  done
  fail "the station did not answer on :${port} with key '${want}' within 15 s (got '${got}')"
}

# start_station LOG [EXTRA ARGS...]: the first station. Sets STATION_PID.
start_station() {
  local log="$1"
  shift
  launch_station STATION_PID "$STATION_PORT" "$WORK/station.key" "$WORK/station-config" \
    "$STATION_ID" "$log" "$@"
}

# run_erasure BASE ASSET_TAG CERT: run an erasure on the station at BASE (mock
# backend), wait for `erased`, and save its certificate to CERT. Sets JOB_ID.
JOB_ID=""
run_erasure() {
  local base="$1" tag="$2" cert="$3" state="" cert_status=""
  JOB_ID=$(curl -fsS -X POST "$base/api/jobs" -H 'Content-Type: application/json' -d "$(
    jq -nc --arg t "$tag" '{
      device_id: "dev-nvme-0",
      classification: "high",
      intent: "reuse",
      verify: true,
      verify_samples: 4,
      operator: {id: "demo-op", display_name: "Demo Operator", email: "demo@wipestation.dev"},
      asset_tag: $t,
      site_label: "DemoSite",
      ticket_ref: "DEMO-COMMERCE"
    }')" | jq -r '.job_id // empty') || true
  [[ -n "$JOB_ID" ]] || fail "POST /api/jobs on ${base} returned no job_id"
  curl -fsS -X POST "$base/api/jobs/${JOB_ID}/start" >/dev/null || fail "POST /api/jobs/${JOB_ID}/start failed"
  for _ in $(seq 1 100); do
    state=$(curl -fsS "$base/api/jobs/${JOB_ID}" | jq -r '.state.state') || true
    case "$state" in
      erased | destroyed | quarantined | aborted) break ;;
    esac
    sleep 0.3
  done
  [[ "$state" == "erased" ]] || fail "job ${JOB_ID} ended in '${state}', expected 'erased'"
  # Signing happens after `erased`, asynchronously: poll, don't sleep.
  for _ in $(seq 1 50); do
    cert_status=$(curl -sS -o "$cert" -w '%{http_code}' "$base/api/jobs/${JOB_ID}/certificate") || true
    [[ "$cert_status" == "200" ]] && break
    sleep 0.2
  done
  [[ "$cert_status" == "200" ]] || fail "GET /api/jobs/${JOB_ID}/certificate returned ${cert_status} after 10 s"
}

# ---------------------------------------------------------------------------
# 1. Preconditions

step "check preconditions"
for tool in jq curl docker cargo; do
  command -v "$tool" >/dev/null 2>&1 || fail "\`$tool\` is not installed"
done
command -v shasum >/dev/null 2>&1 || command -v sha256sum >/dev/null 2>&1 ||
  fail "neither \`shasum\` nor \`sha256sum\` is installed"
wait_for port_open "$PG_PORT" ||
  fail "Postgres is not reachable on 127.0.0.1:${PG_PORT}: run \`docker compose up -d\`"
wait_for pg_ready ||
  fail "the compose Postgres service is not ready: run \`docker compose up -d\`"
wait_for mailpit_up ||
  fail "Mailpit is not reachable on ${MAILPIT}: run \`docker compose up -d\`"
wait_for port_open 1025 ||
  fail "Mailpit SMTP is not reachable on 127.0.0.1:1025: run \`docker compose up -d\`"
for port in "$HUB_PORT" "$STATION_PORT" "$STATION2_PORT"; do
  if port_open "$port"; then
    fail "port ${port} already in use; find the owner with \`lsof -nP -iTCP:${port} -sTCP:LISTEN\`, or set HUB_PORT / STATION_PORT / STATION2_PORT"
  fi
done

TMP_ROOT="${TMPDIR:-/tmp}"
WORK="$(mktemp -d "${TMP_ROOT%/}/wipestation-commerce-demo.XXXXXX")"
echo "    work dir: $WORK"

# ---------------------------------------------------------------------------
# 2. Build

step "build wipe-hub and wipestation"
cargo build -p wipe-hub -p wipe-cli || fail "cargo build failed"
TARGET_DIR=$(cargo metadata --format-version 1 --no-deps | jq -r '.target_directory // empty') || true
[[ -n "$TARGET_DIR" ]] || fail "\`cargo metadata\` named no target directory"
WIPE_HUB_BIN="$TARGET_DIR/debug/wipe-hub"
WIPESTATION_BIN="$TARGET_DIR/debug/wipestation"
[[ -x "$WIPE_HUB_BIN" && -x "$WIPESTATION_BIN" ]] || fail "built binaries not found under $TARGET_DIR/debug"

# ---------------------------------------------------------------------------
# 3. Run database

step "create run database ${DB_NAME}"
# Marked before the create: an interrupt between the two must still drop it
# (the drop is IF EXISTS).
DB_CREATED=1
psql_admin "CREATE DATABASE \"${DB_NAME}\"" >/dev/null || fail "could not create ${DB_NAME}"

# Shared by the hub and both CLIs, so they see one database and one artifact
# dir.
export WIPE_HUB_DATABASE_URL="postgres://wipehub:wipehub@127.0.0.1:${PG_PORT}/${DB_NAME}"
export WIPE_HUB_ARTIFACT_DIR="$WORK/artifacts"
export WIPE_HUB_PUBLIC_BASE_URL="$HUB"
export WIPE_HUB_SMTP_HOST="127.0.0.1"
export WIPE_HUB_SMTP_PORT="1025"

# ---------------------------------------------------------------------------
# 4. Vendor root

step "generate vendor root"
ROOT_OUT=$("$WIPESTATION_BIN" license new-root --out "$WORK/vendor-root.seed")
ROOT_B64=$(printf '%s\n' "$ROOT_OUT" | sed -n 's/.*PUBLISH THIS: *//p')
[[ -n "$ROOT_B64" ]] || fail "\`license new-root\` printed no PUBLISH THIS line: ${ROOT_OUT}"

# ---------------------------------------------------------------------------
# 5. Hub

step "start hub on :${HUB_PORT} (Fake payment provider)"
"$WIPE_HUB_BIN" serve \
  --addr "127.0.0.1:${HUB_PORT}" \
  --database-url "$WIPE_HUB_DATABASE_URL" \
  --payment-provider fake \
  --vendor-root-seed "$WORK/vendor-root.seed" \
  --public-base-url "$HUB" \
  --artifact-dir "$WIPE_HUB_ARTIFACT_DIR" >"$WORK/hub.log" 2>&1 &
HUB_PID=$!
health=""
for _ in $(seq 1 150); do
  kill -0 "$HUB_PID" 2>/dev/null ||
    fail "the hub exited during start-up (see $WORK/hub.log): $(tail -n 5 "$WORK/hub.log")"
  health=$(curl -sS -o /dev/null -w '%{http_code}' "$HUB/api/health" 2>/dev/null) || true
  [[ "$health" == "200" ]] && break
  sleep 0.2
done
[[ "$health" == "200" ]] || fail "GET /api/health did not return 200 within 30 s (last: ${health})"

# ---------------------------------------------------------------------------
# 6. Station, unlicensed: mints the instance key the hub will license

step "start station unlicensed and read its instance key"
start_station "$WORK/station-unlicensed.log"
KEY_ID=$(curl -fsS "$STATION/api/public_key" | jq -r '.public_key_id // empty') || true
STATION_PK=$(curl -fsS "$STATION/api/public_key" | jq -r '.public_key_b64 // empty') || true
[[ -n "$KEY_ID" && -n "$STATION_PK" ]] || fail "GET /api/public_key on the station returned no key"
echo "    instance key: $KEY_ID"

# ---------------------------------------------------------------------------
# 7. Vendor admin

ADMIN_JAR="$WORK/admin.cookies"
CUSTOMER_JAR="$WORK/customer.cookies"
: >"$ADMIN_JAR"
: >"$CUSTOMER_JAR"

step "bootstrap vendor admin ${ADMIN_EMAIL}"
"$WIPE_HUB_BIN" admin create --email "$ADMIN_EMAIL" --name "Demo Vendor" >"$WORK/admin-create.log" 2>&1 ||
  fail "\`wipe-hub admin create\` failed: $(cat "$WORK/admin-create.log")"
mail_token "$ADMIN_EMAIL" "reset-password"
hub_post "$ADMIN_JAR" /api/auth/password-reset/complete \
  "$(jq -nc --arg t "$TOKEN" --arg p "$PASSWORD" '{token: $t, password: $p}')"
hub_post "$ADMIN_JAR" /api/auth/sign-in \
  "$(jq -nc --arg e "$ADMIN_EMAIL" --arg p "$PASSWORD" '{email: $e, password: $p}')"

# ---------------------------------------------------------------------------
# 8. Offering

step "create and publish an offering"
OFFERING_BODY=$(jq -nc '{
  name: "Station — annual, 2 seats (demo)",
  description: "Two stations, unlimited erasures.",
  price_minor: 120000,
  currency: "usd",
  interval: "annual",
  grants: {
    scope: "machine",
    seats: 2,
    quota_erasures: null,
    features: ["pdf_certificates", "hub_sync"],
    term_months: 12
  }
}')
hub_post "$ADMIN_JAR" /api/admin/offerings "$OFFERING_BODY" 201
jq_field '.offering.id'
OFFERING_ID="$FIELD"
hub_post "$ADMIN_JAR" "/api/admin/offerings/${OFFERING_ID}/publish" '{}'
hub_get "$ADMIN_JAR" /api/offerings
FIELD=$(printf '%s' "$BODY" | jq -r --arg id "$OFFERING_ID" '.offerings[] | select(.id == $id) | .id') || true
[[ "$FIELD" == "$OFFERING_ID" ]] || fail "published offering ${OFFERING_ID} is not in GET /api/offerings: ${BODY}"

# ---------------------------------------------------------------------------
# 9. Customer sign-up

step "sign up customer ${CUSTOMER_EMAIL} (${ORG_NAME})"
hub_post "$CUSTOMER_JAR" /api/auth/sign-up \
  "$(jq -nc --arg e "$CUSTOMER_EMAIL" --arg p "$PASSWORD" --arg o "$ORG_NAME" \
    '{email: $e, password: $p, name: "Dana Demo", organization_name: $o}')" 201
jq_field '.organization.id'
ORG_ID="$FIELD"

# ---------------------------------------------------------------------------
# 10. Email verification

step "verify customer email via Mailpit"
mail_token "$CUSTOMER_EMAIL" "verify-email"
hub_post "$CUSTOMER_JAR" /api/auth/verify-email "$(jq -nc --arg t "$TOKEN" '{token: $t}')"
jq_field '.email_verified'

# ---------------------------------------------------------------------------
# 11. Checkout and Fake payment

step "check out and pay (Fake provider)"
hub_post "$CUSTOMER_JAR" /api/checkout "$(jq -nc --arg o "$OFFERING_ID" '{offering_id: $o}')" 201
jq_field '.order.id'
ORDER_ID="$FIELD"
jq_field '.checkout_url'
SESSION=$(printf '%s' "$FIELD" | sed -n 's/.*[?&]session=\([^&]*\).*/\1/p')
[[ "$SESSION" == fake_cs_* ]] || fail "checkout_url has no Fake session: ${FIELD}"
hub_post "$CUSTOMER_JAR" "/api/payments/fake/sessions/${SESSION}/pay" '{}'
jq_field '.order.provider'
[[ "$FIELD" == "fake" ]] || fail "order provider is '${FIELD}', expected 'fake'"
jq_field '.order.status'
[[ "$FIELD" == "paid" ]] || fail "order status is '${FIELD}', expected 'paid'"
hub_get "$CUSTOMER_JAR" /api/entitlements
ENTITLEMENT_ID=$(printf '%s' "$BODY" |
  jq -r --arg o "$ORDER_ID" '.entitlements[] | select(.order_id == $o) | .id') || true
[[ -n "$ENTITLEMENT_ID" ]] || fail "no entitlement for order ${ORDER_ID}: ${BODY}"

# ---------------------------------------------------------------------------
# 12. Release

step "publish release ${RELEASE_VERSION} (${PLATFORM}) with the CLI"
"$WIPE_HUB_BIN" release publish \
  --version "$RELEASE_VERSION" \
  --notes "Commerce demo build" \
  --file "${PLATFORM}=${WIPESTATION_BIN}" >"$WORK/release-publish.log" 2>&1 ||
  fail "\`wipe-hub release publish\` failed: $(cat "$WORK/release-publish.log")"

# ---------------------------------------------------------------------------
# 13. Download

step "download the release as the customer and check its SHA-256"
hub_get "$CUSTOMER_JAR" /api/downloads
ARTIFACT=$(printf '%s' "$BODY" | jq -c --arg v "$RELEASE_VERSION" --arg p "$PLATFORM" \
  '[.releases[] | select(.version == $v) | .artifacts[] | select(.platform == $p)][0] // empty') || true
[[ -n "$ARTIFACT" ]] || fail "release ${RELEASE_VERSION} for ${PLATFORM} is not in GET /api/downloads: ${BODY}"
ARTIFACT_ID=$(printf '%s' "$ARTIFACT" | jq -r '.id')
LISTED_SHA=$(printf '%s' "$ARTIFACT" | jq -r '.sha256')
hub_post "$CUSTOMER_JAR" "/api/downloads/${ARTIFACT_ID}/link" '{}'
jq_field '.url'
LINK="$FIELD"
DL_STATUS=$(curl -sS -o "$WORK/downloaded-wipestation" -w '%{http_code}' "$HUB$LINK") || DL_STATUS="curl-error"
[[ "$DL_STATUS" == "200" ]] || fail "GET ${LINK%%\?*} returned ${DL_STATUS}"
GOT_SHA=$(sha256_of "$WORK/downloaded-wipestation")
BUILT_SHA=$(sha256_of "$WIPESTATION_BIN")
[[ "$GOT_SHA" == "$LISTED_SHA" ]] || fail "downloaded SHA-256 ${GOT_SHA} != listed ${LISTED_SHA}"
[[ "$GOT_SHA" == "$BUILT_SHA" ]] || fail "downloaded SHA-256 ${GOT_SHA} != built binary ${BUILT_SHA}"
echo "    sha256: $GOT_SHA"

# ---------------------------------------------------------------------------
# 14. Station registration and licence

step "register the station and download its licence"
hub_post "$CUSTOMER_JAR" /api/stations \
  "$(jq -nc --arg e "$ENTITLEMENT_ID" --arg k "$KEY_ID" --arg s "$STATION_ID" \
    '{entitlement_id: $e, instance_key_id: $k, station_id: $s, label: "Demo bench 1"}')" 201
jq_field '.licence.id'
LICENCE_ID="$FIELD"
hub_get "$CUSTOMER_JAR" "/api/licences/${LICENCE_ID}/download"
# The hub's bytes, unchanged: the station must load exactly what it served.
cp "$WORK/response.body" "$WORK/licence.json"
jq_field '.body.license_id'
[[ "$FIELD" == "$LICENCE_ID" ]] || fail "licence file names ${FIELD}, expected ${LICENCE_ID}"
echo "    licence: $LICENCE_ID"

# ---------------------------------------------------------------------------
# 15. Vendor root as published by the hub

step "fetch the vendor root from the hub"
hub_get "$CUSTOMER_JAR" /api/licensing/root
jq_field '.root.public_key_b64'
[[ "$FIELD" == "$ROOT_B64" ]] || fail "hub root ${FIELD} != generated root ${ROOT_B64}"

# ---------------------------------------------------------------------------
# 16. Install the licence (ADR-0005 offline path: restart with --license)

step "install licence by restarting the station"
stop_pid "$STATION_PID"
STATION_PID=""
start_station "$WORK/station.log" --license "$WORK/licence.json" --vendor-root-b64 "$ROOT_B64"
grep -qF "licence: ${LICENCE_ID} for ${ORG_NAME} (" "$WORK/station.log" ||
  fail "the station log does not show 'licence: ${LICENCE_ID} for ${ORG_NAME}': $(grep -m1 'licence' "$WORK/station.log" || true)"
KEY_AFTER=$(curl -fsS "$STATION/api/public_key" | jq -r '.public_key_id // empty') || true
[[ "$KEY_AFTER" == "$KEY_ID" ]] || fail "instance key changed across restart: ${KEY_ID} -> ${KEY_AFTER}"

# ---------------------------------------------------------------------------
# 17. Erasure (mock backend)

step "run an erasure on the mock backend and fetch the certificate"
CERT="$WORK/certificate.json"
run_erasure "$STATION" "DEMO-COMMERCE-1" "$CERT"
JOB1_ID="$JOB_ID"

# ---------------------------------------------------------------------------
# 18. Certificate content

step "check the certificate is licensed to the customer"
[[ "$(jq -r '.certificate.evaluation' "$CERT")" == "false" ]] ||
  fail "certificate is marked evaluation (unlicensed)"
CERT_CUSTOMER=$(jq -r '.attestation.license.body.entitlements.customer_name // empty' "$CERT")
[[ "$CERT_CUSTOMER" == "$ORG_NAME" ]] || fail "certificate names customer '${CERT_CUSTOMER}', expected '${ORG_NAME}'"
CERT_CUSTOMER_ID=$(jq -r '.attestation.license.body.entitlements.customer_id // empty' "$CERT")
[[ "$CERT_CUSTOMER_ID" == "$ORG_ID" ]] || fail "certificate names customer id '${CERT_CUSTOMER_ID}', expected '${ORG_ID}'"
CERT_LICENCE=$(jq -r '.attestation.license.body.license_id // empty' "$CERT")
[[ "$CERT_LICENCE" == "$LICENCE_ID" ]] || fail "certificate carries licence '${CERT_LICENCE}', expected '${LICENCE_ID}'"

# ---------------------------------------------------------------------------
# 19. Offline verification

step "verify the certificate offline against the vendor root"
VERIFY_OUT=$("$WIPESTATION_BIN" verify-cert "$CERT" \
  --public-key-b64 "$STATION_PK" \
  --vendor-root-b64 "$ROOT_B64" \
  --require-licensed 2>&1) || fail "verify-cert failed: ${VERIFY_OUT}"
printf '%s\n' "$VERIFY_OUT" | sed 's/^/    /'
grep -q '^LICENSED' <<<"$VERIFY_OUT" || fail "verify-cert did not print LICENSED"
grep -qF "  customer: ${ORG_NAME} (" <<<"$VERIFY_OUT" || fail "verify-cert did not name customer '${ORG_NAME}'"

# ---------------------------------------------------------------------------
# 20. Negative check

step "check an unrelated vendor root is rejected"
WRONG_OUT=$("$WIPESTATION_BIN" license new-root --out "$WORK/wrong-root.seed")
WRONG_B64=$(printf '%s\n' "$WRONG_OUT" | sed -n 's/.*PUBLISH THIS: *//p')
[[ -n "$WRONG_B64" ]] || fail "\`license new-root\` printed no PUBLISH THIS line"
if "$WIPESTATION_BIN" verify-cert "$CERT" \
  --public-key-b64 "$STATION_PK" \
  --vendor-root-b64 "$WRONG_B64" \
  --require-licensed >"$WORK/wrong-root.out" 2>&1; then
  fail "verify-cert accepted the certificate against an unrelated vendor root"
fi
# Any non-zero exit is not enough: a bad flag or a crash also exits non-zero.
grep -q 'attestation chain INVALID' "$WORK/wrong-root.out" ||
  fail "verify-cert failed against the wrong root, but not as an invalid chain: $(cat "$WORK/wrong-root.out")"
echo "    OK: rejected ($(grep -m1 'attestation chain INVALID' "$WORK/wrong-root.out"))"

# ---------------------------------------------------------------------------
# 21. A second station, enrolled from the bench (ADR-0008)

step "start a second station on :${STATION2_PORT} (own key, no licence)"
launch_station STATION2_PID "$STATION2_PORT" "$WORK/station2.key" "$WORK/station2-config" \
  "$STATION2_ID" "$WORK/station2.log" --vendor-root-b64 "$ROOT_B64"
KEY2_ID=$(curl -fsS "$STATION2/api/public_key" | jq -r '.public_key_id // empty') || true
STATION2_PK=$(curl -fsS "$STATION2/api/public_key" | jq -r '.public_key_b64 // empty') || true
[[ -n "$KEY2_ID" && -n "$STATION2_PK" ]] || fail "GET /api/public_key on the second station returned no key"
[[ "$KEY2_ID" != "$KEY_ID" ]] || fail "the second station has the first station's key"
echo "    instance key: $KEY2_ID"

step "run wipestation enroll for the second station"
# Same configuration directory as the second station, so the saved hub
# address and licence land beside its lease state.
WIPESTATION_CONFIG_DIR="$WORK/station2-config" "$WIPESTATION_BIN" enroll \
  --hub "$HUB" \
  --key-path "$WORK/station2.key" \
  --vendor-root-b64 "$ROOT_B64" \
  --station-id "$STATION2_ID" \
  --label "Demo bench 2" \
  --station "$STATION2" >"$WORK/enroll.log" 2>&1 &
ENROLL_PID=$!
USER_CODE=""
for _ in $(seq 1 75); do
  USER_CODE=$(sed -n 's/^user code: //p' "$WORK/enroll.log" | head -n 1) || true
  [[ -n "$USER_CODE" ]] && break
  kill -0 "$ENROLL_PID" 2>/dev/null ||
    fail "enroll exited before printing a code: $(tail -n 5 "$WORK/enroll.log")"
  sleep 0.2
done
[[ -n "$USER_CODE" ]] || fail "enroll printed no 'user code:' line within 15 s"
grep -q "^open: ${HUB}/activate?code=${USER_CODE}\$" "$WORK/enroll.log" ||
  fail "enroll did not print the activation link for ${USER_CODE}: $(cat "$WORK/enroll.log")"
echo "    user code: $USER_CODE"

step "approve the code as the organization owner"
hub_post "$CUSTOMER_JAR" /api/enrollments/approve \
  "$(jq -nc --arg c "$USER_CODE" --arg e "$ENTITLEMENT_ID" '{code: $c, entitlement_id: $e}')" 201
jq_field '.licence.id'
LICENCE2_ID="$FIELD"
echo "    licence: $LICENCE2_ID"

step "wait for enroll to collect, verify and install the licence"
# It polls every 5 s; allow a few intervals.
for _ in $(seq 1 150); do
  kill -0 "$ENROLL_PID" 2>/dev/null || break
  sleep 0.2
done
kill -0 "$ENROLL_PID" 2>/dev/null && fail "enroll still running 30 s after approval: $(tail -n 5 "$WORK/enroll.log")"
if wait "$ENROLL_PID"; then ENROLL_RC=0; else ENROLL_RC=$?; fi
ENROLL_PID=""
[[ "$ENROLL_RC" -eq 0 ]] || fail "enroll exited ${ENROLL_RC}: $(tail -n 5 "$WORK/enroll.log")"
sed 's/^/    | /' "$WORK/enroll.log"
grep -q "^Installed into the station at ${STATION2}" "$WORK/enroll.log" ||
  fail "enroll did not install the licence into the second station"
SAVED2=$(jq -r '.body.license_id // empty' "$WORK/station2-config/enrolled-licence.json" 2>/dev/null) || true
[[ "$SAVED2" == "$LICENCE2_ID" ]] || fail "enrolled-licence.json names '${SAVED2}', expected '${LICENCE2_ID}'"
SAVED_HUB=$(jq -r '.hub_origin // empty' "$WORK/station2-config/station-settings.json" 2>/dev/null) || true
[[ "$SAVED_HUB" == "$HUB" ]] || fail "station-settings.json names hub '${SAVED_HUB}', expected '${HUB}'"
LIC2=$(curl -fsS "$STATION2/api/license") || fail "GET /api/license on the second station failed"
[[ "$(printf '%s' "$LIC2" | jq -r '.status.status')" == "valid" ]] ||
  fail "the second station's licence is not valid: ${LIC2}"
[[ "$(printf '%s' "$LIC2" | jq -r '.instance_public_key_id')" == "$KEY2_ID" ]] ||
  fail "the second station's licence names another key: ${LIC2}"

step "run an erasure on the second station and verify its certificate offline"
CERT2="$WORK/certificate2.json"
run_erasure "$STATION2" "DEMO-COMMERCE-2" "$CERT2"
JOB2_ID="$JOB_ID"
[[ "$(jq -r '.certificate.evaluation' "$CERT2")" == "false" ]] ||
  fail "the second station's certificate is marked evaluation (unlicensed)"
CERT2_LICENCE=$(jq -r '.attestation.license.body.license_id // empty' "$CERT2")
[[ "$CERT2_LICENCE" == "$LICENCE2_ID" ]] ||
  fail "the second certificate carries licence '${CERT2_LICENCE}', expected '${LICENCE2_ID}'"
VERIFY2_OUT=$("$WIPESTATION_BIN" verify-cert "$CERT2" \
  --public-key-b64 "$STATION2_PK" \
  --vendor-root-b64 "$ROOT_B64" \
  --require-licensed 2>&1) || fail "verify-cert failed on the second certificate: ${VERIFY2_OUT}"
grep -q '^LICENSED' <<<"$VERIFY2_OUT" || fail "verify-cert did not print LICENSED for the second certificate"
grep -qF "  license:  ${LICENCE2_ID}" <<<"$VERIFY2_OUT" ||
  fail "verify-cert did not name licence '${LICENCE2_ID}': ${VERIFY2_OUT}"
echo "    OK: job ${JOB2_ID}, LICENSED under ${LICENCE2_ID}"

# ---------------------------------------------------------------------------
# 22. Summary

SUCCEEDED=1
step "done"
echo
echo "============================================================"
echo "  Wipestation commerce demo complete."
echo "  Customer:  ${ORG_NAME} (${CUSTOMER_EMAIL})"
echo "  Licence:   ${LICENCE_ID} (portal registration, file installed)"
echo "  Licence:   ${LICENCE2_ID} (wipestation enroll, approved by the owner)"
echo "  Release:   ${RELEASE_VERSION} (${PLATFORM}), SHA-256 matched"
echo "  Certs:     jobs ${JOB1_ID} and ${JOB2_ID}, LICENSED, verified offline"
echo "             to the vendor root"
echo "  The stations ran the MOCK backend: no real drive was touched, and this"
echo "  demo makes no claim about sanitizing hardware."
echo "============================================================"
