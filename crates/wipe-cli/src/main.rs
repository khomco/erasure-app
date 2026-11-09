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
        /// Bay-topology preset, or a catalog model id (ADR-0004), to start
        /// from. Both expand into the same model a config file uses, with all
        /// bays unbound; run `wipestation bay-presets` to list them and to
        /// dump one as a starting point.
        #[arg(long)]
        bay_profile: Option<String>,
        /// Base URL of a control plane to hold this station's configuration
        /// when the station has no writable storage (ADR-0003 tier 2). The
        /// hub itself is future work; with no writable path and no reachable
        /// control plane the station runs ephemeral and says so.
        #[arg(long, env = "WIPESTATION_CONTROL_PLANE_URL")]
        control_plane_url: Option<String>,
        /// Skip store detection and keep configuration in RAM only. For
        /// demos and for reproducing what a PXE station sees. Licence lease
        /// state is still saved to `lease-state.json` in the config
        /// directory when it is writable.
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
        /// Whether destruction runs need a second signer before their Jobs
        /// are destroyed (ADR-0011 §3): `none` (the standards' default),
        /// `review` or `witness`. Set here, never edited from the bench.
        #[arg(
            long,
            env = "WIPESTATION_CONCURRENCE_POLICY",
            default_value = "none",
            value_parser = parse_concurrence_policy
        )]
        concurrence_policy: wipe_common::ConcurrencePolicy,
    },
    /// List built-in bay-topology presets and known enclosure models, or dump
    /// one as JSON to use as a starting point for a station config file.
    BayPresets {
        /// Print this preset or catalog model as JSON instead of listing names.
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
        /// erp_integration, pdf_certificates, cert_anchoring, operator_sso.
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
            concurrence_policy,
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
                concurrence_policy,
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
        let (topology, source) = resolve_profile(&name)?;
        println!(
            "bay topology: {} ({} bays) from {source} `{name}`",
            topology.label,
            topology.bay_count()
        );
        return Ok(Some(topology));
    }

    println!("bay topology: not configured — bench will be shown as unconfigured");
    Ok(None)
}

