//! `wipestation` CLI — single binary that can:
//!   * `serve`        — run the Axum API + fleet on a TCP port
//!   * `verify-cert`  — verify a signed certificate JSON against a public key
//!   * `inspect`      — enumerate mock devices and print as JSON

use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use wipe_cert::{SignedCertificate, SigningKey, VerifyingKey};
use wipe_engine_mock::{MockBackend, MockTiming};
use wipe_fleet::FleetService;
use wipe_server::{serve, AppState};

#[derive(Parser, Debug)]
#[command(
    name = "wipestation",
    version,
    about = "wipestation — data sanitization toolkit"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run the Axum API server with the mock backend. Advertises on mDNS.
    Serve {
        /// TCP address to bind, e.g. `127.0.0.1:7878`.
        #[arg(long, default_value = "127.0.0.1:7878")]
        addr: SocketAddr,
        /// Station identifier (defaults to a random UUID).
        #[arg(long)]
        station_id: Option<String>,
        /// Disable mDNS fleet advertisement.
        #[arg(long)]
        no_fleet: bool,
        /// Path where the signing key seed is read/written.
        /// If the file doesn't exist a new key is generated and persisted.
        #[arg(long)]
        key_path: Option<PathBuf>,
        /// Speed up mock erasures (useful for demos/CI).
        #[arg(long)]
        fast: bool,
        /// Directory containing the built frontend (`index.html` + `assets/`).
        /// If omitted, common locations are auto-detected; if no UI is found
        /// the server still runs API-only.
        #[arg(long, env = "WIPESTATION_STATIC_DIR")]
        static_dir: Option<PathBuf>,
        /// Path to a bay-topology JSON file describing this station's
        /// physical drive bays (ADR-0002). Takes precedence over
        /// `--bay-profile`. If neither is given the station reports an
        /// explicitly-unconfigured bench rather than inventing a chassis.
        #[arg(long, env = "WIPESTATION_BAY_TOPOLOGY")]
        bay_topology: Option<PathBuf>,
        /// Built-in bay-topology preset to start from. Presets expand into
        /// the same model a config file uses; run `wipestation bay-presets`
        /// to list them and to dump one as a starting point.
        #[arg(long)]
        bay_profile: Option<String>,
        /// Base URL of a control plane to hold this station's configuration
        /// when the station has no writable storage (ADR-0003 tier 2). The
        /// hub itself is future work; with no writable path and no reachable
        /// control plane the station runs ephemeral and says so.
        #[arg(long, env = "WIPESTATION_CONTROL_PLANE_URL")]
        control_plane_url: Option<String>,
        /// Skip store detection and keep configuration in RAM only. For
        /// demos and for reproducing what a PXE station sees.
        #[arg(long)]
        ephemeral_config: bool,
        /// Path to a vendor-issued licence (ADR-0005). Without one the
        /// station runs free-tier: it still erases and still signs, but
        /// certificates are marked `evaluation`.
        #[arg(long, env = "WIPESTATION_LICENSE")]
        license: Option<PathBuf>,
        /// Published vendor root public key(s), base64. Required with
        /// --license: a licence is only meaningful if we check who signed it.
        #[arg(long)]
        vendor_root_b64: Vec<String>,
    },
    /// List built-in bay-topology presets, or dump one as JSON to use as a
    /// starting point for a station config file.
    BayPresets {
        /// Print this preset as JSON instead of listing names.
        #[arg(long)]
        dump: Option<String>,
    },
    /// Verify a signed certificate JSON file against a public key.
    VerifyCert {
        /// Path to a SignedCertificate JSON document.
        cert: PathBuf,
        /// Trusted public key, base64 (no padding) of the 32-byte Ed25519 key.
        /// Can be repeated for multiple keys.
        #[arg(long, required = true)]
        public_key_b64: Vec<String>,
        /// Published vendor ROOT public key, base64. Supply this to verify
        /// the attestation chain (ADR-0005): that the signing key was
        /// licensed to a named customer under stated entitlements. Can be
        /// repeated so a verifier can accept more than one root.
        #[arg(long)]
        vendor_root_b64: Vec<String>,
        /// Treat an unlicensed (evaluation) certificate as a failure.
        /// Off by default: an evaluation cert is still a valid record of a
        /// real erasure, and auditors usually want to see it.
        #[arg(long)]
        require_licensed: bool,
    },
    /// Print the catalog of devices the mock backend would expose.
    Inspect,
    /// Vendor licensing tools (ADR-0005).
    ///
    /// `new-root` and `issue` are *vendor-side* operations. In production the
    /// root private key never leaves an offline signer and these run there,
    /// not on a station.
    #[command(subcommand)]
    License(LicenseCmd),
}

