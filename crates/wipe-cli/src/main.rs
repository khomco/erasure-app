//! `wipestation` CLI — single binary that can:
//!   * `serve`        — run the Axum API + fleet on a TCP port
//!   * `verify-cert`  — verify a signed certificate JSON against a public key
//!   * `inspect`      — enumerate mock devices and print as JSON
//!   * `enroll`       — register this station with the hub by device
//!     authorization (ADR-0008) and install the licence it is issued
//!   * `bay-presets`  — list or dump bay-topology presets
//!   * `license`      — vendor licensing tools (ADR-0005)

use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use wipe_cert::{SignedCertificate, SigningKey, VerifyingKey};
use wipe_engine_mock::MockBackend;
use wipe_server::serve;

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
        /// Also the only roots a licence installed while the station runs
        /// (`POST /api/license`) is checked against; without one, runtime
        /// install is refused. A request can never add a root.
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
    /// Register this station with an organization on the hub (ADR-0008).
    ///
    /// Prints a code that an owner of the organization approves in the
    /// portal, waits for the approval, checks the licence the hub sends
    /// against --vendor-root-b64 only, saves it, and installs it into the
    /// running station (`POST /api/license`). With no station running, the
    /// saved file can be given to `serve --license`. Uses HTTPS_PROXY and
    /// NO_PROXY to reach an https hub.
    Enroll {
        /// The hub's address, e.g. `https://hub.example.com` (http only on a
        /// loopback host). Defaults to the address an earlier enroll saved
        /// in the station settings.
        #[arg(long)]
        hub: Option<String>,
        /// The station's signing key seed: the file `serve --key-path` uses.
        /// It must already exist; enroll never creates a key.
        #[arg(long)]
        key_path: PathBuf,
        /// Published vendor root public key(s), base64. At least one is
        /// required: the station trusts only these, and refuses a licence
        /// they do not verify, whatever the hub says.
        #[arg(long)]
        vendor_root_b64: Vec<String>,
        /// Station identifier. Must match `serve --station-id` (whose default
        /// is also `standalone`), since a machine licence is bound to it.
        #[arg(long, default_value = "standalone")]
        station_id: String,
        /// A label shown to the owner who approves, e.g. `Bench 3`.
        #[arg(long)]
        label: Option<String>,
        /// Address of the running station to install the licence into.
        #[arg(long, default_value = "http://127.0.0.1:7878")]
        station: String,
        /// Where to save the verified licence. Defaults to
        /// `enrolled-licence.json` in the station's configuration directory
        /// ($WIPESTATION_CONFIG_DIR, else ~/.config/wipestation).
        #[arg(long)]
        license_out: Option<PathBuf>,
    },
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
        cmd @ Cmd::Serve { .. } => cmd_serve(serve_args(cmd)).await,
        Cmd::VerifyCert {
            cert,
            public_key_b64,
            vendor_root_b64,
            require_licensed,
        } => cmd_verify_cert(cert, public_key_b64, vendor_root_b64, require_licensed),
        Cmd::Inspect => cmd_inspect().await,
        Cmd::Enroll {
            hub,
            key_path,
            vendor_root_b64,
            station_id,
            label,
            station,
            license_out,
        } => {
            cmd_enroll(EnrollArgs {
                hub,
                key_path,
                vendor_root_b64,
                station_id,
                label,
                station,
                license_out,
            })
            .await
        }
        Cmd::BayPresets { dump } => cmd_bay_presets(dump),
        Cmd::License(sub) => cmd_license(sub),
    }
}

