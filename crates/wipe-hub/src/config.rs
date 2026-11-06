//! Hub configuration: flags and `WIPE_HUB_*` env vars, validated at start-up.
//!
//! Defaults match the root `compose.yaml`, so `docker compose up -d` followed
//! by `wipe-hub serve` with no flags is a working local setup.

use std::fmt;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use thiserror::Error;
use url::Url;
use wipe_cert::SigningKey;
use wipe_license::VendorRoot;

pub const DEFAULT_ADDR: &str = "127.0.0.1:7900";
pub const DEFAULT_DATABASE_URL: &str = "postgres://wipehub:wipehub@127.0.0.1:55432/wipehub";
pub const DEFAULT_SMTP_HOST: &str = "127.0.0.1";
pub const DEFAULT_SMTP_PORT: u16 = 1025;
pub const DEFAULT_PUBLIC_BASE_URL: &str = "http://127.0.0.1:7900";

/// Where the portal is looked for when `--portal-dir` is absent, relative to
/// the working directory.
pub const DEFAULT_PORTAL_DIR: &str = "apps/portal/dist";

/// The command that builds the portal, named in warnings and errors.
pub const PORTAL_BUILD_HINT: &str = "cd apps/portal && pnpm build";

/// Env var that carries the Stripe webhook signing secret.
pub const STRIPE_WEBHOOK_SECRET_ENV: &str = "WIPE_HUB_STRIPE_WEBHOOK_SECRET";

/// Which `PaymentProvider` implementation the hub uses (ADR-0007 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PaymentProvider {
    /// Deterministic, in-process; used by every automated test.
    Fake,
    /// Stripe Checkout and webhooks; requires a webhook secret.
    Stripe,
}

/// Raw `wipe-hub serve` flags, before validation.
#[derive(Debug, Clone, Parser)]
pub struct Args {
    /// Address to listen on.
    #[arg(long, env = "WIPE_HUB_ADDR", default_value = DEFAULT_ADDR)]
    pub addr: SocketAddr,
    /// Postgres connection URL.
    #[arg(
        long,
        env = "WIPE_HUB_DATABASE_URL",
        default_value = DEFAULT_DATABASE_URL,
        hide_env_values = true
    )]
    pub database_url: String,
    /// SMTP host for outgoing mail (Mailpit locally).
    #[arg(long, env = "WIPE_HUB_SMTP_HOST", default_value = DEFAULT_SMTP_HOST)]
    pub smtp_host: String,
    /// SMTP port for outgoing mail.
    #[arg(long, env = "WIPE_HUB_SMTP_PORT", default_value_t = DEFAULT_SMTP_PORT)]
    pub smtp_port: u16,
    /// Absolute URL the hub is reached at, used in links it sends.
    #[arg(long, env = "WIPE_HUB_PUBLIC_BASE_URL", default_value = DEFAULT_PUBLIC_BASE_URL)]
    pub public_base_url: String,
    /// Payment provider.
    #[arg(long, env = "WIPE_HUB_PAYMENT_PROVIDER", value_enum, default_value_t = PaymentProvider::Fake)]
    pub payment_provider: PaymentProvider,
    /// Stripe webhook signing secret; required with `--payment-provider stripe`.
    #[arg(long, env = STRIPE_WEBHOOK_SECRET_ENV, hide_env_values = true)]
    pub stripe_webhook_secret: Option<String>,
    /// Path to a 32-byte vendor root seed. Absent: licence issuance is unavailable.
    #[arg(long, env = "WIPE_HUB_VENDOR_ROOT_SEED")]
    pub vendor_root_seed: Option<PathBuf>,
    /// Built portal directory (must hold `index.html`). Absent: `apps/portal/dist`
    /// under the working directory if built, else the portal is not served.
    #[arg(long, env = "WIPE_HUB_PORTAL_DIR")]
    pub portal_dir: Option<PathBuf>,
}

/// A loaded vendor root and where it came from.
pub struct FileVendorRoot {
    pub path: PathBuf,
    pub root: VendorRoot,
}

/// Validated hub configuration.
pub struct Config {
    pub addr: SocketAddr,
    pub database_url: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub public_base_url: Url,
    pub payment_provider: PaymentProvider,
    pub stripe_webhook_secret: Option<String>,
    /// `None` means licence issuance is unavailable.
    pub vendor_root: Option<FileVendorRoot>,
    /// Built portal to serve at `/`. `None` means the portal is not served.
    pub portal_dir: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(
        "--payment-provider stripe needs a webhook secret: set {STRIPE_WEBHOOK_SECRET_ENV} \
         or pass --stripe-webhook-secret"
    )]
    MissingStripeWebhookSecret,
    #[error("--public-base-url {value:?} is not an absolute http(s) URL: {reason}")]
    BadPublicBaseUrl { value: String, reason: String },
    #[error("cannot read vendor root seed {}: {error}", path.display())]
    SeedUnreadable {
        path: PathBuf,
        error: std::io::Error,
    },
    #[error("vendor root seed {} is not a 32-byte seed (got {len} bytes)", path.display())]
    SeedWrongLength { path: PathBuf, len: usize },
    #[error(
        "--portal-dir {} has no index.html: build the portal ({PORTAL_BUILD_HINT}) \
         or fix the path",
        path.display()
    )]
    PortalDirInvalid { path: PathBuf },
}