#[derive(Subcommand, Debug)]
enum LicenseCmd {
    /// Generate a vendor root keypair. Writes the 32-byte seed and prints the
    /// public key to publish.
    NewRoot {
        /// Where to write the root seed. Guard this file — it is the
        /// highest-value secret in the product.
        #[arg(long)]
        out: PathBuf,
    },
    /// Issue a licence binding an instance key to entitlements.
    Issue {
        /// Vendor root seed produced by `new-root`.
        #[arg(long)]
        root_seed: PathBuf,
        /// Instance public key id to license, e.g. `ed25519:...`. Read it
        /// from a station's startup log or `GET /api/public_key`.
        #[arg(long)]
        instance_key_id: String,
        #[arg(long)]
        license_id: String,
        #[arg(long)]
        customer_id: String,
        #[arg(long)]
        customer_name: String,
        /// Erasure quota. Omit for unlimited.
        #[arg(long)]
        quota: Option<u64>,
        /// Site id for a site-wide licence. Omit with --machine-fingerprint
        /// for a machine-scoped one.
        #[arg(long)]
        site_id: Option<String>,
        /// Bind to one station's machine fingerprint.
        #[arg(long)]
        machine_fingerprint: Option<String>,
        /// Lease length in days from now.
        #[arg(long, default_value_t = 365)]
        days: i64,
        /// Feature to grant; repeatable. One of: enterprise_mode, hub_sync,
        /// erp_integration, pdf_certificates, cert_anchoring.
        #[arg(long)]
        feature: Vec<String>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Print a licence and, given roots, whether it verifies.
    Inspect {
        license: PathBuf,
        #[arg(long)]
        vendor_root_b64: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Serve {
            addr,
            station_id,
            no_fleet,
            key_path,
            fast,
            static_dir,
            bay_topology,
            bay_profile,
            control_plane_url,
            ephemeral_config,
            license,
            vendor_root_b64,
        } => {
            cmd_serve(ServeArgs {
                addr,
                station_id,
                no_fleet,
                key_path,
                fast,
                static_dir,
                bay_topology,
                bay_profile,
                control_plane_url,
                ephemeral_config,
                license,
                vendor_root_b64,
            })
            .await
        }
        Cmd::VerifyCert {
            cert,
            public_key_b64,
            vendor_root_b64,
            require_licensed,
        } => cmd_verify_cert(cert, public_key_b64, vendor_root_b64, require_licensed),
        Cmd::Inspect => cmd_inspect().await,
        Cmd::BayPresets { dump } => cmd_bay_presets(dump),
        Cmd::License(sub) => cmd_license(sub),
    }
}

/// Load the station's bay topology from an explicit file or a named preset.
///
/// A file that names an unknown schema version, or a preset name we don't
/// know, is a hard error: rendering the wrong bay map is worse than
/// rendering none, because the operator's reason to trust it is that they
/// stop double-checking against the metal.
fn load_bay_topology(
    path: Option<PathBuf>,
    profile: Option<String>,
) -> Result<Option<wipe_common::BayTopology>> {
    if let Some(path) = path {
        // Since ADR-0003 this path is read *and* written, so "not there yet"
        // is a first run rather than a misconfiguration. A file that exists
        // but is wrong is still a hard error.
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!(
                    "bay topology: {} does not exist yet — it will be created on first save",
                    path.display()
                );
                return Ok(None);
            }
            Err(e) => {
                return Err(e).with_context(|| format!("reading bay topology {}", path.display()))
            }
        };
        let topology: wipe_common::BayTopology = serde_json::from_str(&raw)
            .with_context(|| format!("parsing bay topology {}", path.display()))?;
        if topology.schema_version != wipe_common::BAY_TOPOLOGY_SCHEMA_VERSION {
            return Err(anyhow!(
                "bay topology {} declares schema_version {}, but this build understands {}",
                path.display(),
                topology.schema_version,
                wipe_common::BAY_TOPOLOGY_SCHEMA_VERSION
            ));
        }
        let dupes = topology.duplicate_bay_ids();
        if !dupes.is_empty() {
            return Err(anyhow!(
                "bay topology {} has duplicate bay ids: {}",
                path.display(),
                dupes
                    .iter()
                    .map(|b| b.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        println!(
            "bay topology: {} ({} bays) from {}",
            topology.label,
            topology.bay_count(),
            path.display()
        );
        return Ok(Some(topology));
    }

    if let Some(name) = profile {
        let topology = wipe_common::preset(&name).ok_or_else(|| {
            anyhow!(
                "unknown bay profile `{name}` — known presets: {}",
                wipe_common::preset_names().join(", ")
            )
        })?;
        println!(
            "bay topology: {} ({} bays) from preset `{name}`",
            topology.label,
            topology.bay_count()
        );
        return Ok(Some(topology));
    }

    println!("bay topology: not configured — bench will be shown as unconfigured");
    Ok(None)
}

fn cmd_bay_presets(dump: Option<String>) -> Result<()> {
    match dump {
        Some(name) => {
            let topology = wipe_common::preset(&name).ok_or_else(|| {
                anyhow!(
                    "unknown bay profile `{name}` — known presets: {}",
                    wipe_common::preset_names().join(", ")
                )
            })?;
            println!("{}", serde_json::to_string_pretty(&topology)?);
        }
        None => {
            for name in wipe_common::preset_names() {
                let t = wipe_common::preset(name).expect("listed preset resolves");
                println!(
                    "{name:<16} {:>3} bays  {}",
                    t.bay_count(),
                    t.enclosures[0].label
                );
            }
        }
    }
    Ok(())
}

/// Serve's flags outgrew a positional argument list once licensing landed.
struct ServeArgs {
    addr: SocketAddr,
    station_id: Option<String>,
    no_fleet: bool,
    key_path: Option<PathBuf>,
    fast: bool,
    static_dir: Option<PathBuf>,
    bay_topology: Option<PathBuf>,
    bay_profile: Option<String>,
    control_plane_url: Option<String>,
    ephemeral_config: bool,
    license: Option<PathBuf>,
    vendor_root_b64: Vec<String>,
}

async fn cmd_serve(args: ServeArgs) -> Result<()> {
    let ServeArgs {
        addr,
        station_id,
        no_fleet,
        key_path,
        fast,
        static_dir,
        bay_topology: bay_topology_path,
        bay_profile,
        control_plane_url,
        ephemeral_config: force_ephemeral,
        license: license_path,
        vendor_root_b64,
    } = args;
    let seed_topology = load_bay_topology(bay_topology_path.clone(), bay_profile)?;
    let signing_key = Arc::new(load_or_create_signing_key(key_path)?);
    println!(
        "signing key public id: {}",
        signing_key.verifying_key().public_key_id()
    );
    println!(
        "signing key public b64: {}",
        signing_key.verifying_key().to_base64()
    );

    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        if fast {
            MockTiming::fast()
        } else {
            MockTiming::default()
        },
    ));

    let resolved_station_id = station_id.clone().unwrap_or_else(|| "standalone".into());

    let fleet = if no_fleet {
        None
    } else {
        use wipe_common::{StationId, StationInfo, StationRole};
        let id = station_id
            .map(StationId)
            .unwrap_or_else(StationId::new_random);
        let hostname = std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".into());
        let info = StationInfo {
            id,
            hostname,
            role: StationRole::Member,
            version: env!("CARGO_PKG_VERSION").into(),
            api_port: addr.port(),
            started_at: time::OffsetDateTime::now_utc(),
            active_jobs: 0,
            last_seen: None,
        };
        Some(Arc::new(FleetService::start(info)?))
    };

    let resolved_dist = static_dir.or_else(detect_frontend_dist);
    match &resolved_dist {
        Some(p) => println!("serving frontend from {}", p.display()),
        None => println!("no frontend bundle found — UI will show an API-only landing page"),
    }

    // Work out where saved configuration goes before serving, so the tier is
    // in the startup log next to everything else an operator checks (ADR-0003).
    let store: Arc<dyn wipe_server::TopologyStore> = if force_ephemeral {
        Arc::new(wipe_server::store::EphemeralStore::new(
            "Started with --ephemeral-config.".into(),
            false,
        ))
    } else {
        wipe_server::store::detect_store(&wipe_server::store::StoreConfig {
            explicit_path: bay_topology_path,
            control_plane_url,
            station_id: resolved_station_id.clone(),
        })
    };
    let store_status = wipe_server::store::status_of(&store);
    println!(
        "config store: {:?} at {} — {}",
        store_status.tier, store_status.location, store_status.detail
    );
    if store_status.needs_operator_decision {
        println!("  ^ the UI will ask the operator to point at a control plane or accept the loss");
    }

    // ADR-0005: a licence only decides how certificates are *marked*. A
    // station with none still erases and still signs.
    let license_ctx = build_license_context(
        license_path,
        vendor_root_b64,
        &signing_key.public_key_id(),
        &resolved_station_id,
    )?;

    let mut state = AppState::with_static_dir(backend.clone(), fleet, signing_key, resolved_dist)
        .with_license(Arc::new(license_ctx))
        .with_topology_store(store)
        // The mock can fake hot-plug, which is what identify mode is driven
        // by. A real backend will not implement DeviceSimulator and the
        // /api/sim/* routes stay unavailable.
        .with_simulator(backend);
    // A --bay-topology file or --bay-profile seeds the bench only when the
    // store had nothing; a saved layout is the operator's and outranks it.
    if state.bay_topology.read().is_none() {
        state = state.with_bay_topology(seed_topology);
    }
    println!("API listening on http://{addr}");
    serve(state, addr).await?;
    Ok(())
}

/// Walk a small list of likely locations for a built frontend (`index.html`
/// must exist). Tries:
///   * `$WIPESTATION_STATIC_DIR` (handled by clap)
///   * `<cwd>/apps/desktop/dist`
///   * `<cwd>/dist`
///   * `<binary-dir>/../apps/desktop/dist`
///   * `<binary-dir>/static`
fn detect_frontend_dist() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("apps/desktop/dist"));
        candidates.push(cwd.join("dist"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("static"));
            candidates.push(parent.join("../apps/desktop/dist"));
            candidates.push(parent.join("../../apps/desktop/dist"));
            candidates.push(parent.join("../../../apps/desktop/dist"));
        }
    }
    candidates
        .into_iter()
        .find(|p| has_index_html(p))
        .map(|p| p.canonicalize().unwrap_or(p))
}

