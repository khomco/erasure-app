# The commercial platform on one laptop

Step by step, from a fresh clone to a running hub, portal and marketing site,
and the commerce demo that walks the whole customer journey (ADR-0006 §3,
ADR-0007). Everything here runs on localhost; nothing needs a Stripe account,
a cloud account or real drives. The station runs the **mock backend**
(`wipe-engine-mock`): no step on this page sanitizes a real drive.

## 1. Prerequisites

- Docker with Compose (`docker compose`).
- Rust stable through rustup (the version is pinned in `rust-toolchain.toml`).
- Node and pnpm 9+.
- `jq` and `curl`, for the commerce demo.

```bash
pnpm install        # at the repo root: the portal, the marketing site and the desktop app
```

## 2. Local dependencies

```bash
docker compose up -d
```

`compose.yaml` publishes, on 127.0.0.1 only:

| Service | Port | What for |
|---|---|---|
| Postgres 16 | `55432` | the hub's database (`postgres://wipehub:wipehub@127.0.0.1:55432/wipehub`) |
| Mailpit SMTP | `1025` | every email the hub sends |
| Mailpit UI and API | `8025` | read that mail at <http://127.0.0.1:8025> |

The hub's tests (`cargo test -p wipe-hub`) need both services and fail, never
skip, when they are down.

## 3. The hub

```bash
cargo run -p wipe-hub -- serve
curl 127.0.0.1:7900/api/health       # {"status":"ok","database":"up","mail":"up"}
curl 127.0.0.1:7900/api/deployment   # what kind of hub this is (section 7)
```

`serve` applies the embedded migrations before it listens and exits within
about 5 s if Postgres is unreachable. Every flag has a `WIPE_HUB_*` env twin,
and the defaults match `compose.yaml`:

| Flag | Env twin | Default |
|---|---|---|
| `--addr` | `WIPE_HUB_ADDR` | `127.0.0.1:7900` |
| `--database-url` | `WIPE_HUB_DATABASE_URL` | the compose Postgres |
| `--smtp-host`, `--smtp-port` | `WIPE_HUB_SMTP_HOST`, `WIPE_HUB_SMTP_PORT` | `127.0.0.1`, `1025` |
| `--public-base-url` | `WIPE_HUB_PUBLIC_BASE_URL` | `http://127.0.0.1:7900` (the origin emailed links point at) |
| `--payment-provider` | `WIPE_HUB_PAYMENT_PROVIDER` | `fake` (`stripe` is section 6) |
| `--vendor-root-seed` | `WIPE_HUB_VENDOR_ROOT_SEED` | none: licence issuance unavailable |
| `--portal-dir` | `WIPE_HUB_PORTAL_DIR` | `apps/portal/dist` under the working directory |
| `--site-origin` | `WIPE_HUB_SITE_ORIGIN` | `http://localhost:4321` (the marketing site's dev server) |
| `--artifact-dir` | `WIPE_HUB_ARTIFACT_DIR` | `.wipe-hub/artifacts` (git-ignored) |

`cargo run -p wipe-hub -- serve --help` lists them all.

### A vendor root, so the hub can sign licences

Without one, station registration answers 503 and the hub warns at start.
Make a development root with the station binary and give the hub its seed:

```bash
cargo run -p wipe-cli --bin wipestation -- license new-root --out .wipe-hub/vendor-root.seed
cargo run -p wipe-hub -- serve --vendor-root-seed .wipe-hub/vendor-root.seed
```

Run `new-root` once: it overwrites an existing file, so re-running it replaces
the seed, and licences signed with the old root no longer verify against the hub.
`new-root` prints the public key to give stations (`PUBLISH THIS` line). A
file-held root is development custody only: the hub logs that on every start,
and production must not launch on one (ADR-0007).

### The first vendor admin

```bash
cargo run -p wipe-hub -- admin create --email admin@example.test --name "Vera Vendor"
```

It emails a set-password link; open it from Mailpit (<http://127.0.0.1:8025>).
Vendor admins publish offerings and releases in the portal's `/admin` area;
`wipe-hub release publish --version 0.2.0 --notes "…" --file linux-x86_64=<path>`
publishes a release from the command line.

## 4. The portal

Either build it once and let the hub serve it on its own origin:

```bash
pnpm --filter portal build
cargo run -p wipe-hub -- serve       # the portal is now at http://127.0.0.1:7900/
```

or run Vite's dev server, with hot reload, on <http://localhost:5174>. It
proxies `/api` to the hub on `:7900` (`VITE_API_TARGET` overrides that):

```bash
pnpm --filter portal dev
```

Emailed links (verification, magic link, invitations) point at the hub's
`--public-base-url`. To have them open the dev server, start the hub with
`--public-base-url http://localhost:5174`.

## 5. The marketing site

```bash
PUBLIC_HUB_ORIGIN=http://localhost:7900 pnpm --filter www dev   # http://localhost:4321
```

- `PUBLIC_HUB_ORIGIN` opens the early-access and contact forms, which post
  to the hub's `POST /api/leads`. Unset, they stay closed. The hub must list
  the site's origin in `--site-origin` (the default matches), and the two must
  be the same site: `localhost` with `localhost`, not `localhost` with
  `127.0.0.1`.
- `PRICING_HUB_ORIGIN=http://127.0.0.1:7900 pnpm --filter www build` lists
  live prices from the hub's `GET /api/offerings`, and fails if it cannot
  reach the hub. Unset, `/pricing/` lists no prices and needs no hub.
- `astro build` refuses `PUBLIC_HUB_ORIGIN` while the privacy notice is a
  placeholder; `astro dev` allows it. See `sites/www/README.md`.

## 6. Stripe test mode (optional)

Every automated test uses the Fake payment provider or a local mock of the
Stripe API. To try real Stripe test mode with test-mode keys and
`stripe listen`, follow [Stripe test mode](../../README.md#stripe-test-mode)
in the README.

## 7. The development banner

`GET /api/deployment` needs no session and reports the hub's payment provider,
how its vendor root is held, and whether it is a **development hub**: one on
the Fake payment provider or with a file-held vendor root. The rule lives in
the hub (`crates/wipe-hub/src/deployment.rs`); the portal only renders it.

While `development` is true, every portal page, signed in or not, shows an
amber bar above the header that cannot be closed: "Development hub — not for
production use." and one line per reason. So the default local hub (Fake, no
root) shows it, the demo hub (Fake, file root) shows both reasons, a Stripe
hub with a file root shows the root reason, and a Stripe hub with no root
shows none. If the hub cannot answer, the portal shows no banner rather than a
guessed one. The marketing site and the desktop app have no banner.

## 8. The commerce demo

```bash
docker compose up -d
scripts/commerce-demo.sh
```

One command for the whole customer journey, exiting non-zero at the first
failed step. It builds `wipe-hub` and `wipestation`, starts a hub with the Fake
provider on a fresh throwaway database and a station on the mock backend, then
creates a vendor admin and an offering, signs a customer up and verifies the
email through Mailpit, buys the offering, publishes and downloads a release
(checking its SHA-256), registers the station, installs its licence and
verifies a licensed certificate offline. It uses ports 7910 (hub) and 7890
(station); override them with `HUB_PORT` and `STATION_PORT`. It ignores any
`WIPE_HUB_*` settings in your shell, so it does not disturb a hub you already
run on 7900.

## 9. Teardown

Stop `wipe-hub serve` and the dev servers with Ctrl-C, then:

```bash
docker compose down          # keeps the Postgres volume
docker compose down -v       # also deletes the database
rm -rf .wipe-hub             # release files and a development vendor root seed
```