impl Config {
    pub fn from_args(args: Args) -> Result<Self, ConfigError> {
        let public_base_url = parse_base_url(&args.public_base_url)?;

        let stripe_webhook_secret = args
            .stripe_webhook_secret
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if args.payment_provider == PaymentProvider::Stripe && stripe_webhook_secret.is_none() {
            return Err(ConfigError::MissingStripeWebhookSecret);
        }

        // A path that is given but invalid is an error: a typo must never
        // silently disable signing.
        let vendor_root = match args.vendor_root_seed {
            Some(path) => {
                let key = load_seed(&path)?;
                Some(FileVendorRoot {
                    path,
                    root: VendorRoot::from_signing_key(key),
                })
            }
            None => None,
        };

        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let portal_dir = resolve_portal_dir(args.portal_dir, &cwd)?;

        Ok(Self {
            addr: args.addr,
            database_url: args.database_url,
            smtp_host: args.smtp_host,
            smtp_port: args.smtp_port,
            public_base_url,
            payment_provider: args.payment_provider,
            stripe_webhook_secret,
            vendor_root,
            portal_dir,
        })
    }
}

/// Resolve the portal directory. An explicit path without `index.html` is an
/// error, never a silent fallback; without one, `<cwd>/apps/portal/dist` is
/// used when it is built.
pub fn resolve_portal_dir(
    explicit: Option<PathBuf>,
    cwd: &Path,
) -> Result<Option<PathBuf>, ConfigError> {
    match explicit {
        Some(path) => {
            if path.join("index.html").is_file() {
                Ok(Some(path))
            } else {
                Err(ConfigError::PortalDirInvalid { path })
            }
        }
        None => {
            let dir = cwd.join(DEFAULT_PORTAL_DIR);
            Ok(dir.join("index.html").is_file().then_some(dir))
        }
    }
}

// Hand-written so secrets (webhook secret, DB password, root seed) never
// reach a log line through `{:?}`.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("addr", &self.addr)
            .field("database_url", &"<redacted>")
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("public_base_url", &self.public_base_url.as_str())
            .field("payment_provider", &self.payment_provider)
            .field(
                "stripe_webhook_secret",
                &self.stripe_webhook_secret.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "vendor_root",
                &self
                    .vendor_root
                    .as_ref()
                    .map(|r| (r.path.display().to_string(), r.root.public_key_id())),
            )
            .field(
                "portal_dir",
                &self.portal_dir.as_ref().map(|p| p.display().to_string()),
            )
            .finish()
    }
}

/// Warnings to log on every start.
pub fn startup_warnings(config: &Config) -> Vec<String> {
    let mut out = Vec::new();
    match &config.vendor_root {
        Some(r) => out.push(format!(
            "vendor root {} is file-held ({}): this is not production custody; \
             production must not launch on a file-held root (ADR-0007)",
            r.root.public_key_id(),
            r.path.display()
        )),
        None => out.push(
            "no vendor root seed configured (--vendor-root-seed / WIPE_HUB_VENDOR_ROOT_SEED): \
             licence issuance is unavailable"
                .to_string(),
        ),
    }
    if config.portal_dir.is_none() {
        let looked = std::env::current_dir()
            .map(|cwd| cwd.join(DEFAULT_PORTAL_DIR))
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_PORTAL_DIR));
        out.push(format!(
            "portal is not served: no built portal at {}; \
             run `{PORTAL_BUILD_HINT}` or pass --portal-dir / WIPE_HUB_PORTAL_DIR",
            looked.display()
        ));
    }
    out
}