/// Resolve a `--bay-profile` name: a built-in preset, or a catalog model id.
///
/// The catalog is a source of defaults (ADR-0004), so a station should be able
/// to start from one without an operator hand-writing the same JSON the
/// builder would have produced. Either way the bays start unbound — the
/// catalog knows the shape of a chassis, never which drive is in which bay.
fn resolve_profile(name: &str) -> Result<(wipe_common::BayTopology, &'static str)> {
    if let Some(topology) = wipe_common::preset(name) {
        return Ok((topology, "preset"));
    }

    let catalog = wipe_common::Catalog::bundled();
    if let Some(model) = catalog.get(&wipe_common::ModelId(name.to_string())) {
        return Ok((
            wipe_common::BayTopology {
                schema_version: wipe_common::BAY_TOPOLOGY_SCHEMA_VERSION,
                label: model.display_name(),
                generated: false,
                revision: 0,
                auto_fill_unbound: true,
                enclosures: vec![model.expand("enc1")],
            },
            "catalog model",
        ));
    }

    Err(anyhow!(
        "unknown bay profile `{name}` — presets: {}; catalog models: {}",
        wipe_common::preset_names().join(", "),
        catalog
            .models
            .iter()
            .map(|m| m.id.0.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn cmd_bay_presets(dump: Option<String>) -> Result<()> {
    match dump {
        Some(name) => {
            let (topology, _) = resolve_profile(&name)?;
            println!("{}", serde_json::to_string_pretty(&topology)?);
        }
        None => {
            println!("presets (starting points, not a hardware compatibility list):");
            for name in wipe_common::preset_names() {
                let t = wipe_common::preset(name).expect("listed preset resolves");
                println!(
                    "  {name:<24} {:>3} bays  {}",
                    t.bay_count(),
                    t.enclosures[0].label
                );
            }
            // Listed here because --bay-profile accepts either, and an
            // operator should not have to know which list a name came from.
            println!("\ncatalog models (ADR-0004):");
            for m in &wipe_common::Catalog::bundled().models {
                println!(
                    "  {:<24} {:>3} bays  {}{}",
                    m.id.0,
                    m.bay_count(),
                    m.display_name(),
                    if m.verified_by.is_some() {
                        " [verified]"
                    } else {
                        ""
                    }
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
    concurrence_policy: wipe_common::ConcurrencePolicy,
}

/// `none`, `review` or `witness`, the policy's serde names.
fn parse_concurrence_policy(s: &str) -> Result<wipe_common::ConcurrencePolicy, String> {
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|_| format!("unknown concurrence policy `{s}`; expected none, review or witness"))
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
        concurrence_policy,
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
    let store_cfg = wipe_server::store::StoreConfig {
        explicit_path: bay_topology_path,
        control_plane_url,
        station_id: resolved_station_id.clone(),
    };
    let (store, lease_store) = serve_stores(&store_cfg, force_ephemeral);
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
        lease_store,
    )?;
    println!("{}", lease_store_line(&license_ctx.lease_persistence()));
    println!("{}", lease_counts_line(&license_ctx.lease_state()));

    let mut state = AppState::with_static_dir(backend.clone(), fleet, signing_key, resolved_dist)
        .with_license(Arc::new(license_ctx))
        .with_topology_store(store)
        .with_concurrence_policy(concurrence_policy)
        // The mock can fake hot-plug, which is what identify mode is driven
        // by. The /api/sim/* routes are always mounted; without this call
        // they answer 404.
        .with_simulator(backend);
    // A --bay-topology file or --bay-profile seeds the bench only when the
    // store had nothing; a saved layout is the operator's and outranks it.
    if state.bay_topology.read().is_none() {
        state = state.with_bay_topology(seed_topology);
    }
    println!(
        "destruction concurrence policy: {}",
        serde_json::to_value(concurrence_policy)?
            .as_str()
            .unwrap_or_default()
    );
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
    print!("{}", describe_certificate(&signed.certificate));

    report_attestation(&signed, &roots, require_licensed)
}

/// The verified certificate as `verify-cert` prints it: the headline, then,
/// for a destroyed drive, who attested what and when (ADR-0011).
fn describe_certificate(cert: &wipe_cert::Certificate) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let device = match &cert.device {
        Some(d) => format!("{} ({})", d.model, d.serial),
        None => "not read from the device (never erased)".to_owned(),
    };
    let category = match cert.sanitization.category {
        Some(c) => format!("{c:?}"),
        None => "none (not NIST Destroy)".to_owned(),
    };
    let _ = writeln!(
        out,
        "  format: v{}\n  job_id: {}\n  disposition: {}\n  device: {device}\n  method: {}\n  \
         category: {category}\n  operator: {} <{}>",
        cert.cert_format_version,
        cert.job_id,
        serde_name(&cert.disposition),
        cert.sanitization.method_human,
        cert.operator.display_name,
        cert.operator.email,
    );
    let Some(rec) = &cert.destruction else {
        return out;
    };
    let _ = writeln!(out, "\n  destruction run: {}", rec.manifest_id);
    let _ = writeln!(
        out,
        "    technique: {} — {}",
        rec.technique.human_name(),
        rec.standard_label
    );
    if let Some(caution) = &rec.caution {
        let _ = writeln!(out, "    caution: {caution}");
    }
    let attestation = cert.activities.iter().find_map(|a| match a {
        wipe_common::JobActivity::Destruction(e) if e.id == rec.destruction_event_id => {
            e.attestation.as_ref()
        }
        _ => None,
    });
    if let Some(a) = attestation {
        let ids = &a.identifiers;
        let _ = writeln!(
            out,
            "    recorded for destruction: {}, {} ({}) at {}\n      equipment {}, location \
             {}\n      identifiers: asset tag {}, serial {} (source: {})\n      statement: {}",
            a.signer.full_name,
            a.signer.title,
            assurance(&a.signer.identity_assurance),
            rfc3339(a.at),
            a.equipment_id,
            a.location,
            ids.asset_tag.as_deref().unwrap_or("none"),
            ids.serial.as_deref().unwrap_or("none"),
            serde_name(&ids.source),
            a.statement,
        );
    }
    let c = &rec.completion;
    let _ = writeln!(
        out,
        "    completed: {} by {}, {} ({})\n      verification: remnants inspected, equipment {} \
         completed its cycle; criterion: {}\n      video: {}\n      statement: {}",
        rfc3339(c.completed_at),
        c.signer.full_name,
        c.signer.title,
        assurance(&c.signer.identity_assurance),
        c.verification.equipment_id,
        c.verification.criterion,
        c.video_ref.as_deref().unwrap_or("none"),
        c.statement,
    );
    match &rec.concurrence {
        Some(k) => {
            let _ = writeln!(
                out,
                "    concurrence: {} by {}, {} ({}) at {}\n      statement: {}",
                serde_name(&k.kind),
                k.signer.full_name,
                k.signer.title,
                assurance(&k.signer.identity_assurance),
                rfc3339(k.at),
                k.statement,
            );
        }
        None => {
            let _ = writeln!(out, "    concurrence: none required");
        }
    }
    out
}

fn assurance(a: &wipe_common::IdentityAssurance) -> String {
    match a {
        wipe_common::IdentityAssurance::SelfAsserted => "self-asserted".to_owned(),
        wipe_common::IdentityAssurance::Verified { issuer, subject } => {
            format!("verified by {issuer}, subject {subject}")
        }
    }
}

fn serde_name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| at.to_string())
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

/// The topology store and the lease store `wipestation serve` runs with.
///
/// `--ephemeral-config` keeps *configuration* in RAM; lease state is detected
/// whatever it says, because lease state is licensing enforcement, not
/// configuration (ADR-0005 §3a). That only stops this flag from refilling a
/// quota: deleting or moving `lease-state.json`, or pointing `--bay-topology`
/// or `WIPESTATION_CONFIG_DIR` at an empty directory, still starts a fresh
/// lease — the speed bump ADR-0005 §3a admits.
fn serve_stores(
    cfg: &wipe_server::store::StoreConfig,
    force_ephemeral: bool,
) -> (
    Arc<dyn wipe_server::TopologyStore>,
    Arc<dyn wipe_server::LeaseStore>,
) {
    let topology: Arc<dyn wipe_server::TopologyStore> = if force_ephemeral {
        Arc::new(wipe_server::store::EphemeralStore::new(
            "Started with --ephemeral-config.".into(),
            false,
        ))
    } else {
        wipe_server::store::detect_store(cfg)
    };
    (topology, wipe_server::detect_lease_store(cfg))
}

/// The start-up line saying where lease state lives and whether it survives
/// reboot. Same shape as the `config store:` line above it.
fn lease_store_line(lease: &wipe_server::LeasePersistence) -> String {
    format!(
        "lease store: {:?} at {} — {}",
        lease.tier, lease.location, lease.detail
    )
}

/// The start-up line giving the local erasure counts (ADR-0005 §5): licensed
/// erasures under the licence the stored count belongs to (which, on an
/// unlicensed start, is the last licence seen, if any), and evaluation
/// erasures over the station's life. A count only — no limit or warning is
/// attached to it.
fn lease_counts_line(lease: &wipe_license::LeaseState) -> String {
    let licensed = match &lease.license_id {
        Some(id) => format!(
            "{} licensed erasure(s) under licence {id}",
            lease.erasures_used
        ),
        None => "no licensed erasures (no licence seen)".to_string(),
    };
    format!(
        "lease counts: {licensed}, {} evaluation erasure(s) on this station",
        lease.evaluation_erasures
    )
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
    lease_store: Arc<dyn wipe_server::LeaseStore>,
) -> Result<wipe_server::LicenseContext> {
    let now = time::OffsetDateTime::now_utc();
    let Some(path) = license_path else {
        println!("licence: none — free tier, certificates will be marked evaluation");
        return Ok(wipe_server::LicenseContext::open(
            station_id,
            None,
            lease_store,
            now,
        ));
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
    let ctx = wipe_server::LicenseContext::open(station_id, Some(chain), lease_store, now);
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
        "operator_sso" => OperatorSso,
        other => return Err(anyhow!("unknown feature `{other}`")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wipe_license::Feature;

    /// Exhaustive: a new `Feature` variant fails to compile here until it is
    /// listed, and then fails the test until `parse_feature` accepts it.
    fn all_features() -> Vec<Feature> {
        let all = vec![
            Feature::EnterpriseMode,
            Feature::HubSync,
            Feature::ErpIntegration,
            Feature::PdfCertificates,
            Feature::CertAnchoring,
            Feature::OperatorSso,
        ];
        for f in &all {
            match f {
                Feature::EnterpriseMode
                | Feature::HubSync
                | Feature::ErpIntegration
                | Feature::PdfCertificates
                | Feature::CertAnchoring
                | Feature::OperatorSso => {}
            }
        }
        all
    }

    fn lease_tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wipestation-cli-lease-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store_cfg_in(dir: &std::path::Path) -> wipe_server::store::StoreConfig {
        wipe_server::store::StoreConfig {
            explicit_path: Some(dir.join("bay-topology.json")),
            control_plane_url: None,
            station_id: "station-1".into(),
        }
    }

    #[test]
    fn ephemeral_config_keeps_topology_in_ram_but_still_persists_lease_state() {
        let dir = lease_tmpdir("ephemeral-config");
        let cfg = store_cfg_in(&dir);

        let (topology, lease) = serve_stores(&cfg, true);
        assert_eq!(topology.tier(), wipe_server::StoreTier::Ephemeral);
        assert_eq!(lease.tier(), wipe_server::StoreTier::LocalFile);
        assert_eq!(
            lease.location(),
            dir.join("lease-state.json").display().to_string()
        );

        let (topology, lease) = serve_stores(&cfg, false);
        assert_eq!(topology.tier(), wipe_server::StoreTier::LocalFile);
        assert_eq!(lease.tier(), wipe_server::StoreTier::LocalFile);
    }

    #[test]
    fn an_unlicensed_station_saves_its_lease_after_a_signing_decision() {
        let dir = lease_tmpdir("unlicensed");
        let path = dir.join("lease-state.json");
        let store = Arc::new(wipe_server::LocalFileLeaseStore::new(path.clone()));
        let ctx =
            build_license_context(None, Vec::new(), "ed25519:test", "station-1", store).unwrap();
        assert_eq!(
            ctx.lease_persistence().tier,
            wipe_server::StoreTier::LocalFile
        );

        let decision = ctx.signing_decision(time::OffsetDateTime::now_utc());
        assert!(decision.evaluation);
        let saved: wipe_license::LeaseState =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.erasures_used, 0);

        let line = lease_store_line(&ctx.lease_persistence());
        assert!(line.starts_with("lease store: LocalFile at "), "{line}");
        assert!(line.contains(&path.display().to_string()), "{line}");
    }

    #[test]
    fn the_start_up_counts_line_reports_an_unlicensed_stations_evaluation_erasures() {
        let dir = lease_tmpdir("counts-line");
        let path = dir.join("lease-state.json");
        let mut seeded = wipe_license::LeaseState::new(time::OffsetDateTime::now_utc());
        seeded.evaluation_erasures = 3;
        let store = Arc::new(wipe_server::LocalFileLeaseStore::new(path));
        wipe_server::LeaseStore::save(store.as_ref(), &seeded).unwrap();
        let ctx =
            build_license_context(None, Vec::new(), "ed25519:test", "station-1", store).unwrap();
        assert_eq!(
            lease_counts_line(&ctx.lease_state()),
            "lease counts: no licensed erasures (no licence seen), 3 evaluation erasure(s) on \
             this station"
        );
    }

    #[test]
    fn the_start_up_counts_line_names_the_licence_a_stored_count_belongs_to() {
        let dir = lease_tmpdir("counts-line-licence");
        let path = dir.join("lease-state.json");
        let mut seeded = wipe_license::LeaseState::new(time::OffsetDateTime::now_utc());
        seeded.erasures_used = 5;
        seeded.evaluation_erasures = 2;
        seeded.license_id = Some("lic-old".into());
        let store = Arc::new(wipe_server::LocalFileLeaseStore::new(path));
        wipe_server::LeaseStore::save(store.as_ref(), &seeded).unwrap();
        // Unlicensed start: the stored count is kept, and named by its licence.
        let ctx =
            build_license_context(None, Vec::new(), "ed25519:test", "station-1", store).unwrap();
        assert_eq!(
            lease_counts_line(&ctx.lease_state()),
            "lease counts: 5 licensed erasure(s) under licence lic-old, 2 evaluation \
             erasure(s) on this station"
        );
    }

    #[test]
    fn a_licensed_station_starts_from_its_saved_lease_count() {
        let dir = lease_tmpdir("licensed");
        let now = time::OffsetDateTime::now_utc();
        let root = wipe_license::VendorRoot::generate();
        let ent = wipe_license::Entitlements {
            customer_id: "c".into(),
            customer_name: "C".into(),
            quota: wipe_license::Quota::Count { erasures: 10 },
            scope: wipe_license::Scope::Site {
                site_id: "s".into(),
            },
            not_before: now - time::Duration::days(1),
            not_after: now + time::Duration::days(30),
            features: vec![],
            allowed_methods: wipe_license::AllowedMethods::All,
            machine_binding: None,
        };
        let license = root.issue("lic-cli", "ed25519:test", ent, now).unwrap();
        let license_path = dir.join("licence.json");
        wipe_license::save_license(&license_path, &license).unwrap();

        let lease_path = dir.join("lease-state.json");
        let mut seeded = wipe_license::LeaseState::new(now - time::Duration::hours(1));
        seeded.erasures_used = 2;
        seeded.license_id = Some("lic-cli".into());
        let store = Arc::new(wipe_server::LocalFileLeaseStore::new(lease_path));
        wipe_server::LeaseStore::save(store.as_ref(), &seeded).unwrap();

        let ctx = build_license_context(
            Some(license_path),
            vec![root.verifying_key().to_base64()],
            "ed25519:test",
            "station-1",
            store,
        )
        .unwrap();
        assert_eq!(ctx.lease_state().erasures_used, 2);
        assert_eq!(
            ctx.lease_persistence().tier,
            wipe_server::StoreTier::LocalFile
        );
    }

    #[cfg(unix)]
    #[test]
    fn serve_stores_on_a_read_only_directory_runs_the_lease_in_ram_and_says_so() {
        use std::os::unix::fs::PermissionsExt;
        let dir = lease_tmpdir("read-only");
        let set = |mode| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode));
        set(0o500).unwrap();
        let probe = dir.join(".mode-probe");
        if std::fs::write(&probe, b"x").is_ok() {
            let _ = std::fs::remove_file(&probe);
            set(0o700).unwrap();
            return; // mode bits do not stop this user (root)
        }
        let (_, lease) = serve_stores(&store_cfg_in(&dir), false);
        set(0o700).unwrap();
        assert_eq!(lease.tier(), wipe_server::StoreTier::Ephemeral);
        let ctx =
            build_license_context(None, Vec::new(), "ed25519:test", "station-1", lease).unwrap();
        let line = lease_store_line(&ctx.lease_persistence());
        assert!(
            line.contains("quota count and clock-rollback protection reset on reboot"),
            "{line}"
        );
    }

    #[test]
    fn the_start_up_line_says_what_a_ram_only_lease_loses_on_reboot() {
        let store = Arc::new(wipe_server::EphemeralLeaseStore::new(
            "This station cannot write /ro.".into(),
        ));
        let ctx =
            build_license_context(None, Vec::new(), "ed25519:test", "station-1", store).unwrap();
        let line = lease_store_line(&ctx.lease_persistence());
        assert!(line.starts_with("lease store: Ephemeral at "), "{line}");
        assert!(
            line.contains("quota count and clock-rollback protection reset on reboot"),
            "{line}"
        );
    }

    #[test]
    fn parse_feature_accepts_every_feature_serde_name() {
        for f in all_features() {
            let name = serde_json::to_value(f).unwrap();
            let name = name.as_str().unwrap();
            assert_eq!(parse_feature(name).unwrap(), f, "{name}");
        }
        assert!(parse_feature("no_such_feature").is_err());
    }

    const STATION_KEY_B64: &str = "GX9rI+FshTLGq8g4+s1ep4m+DHaykgM0A5v6iz02jWE";
    const SUPERVISOR_KEY_B64: &str = "RQigeqlBcH8+stuUyIl6gLLBGXR2tt4hOsJz332GxP8";

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../wipe-cert/tests/fixtures")
            .join(name)
    }

    fn verify_fixture(path: PathBuf) -> Result<()> {
        cmd_verify_cert(path, vec![STATION_KEY_B64.into()], Vec::new(), false)
    }

    #[test]
    fn verify_cert_accepts_the_v2_and_v3_fixtures() {
        verify_fixture(fixture("v2-erased.json")).unwrap();
        verify_fixture(fixture("v2-destroyed-cosigned.json")).unwrap();
        verify_fixture(fixture("v3-destroyed.json")).unwrap();
        // The supervisor key alone does not verify the station's signature.
        let err = cmd_verify_cert(
            fixture("v2-erased.json"),
            vec![SUPERVISOR_KEY_B64.into()],
            Vec::new(),
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("no trusted key"), "{err}");
    }

    #[test]
    fn verify_cert_describes_a_destroyed_drive() {
        let raw = fs::read(fixture("v3-destroyed.json")).unwrap();
        let signed: SignedCertificate = serde_json::from_slice(&raw).unwrap();
        let out = describe_certificate(&signed.certificate);
        let rec = signed.certificate.destruction.as_ref().unwrap();
        for needle in [
            "format: v3",
            "disposition: destroyed",
            "device: not read from the device",
            "technique: Shred — NIST SP 800-88 Destroy",
            wipe_common::SHRED_PULVERIZE_CAUTION,
            "recorded for destruction: Ana Ruiz, Bench lead (self-asserted)",
            "(source: scanned)",
            "criterion: particles no larger than 2 mm",
            "concurrence: none required",
        ] {
            assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
        }
        assert!(out.contains(&rfc3339(rec.completion.completed_at)), "{out}");
        let v2 = fs::read(fixture("v2-erased.json")).unwrap();
        let v2: SignedCertificate = serde_json::from_slice(&v2).unwrap();
        let out = describe_certificate(&v2.certificate);
        assert!(
            out.contains("format: v2") && out.contains("device: MZ-V9P2T0"),
            "{out}"
        );
        assert!(!out.contains("destruction run"));
    }

    #[test]
    fn verify_cert_describes_a_witnessed_run() {
        verify_fixture(fixture("v3-destroyed-witness.json")).unwrap();
        let raw = fs::read(fixture("v3-destroyed-witness.json")).unwrap();
        let signed: SignedCertificate = serde_json::from_slice(&raw).unwrap();
        let out = describe_certificate(&signed.certificate);
        assert!(
            out.contains("concurrence: witness by Bo Chen, Site supervisor (self-asserted)"),
            "{out}"
        );
    }

    #[test]
    fn verify_cert_refuses_a_re_signed_tampered_v3_certificate() {
        let raw = fs::read(fixture("v3-destroyed.json")).unwrap();
        let signed: SignedCertificate = serde_json::from_slice(&raw).unwrap();
        let mut cert = signed.certificate;
        cert.destruction.as_mut().unwrap().completion.statement =
            "Destruction was completed. Trust me.".into();
        let forged = wipe_cert::sign(cert, &SigningKey::from_seed([42; 32])).unwrap();
        let path =
            std::env::temp_dir().join(format!("wipestation-forged-{}.json", std::process::id()));
        fs::write(&path, serde_json::to_vec(&forged).unwrap()).unwrap();
        let err = verify_fixture(path.clone()).unwrap_err();
        let _ = fs::remove_file(&path);
        assert!(err.to_string().contains("completion statement"), "{err}");
    }

    fn serve_policy(args: &[&str]) -> Result<wipe_common::ConcurrencePolicy, clap::Error> {
        let argv = ["wipestation", "serve"].iter().chain(args).copied();
        match Cli::try_parse_from(argv)?.cmd {
            Cmd::Serve {
                concurrence_policy, ..
            } => Ok(concurrence_policy),
            other => panic!("parsed {other:?}"),
        }
    }

    #[test]
    fn the_concurrence_policy_flag_parses_and_defaults_to_none() {
        use wipe_common::ConcurrencePolicy;
        // The env twin would override the default; keep this test hermetic.
        if std::env::var_os("WIPESTATION_CONCURRENCE_POLICY").is_none() {
            assert_eq!(serve_policy(&[]).unwrap(), ConcurrencePolicy::None);
        }
        for (name, policy) in [
            ("none", ConcurrencePolicy::None),
            ("review", ConcurrencePolicy::Review),
            ("witness", ConcurrencePolicy::Witness),
        ] {
            assert_eq!(
                serve_policy(&["--concurrence-policy", name]).unwrap(),
                policy
            );
        }
        let err = serve_policy(&["--concurrence-policy", "supervisor"]).unwrap_err();
        assert!(err.to_string().contains("expected none, review or witness"));
    }
}