fn has_index_html(p: &Path) -> bool {
    p.join("index.html").is_file()
}

fn decode_keys(label: &str, keys_b64: Vec<String>) -> Result<Vec<VerifyingKey>> {
    use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine as _};
    let mut out = Vec::new();
    for b64 in keys_b64 {
        let bytes = B64
            .decode(b64.as_bytes())
            .with_context(|| format!("invalid base64 in --{label}: {b64}"))?;
        if bytes.len() != 32 {
            return Err(anyhow!("public key must be 32 bytes (got {})", bytes.len()));
        }
        let arr: [u8; 32] = bytes.try_into().unwrap();
        out.push(
            VerifyingKey::from_bytes(&arr).map_err(|e| anyhow!("invalid public key bytes: {e}"))?,
        );
    }
    Ok(out)
}

fn cmd_verify_cert(
    path: PathBuf,
    public_keys_b64: Vec<String>,
    vendor_roots_b64: Vec<String>,
    require_licensed: bool,
) -> Result<()> {
    let raw = fs::read(&path).with_context(|| format!("reading {path:?}"))?;
    let signed: SignedCertificate = serde_json::from_slice(&raw)
        .with_context(|| format!("parsing {path:?} as SignedCertificate JSON"))?;

    let trusted = decode_keys("public-key-b64", public_keys_b64)?;
    let roots = decode_keys("vendor-root-b64", vendor_roots_b64)?;

    let key_id = wipe_cert::verify(&signed, &trusted)?;
    println!("OK — certificate verified against trusted key {key_id}");
    println!(
        "  job_id: {}\n  device: {} ({})\n  method: {}\n  category: {:?}\n  operator: {} <{}>",
        signed.certificate.job_id,
        signed.certificate.device.model,
        signed.certificate.device.serial,
        signed.certificate.sanitization.method_human,
        signed.certificate.sanitization.category,
        signed.certificate.operator.display_name,
        signed.certificate.operator.email,
    );

    report_attestation(&signed, &roots, require_licensed)
}