fn cmd_bay_presets(dump: Option<String>) -> Result<()> {
    match dump {
        Some(name) => {
            let (topology, _) = wipe_server::bootstrap::resolve_profile(&name)?;
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

/// The `serve` subcommand's flags as one struct.
fn serve_args(cmd: Cmd) -> ServeArgs {
    match cmd {
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
        } => ServeArgs {
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
        },
        other => unreachable!("serve_args called with {other:?}"),
    }
}

/// `none`, `review` or `witness`, the policy's serde names.
fn parse_concurrence_policy(s: &str) -> Result<wipe_common::ConcurrencePolicy, String> {
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|_| format!("unknown concurrence policy `{s}`; expected none, review or witness"))
}

async fn cmd_serve(args: ServeArgs) -> Result<()> {
    let addr = args.addr;
    // ADR-0005: the roots are decoded whether or not --license is given: they
    // are also what a licence installed at runtime must verify against
    // (ADR-0008 §4).
    let vendor_roots = decode_keys("vendor-root-b64", args.vendor_root_b64.clone())?;
    let cfg = station_config(args, vendor_roots);
    let state = wipe_server::bootstrap::build_station(cfg, &mut |line| println!("{line}"))?;
    println!("API listening on http://{addr}");
    serve(state, addr).await?;
    Ok(())
}

/// Map `serve`'s flags onto the shared station bootstrap's config.
fn station_config(
    args: ServeArgs,
    vendor_roots: Vec<VerifyingKey>,
) -> wipe_server::bootstrap::StationConfig {
    use wipe_server::bootstrap::{Fleet, Persistence, StationConfig};
    let ServeArgs {
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
        vendor_root_b64: _,
        concurrence_policy,
    } = args;
    StationConfig {
        addr,
        station_id,
        hostname: std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".into()),
        fleet: if no_fleet {
            Fleet::Off
        } else {
            Fleet::Required
        },
        key_path,
        fast,
        static_dir,
        host_static_candidates: Vec::new(),
        bay_topology,
        bay_profile,
        persistence: Persistence::Detect {
            control_plane_url,
            ephemeral_config,
            config_dir: None,
        },
        license,
        vendor_roots,
        concurrence_policy,
    }
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

/// `enroll`'s flags.
struct EnrollArgs {
    hub: Option<String>,
    key_path: PathBuf,
    vendor_root_b64: Vec<String>,
    station_id: String,
    label: Option<String>,
    station: String,
    license_out: Option<PathBuf>,
}

/// `wipestation enroll` (ADR-0008, station-enrollment story 3). Everything
/// that can be refused without the hub (roots, key file, hub address,
/// station address) is refused before any request is sent.
async fn cmd_enroll(args: EnrollArgs) -> Result<()> {
    use wipe_server::hub_client::{
        enroll, install_on_station, Enrollment, HubClient, HubOrigin, ProxySettings,
        StationInstall, SystemClock,
    };
    use wipe_server::store::{
        enrolled_licence_path, load_station_settings, save_enrolled_licence, save_station_settings,
        station_settings_path, StoreConfig,
    };

    if args.vendor_root_b64.is_empty() {
        return Err(anyhow!(
            "enroll needs at least one --vendor-root-b64: this station trusts only the vendor \
             roots it is given, and refuses any licence they do not verify"
        ));
    }
    let roots = decode_keys("vendor-root-b64", args.vendor_root_b64.clone())?;
    let key = load_existing_key(&args.key_path)?;
    let station = args.station.trim_end_matches('/').to_string();
    if !(station.starts_with("http://") || station.starts_with("https://")) {
        return Err(anyhow!(
            "--station must be the running station's http(s) address, such as \
             http://127.0.0.1:7878; got `{}`",
            args.station
        ));
    }

    // The directory the lease store uses when serve is given no
    // --bay-topology: $WIPESTATION_CONFIG_DIR, else ~/.config/wipestation.
    let settings_path = station_settings_path(&StoreConfig::default());
    let raw_hub = match args.hub {
        Some(h) => h,
        None => match load_station_settings(&settings_path) {
            Ok(Some(wipe_server::store::StationSettings {
                hub_origin: Some(h),
            })) => h,
            Ok(_) => {
                return Err(anyhow!(
                    "no hub address: pass --hub <origin> (none is saved in {})",
                    settings_path.display()
                ))
            }
            Err(e) => {
                return Err(anyhow!(
                    "no hub address: pass --hub <origin> ({} could not be read: {e})",
                    settings_path.display()
                ))
            }
        },
    };
    let origin = HubOrigin::parse(&raw_hub).map_err(|e| anyhow!("{e}"))?;
    let licence_out = args
        .license_out
        .clone()
        .unwrap_or_else(|| enrolled_licence_path(&settings_path));
    let client =
        HubClient::new(origin.clone(), &ProxySettings::from_env()).map_err(|e| anyhow!("{e}"))?;

    println!(
        "Enrolling station `{}` (instance key {}) with the hub at {origin}",
        args.station_id,
        key.public_key_id()
    );
    let who = Enrollment {
        key: &key,
        station_id: &args.station_id,
        label: args.label.as_deref(),
        roots: &roots,
    };
    let enrolled = enroll(&client, &who, &mut SystemClock::default(), &mut |s| {
        println!();
        println!("An owner of your organization approves this station in the portal.");
        println!("Open this link on any phone or computer signed in to the portal:");
        println!("open: {}", s.verification_uri_complete);
        println!("or go to the address and enter the code:");
        println!("address: {}", s.verification_uri);
        println!("user code: {}", s.user_code);
        println!();
        println!(
            "The code expires in {} minutes. Waiting for approval...",
            s.expires_in.div_ceil(60)
        );
    })
    .await
    .map_err(|e| anyhow!("{e}"))?;

    let body = &enrolled.licence.body;
    println!(
        "Approved: licence {} for {} ({}), valid {} to {}, verified against the vendor root.",
        body.license_id,
        body.entitlements.customer_name,
        body.entitlements.customer_id,
        body.entitlements.not_before.date(),
        body.entitlements.not_after.date()
    );

    let saved = save_enrolled_licence(&licence_out, &enrolled.licence_json);
    match &saved {
        Ok(()) => println!("Licence saved to {}", licence_out.display()),
        Err(e) => println!("warning: the licence could not be saved ({e})"),
    }

    let mut settings = load_station_settings(&settings_path)
        .ok()
        .flatten()
        .unwrap_or_default();
    settings.hub_origin = Some(origin.to_string());
    match save_station_settings(&settings_path, &settings) {
        Ok(()) => println!("Hub address saved to {}", settings_path.display()),
        Err(e) => println!(
            "The hub address was not saved: {} cannot be written ({e}). Pass --hub again next \
             time.",
            settings_path.display()
        ),
    }

    match install_on_station(&station, &enrolled.licence_json).await {
        StationInstall::Installed { operator_message } => {
            println!("Installed into the station at {station}: {operator_message}");
            match &saved {
                Ok(()) => println!(
                    "The station holds it until it restarts; to keep it, start serve with \
                     --license {}.",
                    licence_out.display()
                ),
                Err(_) => {
                    println!(
                        "The station holds it until it restarts. The licence could not be saved; \
                         keep its JSON, which follows, to start serve with --license:"
                    );
                    println!("{}", enrolled.licence_json);
                }
            }
            Ok(())
        }
        StationInstall::NoStation { reason } => {
            println!("No station answered at {station} ({reason}), so nothing was installed now.");
            if saved.is_err() {
                println!("The licence, which could not be saved, follows:");
                println!("{}", enrolled.licence_json);
                return Err(anyhow!(
                    "the licence could be neither saved nor installed; its JSON is printed above"
                ));
            }
            let roots = args
                .vendor_root_b64
                .iter()
                .map(|r| format!(" --vendor-root-b64 {r}"))
                .collect::<String>();
            println!(
                "It is saved at {path}. Start the station with it:\n  wipestation serve \
                 --license {path}{roots} --key-path {key} --station-id {id}",
                path = licence_out.display(),
                key = args.key_path.display(),
                id = args.station_id,
            );
            Ok(())
        }
        StationInstall::Refused { status, error } => {
            let kept = match &saved {
                Ok(()) => format!(" The licence is saved at {}.", licence_out.display()),
                Err(_) => {
                    println!("The licence, which could not be saved, follows:");
                    println!("{}", enrolled.licence_json);
                    " The licence could not be saved; its JSON is printed above.".to_string()
                }
            };
            Err(anyhow!(
                "the station at {station} refused the licence (HTTP {status}): {error}{kept}"
            ))
        }
    }
}

/// The station's signing key from `path`, which must already exist and hold
/// a 32-byte seed. Unlike `serve`, never creates one: a new key would be a
/// different station.
fn load_existing_key(path: &Path) -> Result<SigningKey> {
    match fs::symlink_metadata(path) {
        Ok(_) => load_seed(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(anyhow!(
            "the signing key {} does not exist. enroll uses the station's existing key (the \
             file `serve --key-path` reads) and never creates one; start `wipestation serve \
             --key-path {}` once to create it",
            path.display(),
            path.display()
        )),
        Err(e) => Err(e).with_context(|| format!("reading signing key {}", path.display())),
    }
}

async fn cmd_inspect() -> Result<()> {
    use wipe_engine::DeviceBackend;
    let backend = MockBackend::default_catalog();
    let devices = backend.enumerate().await?;
    let json = serde_json::to_string_pretty(&devices)?;
    println!("{json}");
    Ok(())
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

    fn serve_config(args: &[&str]) -> wipe_server::bootstrap::StationConfig {
        let argv = ["wipestation", "serve"].iter().chain(args).copied();
        station_config(
            serve_args(Cli::try_parse_from(argv).unwrap().cmd),
            Vec::new(),
        )
    }

    #[test]
    fn serve_flags_map_onto_the_station_config() {
        use wipe_server::bootstrap::{Fleet, Persistence};
        let cfg = serve_config(&[
            "--no-fleet",
            "--ephemeral-config",
            "--bay-topology",
            "X",
            "--station-id",
            "S",
            "--control-plane-url",
            "U",
            "--addr",
            "127.0.0.1:7999",
            "--key-path",
            "K",
            "--fast",
            "--static-dir",
            "D",
            "--bay-profile",
            "P",
            "--license",
            "L",
        ]);
        assert_eq!(cfg.addr, "127.0.0.1:7999".parse().unwrap());
        assert_eq!(cfg.fleet, Fleet::Off);
        assert_eq!(cfg.station_id.as_deref(), Some("S"));
        assert_eq!(cfg.bay_topology, Some(PathBuf::from("X")));
        assert_eq!(cfg.key_path, Some(PathBuf::from("K")));
        assert!(cfg.fast);
        assert_eq!(cfg.static_dir, Some(PathBuf::from("D")));
        assert_eq!(cfg.bay_profile.as_deref(), Some("P"));
        assert_eq!(cfg.license, Some(PathBuf::from("L")));
        assert!(cfg.host_static_candidates.is_empty());
        match cfg.persistence {
            Persistence::Detect {
                control_plane_url,
                ephemeral_config,
                config_dir,
            } => {
                assert_eq!(control_plane_url.as_deref(), Some("U"));
                assert!(ephemeral_config);
                assert!(config_dir.is_none());
            }
            other => panic!("expected Detect, got {other:?}"),
        }

        // Env twins would leak into the defaults; keep this half hermetic.
        if std::env::var_os("WIPESTATION_CONTROL_PLANE_URL").is_none()
            && std::env::var_os("WIPESTATION_BAY_TOPOLOGY").is_none()
        {
            let cfg = serve_config(&[]);
            assert_eq!(cfg.fleet, Fleet::Required);
            assert!(cfg.bay_topology.is_none());
            match cfg.persistence {
                Persistence::Detect {
                    control_plane_url,
                    ephemeral_config,
                    config_dir,
                } => {
                    assert!(control_plane_url.is_none());
                    assert!(!ephemeral_config);
                    assert!(config_dir.is_none());
                }
                other => panic!("expected Detect, got {other:?}"),
            }
        }
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