fn parse_base_url(value: &str) -> Result<Url, ConfigError> {
    let bad = |reason: String| ConfigError::BadPublicBaseUrl {
        value: value.to_string(),
        reason,
    };
    let url = Url::parse(value).map_err(|e| bad(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(bad(format!(
            "scheme {:?} is not http or https",
            url.scheme()
        )));
    }
    if url.host_str().map_or(true, str::is_empty) {
        return Err(bad("no host".to_string()));
    }
    Ok(url)
}

fn load_seed(path: &Path) -> Result<SigningKey, ConfigError> {
    let bytes = fs::read(path).map_err(|error| ConfigError::SeedUnreadable {
        path: path.to_path_buf(),
        error,
    })?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|b: Vec<u8>| ConfigError::SeedWrongLength {
            path: path.to_path_buf(),
            len: b.len(),
        })?;
    Ok(SigningKey::from_seed(seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Defaults, built without reading the environment so a developer's
    /// `WIPE_HUB_*` vars cannot change the outcome.
    fn args() -> Args {
        Args {
            addr: DEFAULT_ADDR.parse().unwrap(),
            database_url: DEFAULT_DATABASE_URL.into(),
            smtp_host: DEFAULT_SMTP_HOST.into(),
            smtp_port: DEFAULT_SMTP_PORT,
            public_base_url: DEFAULT_PUBLIC_BASE_URL.into(),
            payment_provider: PaymentProvider::Fake,
            stripe_webhook_secret: None,
            vendor_root_seed: None,
            portal_dir: None,
        }
    }

    #[test]
    fn defaults_are_valid() {
        let c = Config::from_args(args()).unwrap();
        assert_eq!(c.payment_provider, PaymentProvider::Fake);
        assert_eq!(c.addr.to_string(), DEFAULT_ADDR);
    }

    #[test]
    fn stripe_without_secret_names_the_env_var() {
        for secret in [None, Some(String::new()), Some("  ".into())] {
            let mut a = args();
            a.payment_provider = PaymentProvider::Stripe;
            a.stripe_webhook_secret = secret;
            let err = Config::from_args(a).unwrap_err();
            assert!(matches!(err, ConfigError::MissingStripeWebhookSecret));
            assert!(err.to_string().contains("WIPE_HUB_STRIPE_WEBHOOK_SECRET"));
        }
    }

    #[test]
    fn stripe_with_secret_starts() {
        let mut a = args();
        a.payment_provider = PaymentProvider::Stripe;
        a.stripe_webhook_secret = Some(" whsec_test\n".into());
        let c = Config::from_args(a).unwrap();
        assert_eq!(c.payment_provider, PaymentProvider::Stripe);
        assert_eq!(c.stripe_webhook_secret.as_deref(), Some("whsec_test"));
        assert!(!format!("{c:?}").contains("whsec_test"));
    }

    #[test]
    fn unknown_payment_provider_is_rejected_by_clap() {
        let err = Args::try_parse_from(["serve", "--payment-provider", "paypal"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
    }

    #[test]
    fn valid_seed_file_starts_and_warns_file_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("root.seed");
        fs::write(&path, [7u8; 32]).unwrap();
        let mut a = args();
        a.vendor_root_seed = Some(path.clone());
        let c = Config::from_args(a).unwrap();
        let expected_id =
            VendorRoot::from_signing_key(SigningKey::from_seed([7u8; 32])).public_key_id();
        let r = c.vendor_root.as_ref().unwrap();
        assert_eq!(r.root.public_key_id(), expected_id);
        let warnings: Vec<String> = startup_warnings(&c)
            .into_iter()
            .filter(|w| w.contains("vendor root"))
            .collect();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("file-held"), "{warnings:?}");
        assert!(
            warnings[0].contains("not production custody"),
            "{warnings:?}"
        );
    }

    #[test]
    fn missing_seed_file_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.seed");
        let mut a = args();
        a.vendor_root_seed = Some(path.clone());
        let err = Config::from_args(a).unwrap_err();
        assert!(matches!(err, ConfigError::SeedUnreadable { .. }));
        assert!(err.to_string().contains(&path.display().to_string()));
    }

    #[test]
    fn wrong_length_seed_file_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        for len in [0usize, 31, 33, 64] {
            let path = dir.path().join(format!("seed-{len}"));
            fs::write(&path, vec![1u8; len]).unwrap();
            let mut a = args();
            a.vendor_root_seed = Some(path.clone());
            let err = Config::from_args(a).unwrap_err();
            assert!(
                matches!(err, ConfigError::SeedWrongLength { len: l, .. } if l == len),
                "{err}"
            );
            assert!(err.to_string().contains(&path.display().to_string()));
        }
    }

    #[test]
    fn no_seed_starts_and_says_issuance_unavailable() {
        let c = Config::from_args(args()).unwrap();
        assert!(c.vendor_root.is_none());
        let warnings = startup_warnings(&c);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("licence issuance is unavailable")),
            "{warnings:?}"
        );
        assert!(!warnings.iter().any(|w| w.contains("file-held")));
    }

    #[test]
    fn bad_public_base_url_is_rejected() {
        for bad in [
            "notaurl",
            "",
            "mailto:ops@example.com",
            "ftp://example.com",
            "http://",
        ] {
            let mut a = args();
            a.public_base_url = bad.into();
            let err = Config::from_args(a).unwrap_err();
            assert!(
                matches!(err, ConfigError::BadPublicBaseUrl { .. }),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn env_var_names_follow_the_prefix() {
        use clap::CommandFactory;
        let cmd = Args::command();
        for arg in cmd.get_arguments() {
            if matches!(arg.get_id().as_str(), "help" | "version") {
                continue;
            }
            let env = arg
                .get_env()
                .unwrap_or_else(|| panic!("--{} has no env var", arg.get_id()));
            assert!(env.to_string_lossy().starts_with("WIPE_HUB_"), "{env:?}");
        }
    }

    #[test]
    fn clap_defaults_match_the_constants() {
        use clap::CommandFactory;
        let smtp_port = DEFAULT_SMTP_PORT.to_string();
        let expected: [(&str, Option<&str>); 9] = [
            ("addr", Some(DEFAULT_ADDR)),
            ("database_url", Some(DEFAULT_DATABASE_URL)),
            ("smtp_host", Some(DEFAULT_SMTP_HOST)),
            ("smtp_port", Some(&smtp_port)),
            ("public_base_url", Some(DEFAULT_PUBLIC_BASE_URL)),
            ("payment_provider", Some("fake")),
            ("stripe_webhook_secret", None),
            ("vendor_root_seed", None),
            ("portal_dir", None),
        ];
        let cmd = Args::command();
        let mut seen = 0;
        for arg in cmd.get_arguments() {
            let id = arg.get_id().as_str();
            if matches!(id, "help" | "version") {
                continue;
            }
            let want = expected
                .iter()
                .find(|(name, _)| *name == id)
                .unwrap_or_else(|| panic!("--{id} has no expected default listed"))
                .1;
            let got: Vec<String> = arg
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            let want: Vec<String> = want.into_iter().map(str::to_string).collect();
            assert_eq!(got, want, "default of --{id}");
            seen += 1;
        }
        assert_eq!(seen, expected.len());
    }

    fn built_portal(root: &Path) -> PathBuf {
        let dir = root.join(DEFAULT_PORTAL_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
        dir
    }

    #[test]
    fn explicit_portal_dir_with_index_is_used() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = built_portal(tmp.path());
        let got = resolve_portal_dir(Some(dir.clone()), Path::new("/nonexistent")).unwrap();
        assert_eq!(got, Some(dir));
    }

    #[test]
    fn explicit_missing_portal_dir_is_an_error_naming_path_and_build() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist");
        // Even with a built default present, an explicit wrong path never falls back.
        built_portal(tmp.path());
        let err = resolve_portal_dir(Some(path.clone()), tmp.path()).unwrap_err();
        assert!(matches!(err, ConfigError::PortalDirInvalid { .. }), "{err}");
        let msg = err.to_string();
        assert!(msg.contains(&path.display().to_string()), "{msg}");
        assert!(msg.contains("pnpm build"), "{msg}");
    }

    #[test]
    fn explicit_portal_dir_without_index_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("assets")).unwrap();
        let err = resolve_portal_dir(Some(tmp.path().to_path_buf()), tmp.path()).unwrap_err();
        assert!(matches!(err, ConfigError::PortalDirInvalid { .. }), "{err}");
    }

    #[test]
    fn explicit_bad_portal_dir_fails_from_args() {
        let mut a = args();
        a.portal_dir = Some(PathBuf::from("/does/not/exist"));
        let err = Config::from_args(a).unwrap_err();
        assert!(matches!(err, ConfigError::PortalDirInvalid { .. }), "{err}");
        assert!(err.to_string().contains("/does/not/exist"));
    }

    #[test]
    fn absent_portal_dir_uses_built_cwd_default() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = built_portal(tmp.path());
        assert_eq!(resolve_portal_dir(None, tmp.path()).unwrap(), Some(dir));
    }

    #[test]
    fn absent_portal_dir_without_build_is_none_and_warns() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(resolve_portal_dir(None, tmp.path()).unwrap(), None);
        // An unbuilt default (directory without index.html) is also None.
        fs::create_dir_all(tmp.path().join(DEFAULT_PORTAL_DIR)).unwrap();
        assert_eq!(resolve_portal_dir(None, tmp.path()).unwrap(), None);

        let mut c = Config::from_args(args()).unwrap();
        c.portal_dir = None;
        let warnings = startup_warnings(&c);
        let w = warnings
            .iter()
            .find(|w| w.contains("portal is not served"))
            .unwrap_or_else(|| panic!("{warnings:?}"));
        assert!(w.contains(PORTAL_BUILD_HINT), "{w}");
        assert!(w.contains("--portal-dir"), "{w}");
    }

    #[test]
    fn portal_dir_is_in_debug() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = built_portal(tmp.path());
        let mut a = args();
        a.portal_dir = Some(dir.clone());
        let c = Config::from_args(a).unwrap();
        assert!(format!("{c:?}").contains(&dir.display().to_string()));
    }
}