/// Report the ADR-0005 attestation chain.
///
/// The erasure signature above proves the payload is intact. This proves who
/// was *entitled* to hold the key that made it — a separate question, and the
/// output keeps them separate so neither can be mistaken for the other.
fn report_attestation(
    signed: &SignedCertificate,
    roots: &[VerifyingKey],
    require_licensed: bool,
) -> Result<()> {
    use wipe_license::{verify_chain, AttestationChain, ChainVerdict};

    let chain: Option<AttestationChain> = match &signed.attestation {
        Some(v) => Some(
            serde_json::from_value(v.clone())
                .context("certificate carries an attestation block we cannot parse")?,
        ),
        None => None,
    };

    if roots.is_empty() {
        match (&chain, signed.certificate.evaluation) {
            (Some(_), _) => println!(
                "\n  attestation: present but NOT CHECKED — pass --vendor-root-b64 to verify it"
            ),
            (None, true) => {
                println!("\n  attestation: none — EVALUATION certificate (unlicensed station)")
            }
            (None, false) => println!(
                "\n  attestation: none, but the certificate does not claim evaluation status"
            ),
        }
        return Ok(());
    }

    let verdict = verify_chain(
        chain.as_ref(),
        &signed.signature.public_key_id,
        signed.certificate.issued_at,
        roots,
    );

    match &verdict {
        ChainVerdict::Licensed {
            customer_id,
            customer_name,
            license_id,
            root_key_id,
            expired_at_signing,
        } => {
            println!("\nLICENSED — attestation chain verified to vendor root {root_key_id}");
            println!("  customer: {customer_name} ({customer_id})");
            println!("  license:  {license_id}");
            if *expired_at_signing {
                println!(
                    "  NOTE: the licence had expired when this certificate was signed. \n\
                             The chain is authentic; the licence had lapsed."
                );
            }
            // Belt and braces: a licensed chain on a cert that marks itself
            // evaluation means the two halves disagree, which is a finding.
            if signed.certificate.evaluation {
                println!(
                    "  WARNING: certificate is marked `evaluation` yet carries a valid chain."
                );
            }
        }
        ChainVerdict::Unlicensed => {
            println!("\nEVALUATION — no attestation chain (unlicensed station)");
            println!(
                "  The erasure and its signature are valid. This certificate was not produced\n\
                 under a vendor licence and is not evidence of a licensed deployment."
            );
            if !signed.certificate.evaluation {
                println!(
                    "  WARNING: no chain, yet the certificate does not mark itself `evaluation`."
                );
            }
            if require_licensed {
                return Err(anyhow!(
                    "--require-licensed was set but this certificate is unlicensed"
                ));
            }
        }
        ChainVerdict::Invalid { reason } => {
            return Err(anyhow!("attestation chain INVALID: {reason}"));
        }
    }
    Ok(())
}

