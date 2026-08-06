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
    },
    /// Print the catalog of devices the mock backend would expose.
    Inspect,
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
        } => {
            cmd_serve(
                addr,
                station_id,
                no_fleet,
                key_path,
                fast,
                static_dir,
                bay_topology,
                bay_profile,
            )
            .await
        }
        Cmd::VerifyCert {
            cert,
            public_key_b64,
        } => cmd_verify_cert(cert, public_key_b64),
        Cmd::Inspect => cmd_inspect().await,
        Cmd::BayPresets { dump } => cmd_bay_presets(dump),
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
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("reading bay topology {}", path.display()))?;
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

// Serve takes one argument per CLI flag by design; grouping them into a
// struct would just move the same list somewhere else.
#[allow(clippy::too_many_arguments)]
async fn cmd_serve(
    addr: SocketAddr,
    station_id: Option<String>,
    no_fleet: bool,
    key_path: Option<PathBuf>,
    fast: bool,
    static_dir: Option<PathBuf>,
    bay_topology_path: Option<PathBuf>,
    bay_profile: Option<String>,
) -> Result<()> {
    let bay_topology = load_bay_topology(bay_topology_path, bay_profile)?;
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

    let state = AppState::with_static_dir(backend, fleet, signing_key, resolved_dist)
        .with_bay_topology(bay_topology);
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

fn cmd_verify_cert(path: PathBuf, public_keys_b64: Vec<String>) -> Result<()> {
    use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine as _};
    let raw = fs::read(&path).with_context(|| format!("reading {path:?}"))?;
    let signed: SignedCertificate = serde_json::from_slice(&raw)
        .with_context(|| format!("parsing {path:?} as SignedCertificate JSON"))?;

    let mut trusted = Vec::new();
    for b64 in public_keys_b64 {
        let bytes = B64
            .decode(b64.as_bytes())
            .with_context(|| format!("invalid base64 in --public-key-b64: {b64}"))?;
        if bytes.len() != 32 {
            return Err(anyhow!("public key must be 32 bytes (got {})", bytes.len()));
        }
        let arr: [u8; 32] = bytes.try_into().unwrap();
        let vk =
            VerifyingKey::from_bytes(&arr).map_err(|e| anyhow!("invalid public key bytes: {e}"))?;
        trusted.push(vk);
    }

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