async fn cmd_inspect() -> Result<()> {
    use wipe_engine::DeviceBackend;
    let backend = MockBackend::default_catalog();
    let devices = backend.enumerate().await?;
    let json = serde_json::to_string_pretty(&devices)?;
    println!("{json}");
    Ok(())
}

fn load_or_create_signing_key(path: Option<PathBuf>) -> Result<SigningKey> {
    match path {
        None => Ok(SigningKey::generate()),
        Some(p) => {
            if p.exists() {
                let bytes = fs::read(&p)?;
                if bytes.len() != 32 {
                    return Err(anyhow!(
                        "{p:?} is not a 32-byte Ed25519 seed (got {})",
                        bytes.len()
                    ));
                }
                let arr: [u8; 32] = bytes.try_into().unwrap();
                Ok(SigningKey::from_seed(arr))
            } else {
                let key = SigningKey::generate();
                fs::write(&p, key.0.to_bytes())?;
                Ok(key)
            }
        }
    }
}

/// Build the station's licensing state from `--license` / `--vendor-root-b64`.
///
/// A licence that fails to verify, or that names another station's key, is a
/// hard error at startup rather than a silent downgrade: the operator asked
/// for licensed operation and needs to know they did not get it. Running with
/// *no* licence at all is a different, supported thing — free tier.
fn build_license_context(
    license_path: Option<PathBuf>,
    vendor_root_b64: Vec<String>,
    station_key_id: &str,
    station_id: &str,
) -> Result<wipe_server::LicenseContext> {
    let now = time::OffsetDateTime::now_utc();
    let Some(path) = license_path else {
        println!("licence: none — free tier, certificates will be marked evaluation");
        return Ok(wipe_server::LicenseContext::unlicensed(station_id, now));
    };

    if vendor_root_b64.is_empty() {
        return Err(anyhow!(
            "--license needs --vendor-root-b64: an unverified licence proves nothing"
        ));
    }
    let roots = decode_keys("vendor-root-b64", vendor_root_b64)?;
    let license = wipe_license::load_license(&path).map_err(|e| anyhow!("{e}"))?;
    wipe_license::check_installable(&license, station_key_id, &roots)
        .map_err(|p| anyhow!("cannot install {}: {p}", path.display()))?;

    let ent = &license.body.entitlements;
    println!(
        "licence: {} for {} ({}) — valid {} to {}",
        license.body.license_id,
        ent.customer_name,
        ent.customer_id,
        ent.not_before.date(),
        ent.not_after.date()
    );

    let chain = wipe_license::AttestationChain::new(license);
    let ctx = wipe_server::LicenseContext::licensed(
        station_id,
        chain,
        wipe_license::LeaseState::new(now),
    );
    println!("  {}", ctx.status(now).operator_message());
    Ok(ctx)
}

fn cmd_license(cmd: LicenseCmd) -> Result<()> {
    match cmd {
        LicenseCmd::NewRoot { out } => {
            let root = wipe_license::VendorRoot::generate();
            // Seed bytes, matching the station key format so one loader works
            // for both.
            let seed = root_seed_bytes(&root);
            if let Some(dir) = out.parent() {
                if !dir.as_os_str().is_empty() {
                    fs::create_dir_all(dir)?;
                }
            }
            fs::write(&out, seed)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&out, fs::Permissions::from_mode(0o600))?;
            }
            println!("vendor root seed written to {}", out.display());
            println!("  root key id:  {}", root.public_key_id());
            println!("  PUBLISH THIS: {}", root.verifying_key().to_base64());
            println!(
                "\n  Guard the seed file. Losing it means no new licences; leaking it means\n  \
                 anyone can mint them."
            );
            Ok(())
        }

        LicenseCmd::Issue {
            root_seed,
            instance_key_id,
            license_id,
            customer_id,
            customer_name,
            quota,
            site_id,
            machine_fingerprint,
            days,
            feature,
            out,
        } => {
            let root = wipe_license::VendorRoot::from_signing_key(load_seed(&root_seed)?);
            let now = time::OffsetDateTime::now_utc();

            let scope = match (&site_id, &machine_fingerprint) {
                (Some(s), _) => wipe_license::Scope::Site { site_id: s.clone() },
                (None, Some(f)) => wipe_license::Scope::Machine {
                    fingerprint: f.clone(),
                },
                (None, None) => {
                    return Err(anyhow!(
                        "specify --site-id for a site licence or --machine-fingerprint for a \
                         machine-scoped one"
                    ))
                }
            };

            let features = feature
                .iter()
                .map(|f| parse_feature(f))
                .collect::<Result<Vec<_>>>()?;

            let ent = wipe_license::Entitlements {
                customer_id,
                customer_name,
                quota: match quota {
                    Some(n) => wipe_license::Quota::Count { erasures: n },
                    None => wipe_license::Quota::Unlimited,
                },
                scope,
                not_before: now,
                not_after: now + time::Duration::days(days),
                features,
                allowed_methods: wipe_license::AllowedMethods::All,
                machine_binding: machine_fingerprint,
            };

            let license = root
                .issue(license_id, instance_key_id, ent, now)
                .map_err(|e| anyhow!("{e}"))?;
            wipe_license::save_license(&out, &license).map_err(|e| anyhow!("{e}"))?;
            println!("licence written to {}", out.display());
            println!("  root key id: {}", root.public_key_id());
            println!("  instance:    {}", license.body.instance_public_key_id);
            Ok(())
        }

        LicenseCmd::Inspect {
            license,
            vendor_root_b64,
        } => {
            let lic = wipe_license::load_license(&license).map_err(|e| anyhow!("{e}"))?;
            let ent = &lic.body.entitlements;
            println!("licence:  {}", lic.body.license_id);
            println!("customer: {} ({})", ent.customer_name, ent.customer_id);
            println!("instance: {}", lic.body.instance_public_key_id);
            println!("root:     {}", lic.body.root_key_id);
            println!(
                "valid:    {} .. {}",
                ent.not_before.date(),
                ent.not_after.date()
            );
            println!("quota:    {:?}", ent.quota);
            println!("scope:    {:?}", ent.scope);
            println!("features: {:?}", ent.features);

            if vendor_root_b64.is_empty() {
                println!("\nNOT VERIFIED — pass --vendor-root-b64 to check the vendor signature");
                return Ok(());
            }
            let roots = decode_keys("vendor-root-b64", vendor_root_b64)?;
            match lic.verify(&roots) {
                Ok(id) => println!("\nVERIFIED against vendor root {id}"),
                Err(e) => return Err(anyhow!("licence did NOT verify: {e}")),
            }
            Ok(())
        }
    }
}

/// Recover the 32-byte seed from a freshly generated root.
fn root_seed_bytes(root: &wipe_license::VendorRoot) -> [u8; 32] {
    root.seed_bytes()
}

fn load_seed(path: &Path) -> Result<SigningKey> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "{} is not a 32-byte seed (got {})",
            path.display(),
            bytes.len()
        ));
    }
    let arr: [u8; 32] = bytes.try_into().unwrap();
    Ok(SigningKey::from_seed(arr))
}

fn parse_feature(s: &str) -> Result<wipe_license::Feature> {
    use wipe_license::Feature::*;
    Ok(match s {
        "enterprise_mode" => EnterpriseMode,
        "hub_sync" => HubSync,
        "erp_integration" => ErpIntegration,
        "pdf_certificates" => PdfCertificates,
        "cert_anchoring" => CertAnchoring,
        other => return Err(anyhow!("unknown feature `{other}`")),
    })
}
