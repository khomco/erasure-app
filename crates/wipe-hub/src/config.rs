//! Hub configuration: flags and `WIPE_HUB_*` env vars, validated at start-up.
//!
//! Defaults match the root `compose.yaml`, so `docker compose up -d` followed
//! by `wipe-hub serve` with no flags is a working local setup.

use std::fmt;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use lettre::message::Mailbox;
use thiserror::Error;
use url::Url;
use wipe_cert::SigningKey;
use wipe_license::VendorRoot;

pub use crate::artifacts::ARTIFACT_MAX_BYTES;

pub const DEFAULT_ADDR: &str = "127.0.0.1:7900";
pub const DEFAULT_DATABASE_URL: &str = "postgres://wipehub:wipehub@127.0.0.1:55432/wipehub";
pub const DEFAULT_SMTP_HOST: &str = "127.0.0.1";
pub const DEFAULT_SMTP_PORT: u16 = 1025;
pub const DEFAULT_PUBLIC_BASE_URL: &str = "http://127.0.0.1:7900";
pub const DEFAULT_MAIL_FROM: &str = "Wipestation <no-reply@wipestation.localhost>";
/// The marketing site's origin in development (Astro's dev port).
pub const DEFAULT_SITE_ORIGIN: &str = "http://localhost:4321";

/// Where the portal is looked for when `--portal-dir` is absent, relative to
/// the working directory.
pub const DEFAULT_PORTAL_DIR: &str = "apps/portal/dist";

/// Where release artifacts are stored when `--artifact-dir` is absent,
/// relative to the working directory (git-ignored).
pub const DEFAULT_ARTIFACT_DIR: &str = ".wipe-hub/artifacts";

/// The command that builds the portal, named in warnings and errors.
pub const PORTAL_BUILD_HINT: &str = "cd apps/portal && pnpm build";

/// Env var that carries the Stripe webhook signing secret.
pub const STRIPE_WEBHOOK_SECRET_ENV: &str = "WIPE_HUB_STRIPE_WEBHOOK_SECRET";
/// Env var that carries the Stripe secret API key.
pub const STRIPE_SECRET_KEY_ENV: &str = "WIPE_HUB_STRIPE_SECRET_KEY";
/// Env var that overrides the Stripe API base URL (tests point it at a mock).
pub const STRIPE_API_BASE_ENV: &str = "WIPE_HUB_STRIPE_API_BASE";
/// The Stripe API.
pub const DEFAULT_STRIPE_API_BASE: &str = "https://api.stripe.com";

/// Which `PaymentProvider` implementation the hub uses (ADR-0007 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PaymentProvider {
    /// Deterministic, in-process; used by every automated test.
    Fake,
    /// Stripe Checkout and webhooks; requires a secret key and a webhook
    /// secret.
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
    /// Sender of every email the hub sends, as a mailbox
    /// (`Name <address>` or a bare address).
    #[arg(long, env = "WIPE_HUB_MAIL_FROM", default_value = DEFAULT_MAIL_FROM)]
    pub mail_from: String,
    /// Absolute URL the hub is reached at, used in links it sends.
    #[arg(long, env = "WIPE_HUB_PUBLIC_BASE_URL", default_value = DEFAULT_PUBLIC_BASE_URL)]
    pub public_base_url: String,
    /// Payment provider.
    #[arg(long, env = "WIPE_HUB_PAYMENT_PROVIDER", value_enum, default_value_t = PaymentProvider::Fake)]
    pub payment_provider: PaymentProvider,
    /// Stripe webhook signing secret; required with `--payment-provider stripe`.
    #[arg(long, env = STRIPE_WEBHOOK_SECRET_ENV, hide_env_values = true)]
    pub stripe_webhook_secret: Option<String>,
    /// Stripe secret API key (`sk_…` or a restricted `rk_…`); required with
    /// `--payment-provider stripe`.
    #[arg(long, env = STRIPE_SECRET_KEY_ENV, hide_env_values = true)]
    pub stripe_secret_key: Option<String>,
    /// Stripe API base URL: https, or http on a local host only.
    #[arg(long, env = STRIPE_API_BASE_ENV, default_value = DEFAULT_STRIPE_API_BASE)]
    pub stripe_api_base: String,
    /// Path to a 32-byte vendor root seed. Absent: licence issuance is unavailable.
    #[arg(long, env = "WIPE_HUB_VENDOR_ROOT_SEED")]
    pub vendor_root_seed: Option<PathBuf>,
    /// Built portal directory (must hold `index.html`). Absent: `apps/portal/dist`
    /// under the working directory if built, else the portal is not served.
    #[arg(long, env = "WIPE_HUB_PORTAL_DIR")]
    pub portal_dir: Option<PathBuf>,
    /// Origin of the marketing site (`http(s)://host[:port]`) whose forms post
    /// sales leads; the only origin sent CORS headers, on `GET /api/auth/csrf`
    /// and `POST /api/leads`.
    #[arg(long, env = "WIPE_HUB_SITE_ORIGIN", default_value = DEFAULT_SITE_ORIGIN)]
    pub site_origin: String,
    /// Directory release artifacts are stored in; created if missing.
    /// Relative paths are under the working directory.
    #[arg(long, env = "WIPE_HUB_ARTIFACT_DIR", default_value = DEFAULT_ARTIFACT_DIR)]
    pub artifact_dir: PathBuf,
}

/// Raw `wipe-hub admin create` flags (portal-accounts story 5). The
/// connection and mail flags are `serve`'s, with the same env twins and
/// defaults.
#[derive(Debug, Clone, Parser)]
pub struct AdminCreateArgs {
    /// Email address of the new vendor admin; the set-password link goes here.
    #[arg(long)]
    pub email: String,
    /// Display name. Defaults to the email's local part.
    #[arg(long)]
    pub name: Option<String>,
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
    /// Sender of every email the hub sends, as a mailbox
    /// (`Name <address>` or a bare address).
    #[arg(long, env = "WIPE_HUB_MAIL_FROM", default_value = DEFAULT_MAIL_FROM)]
    pub mail_from: String,
    /// Absolute URL the hub is reached at, used in the emailed link.
    #[arg(long, env = "WIPE_HUB_PUBLIC_BASE_URL", default_value = DEFAULT_PUBLIC_BASE_URL)]
    pub public_base_url: String,
}

/// Raw `wipe-hub release publish` flags (software-downloads story 1, CAP-5).
/// `--database-url` and `--artifact-dir` are `serve`'s, with the same env
/// twins and defaults.
#[derive(Debug, Clone, Parser)]
pub struct ReleasePublishArgs {
    /// SemVer 2.0.0 version, unique across every release (e.g. `0.2.0`).
    #[arg(long)]
    pub version: String,
    /// Release notes shown with the release.
    #[arg(long)]
    pub notes: String,
    /// Release channel: `stable` or `preview`.
    #[arg(long, default_value = "stable")]
    pub channel: String,
    /// One artifact as `<platform>=<path>` (e.g. `linux-x86_64=target/release/wipestation`);
    /// repeat for each platform.
    #[arg(long = "file", value_name = "PLATFORM=PATH", required = true)]
    pub files: Vec<String>,
    /// Postgres connection URL.
    #[arg(
        long,
        env = "WIPE_HUB_DATABASE_URL",
        default_value = DEFAULT_DATABASE_URL,
        hide_env_values = true
    )]
    pub database_url: String,
    /// Directory release artifacts are stored in; created if missing.
    /// Relative paths are under the working directory.
    #[arg(long, env = "WIPE_HUB_ARTIFACT_DIR", default_value = DEFAULT_ARTIFACT_DIR)]
    pub artifact_dir: PathBuf,
}

/// `--mail-from` as a mailbox, or [`ConfigError::BadMailFrom`].
pub fn parse_mail_from(value: &str) -> Result<Mailbox, ConfigError> {
    value.trim().parse().map_err(
        |e: lettre::address::AddressError| ConfigError::BadMailFrom {
            value: value.to_string(),
            reason: e.to_string(),
        },
    )
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
    pub mail_from: Mailbox,
    pub public_base_url: Url,
    pub payment_provider: PaymentProvider,
    pub stripe_webhook_secret: Option<String>,
    pub stripe_secret_key: Option<String>,
    /// Where the Stripe provider sends its API calls.
    pub stripe_api_base: Url,
    /// `None` means licence issuance is unavailable.
    pub vendor_root: Option<FileVendorRoot>,
    /// Built portal to serve at `/`. `None` means the portal is not served.
    pub portal_dir: Option<PathBuf>,
    /// The marketing site's origin, serialized as a browser sends it in
    /// `Origin` (`scheme://host[:port]`, no default port, no trailing slash).
    pub site_origin: String,
    /// The existing directory release artifacts are stored in (absolute).
    pub artifact_dir: PathBuf,
    /// The largest artifact accepted: [`ARTIFACT_MAX_BYTES`]. Not a flag;
    /// tests lower it.
    pub artifact_max_bytes: u64,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(
        "--payment-provider stripe needs a webhook secret: set {STRIPE_WEBHOOK_SECRET_ENV} \
         or pass --stripe-webhook-secret"
    )]
    MissingStripeWebhookSecret,
    #[error(
        "--payment-provider stripe needs a secret API key: set {STRIPE_SECRET_KEY_ENV} \
         or pass --stripe-secret-key"
    )]
    MissingStripeSecretKey,
    #[error(
        "--stripe-api-base {value:?} must be an https URL (or http on a local host): {reason}"
    )]
    BadStripeApiBase { value: String, reason: String },
    #[error("--public-base-url {value:?} is not an absolute http(s) URL: {reason}")]
    BadPublicBaseUrl { value: String, reason: String },
    #[error("--mail-from {value:?} is not a mailbox (`Name <address>` or `address`): {reason}")]
    BadMailFrom { value: String, reason: String },
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
    #[error("--site-origin {value:?} is not a site origin (`http(s)://host[:port]`): {reason}")]
    BadSiteOrigin { value: String, reason: String },
    #[error("--artifact-dir {}: {reason}", path.display())]
    ArtifactDirInvalid { path: PathBuf, reason: String },
}

impl Config {
    pub fn from_args(args: Args) -> Result<Self, ConfigError> {
        let public_base_url = parse_base_url(&args.public_base_url)?;
        let mail_from = parse_mail_from(&args.mail_from)?;
        let site_origin = parse_site_origin(&args.site_origin)?;

        let secret = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let stripe_webhook_secret = secret(args.stripe_webhook_secret);
        let stripe_secret_key = secret(args.stripe_secret_key);
        if args.payment_provider == PaymentProvider::Stripe {
            if stripe_webhook_secret.is_none() {
                return Err(ConfigError::MissingStripeWebhookSecret);
            }
            if stripe_secret_key.is_none() {
                return Err(ConfigError::MissingStripeSecretKey);
            }
        }
        let stripe_api_base = parse_stripe_api_base(&args.stripe_api_base)?;

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
        let artifact_dir = resolve_artifact_dir(&args.artifact_dir, &cwd)?;

        Ok(Self {
            addr: args.addr,
            database_url: args.database_url,
            smtp_host: args.smtp_host,
            smtp_port: args.smtp_port,
            mail_from,
            public_base_url,
            payment_provider: args.payment_provider,
            stripe_webhook_secret,
            stripe_secret_key,
            stripe_api_base,
            vendor_root,
            portal_dir,
            site_origin,
            artifact_dir,
            artifact_max_bytes: ARTIFACT_MAX_BYTES,
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

/// The artifact directory as an absolute path (relative ones are under
/// `cwd`), created if missing. A path that cannot be created, or that is not
/// a directory, is an error: the hub must not start without somewhere to
/// keep release files.
pub fn resolve_artifact_dir(path: &Path, cwd: &Path) -> Result<PathBuf, ConfigError> {
    let dir = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let bad = |reason: String| ConfigError::ArtifactDirInvalid {
        path: dir.clone(),
        reason,
    };
    if path.as_os_str().is_empty() {
        return Err(bad("the path is empty".into()));
    }
    if dir.exists() && !dir.is_dir() {
        return Err(bad("it exists and is not a directory".into()));
    }
    fs::create_dir_all(&dir).map_err(|e| bad(format!("it cannot be created: {e}")))?;
    if !dir.is_dir() {
        return Err(bad("it is not a directory".into()));
    }
    Ok(dir)
}

// Hand-written so secrets (Stripe key, webhook secret, DB password, root seed) never
// reach a log line through `{:?}`.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("addr", &self.addr)
            .field("database_url", &"<redacted>")
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("mail_from", &self.mail_from.to_string())
            .field("public_base_url", &self.public_base_url.as_str())
            .field("payment_provider", &self.payment_provider)
            .field(
                "stripe_webhook_secret",
                &self.stripe_webhook_secret.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "stripe_secret_key",
                &self.stripe_secret_key.as_ref().map(|_| "<redacted>"),
            )
            .field("stripe_api_base", &self.stripe_api_base.as_str())
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
            .field("site_origin", &self.site_origin)
            .field("artifact_dir", &self.artifact_dir.display().to_string())
            .field("artifact_max_bytes", &self.artifact_max_bytes)
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
    let base = &config.public_base_url;
    if base.scheme() == "http" && crate::auth::secure_cookies(base) {
        out.push(format!(
            "--public-base-url {base} is http on a non-local host: session and CSRF cookies \
             are marked Secure, which browsers will not keep over http, so nobody can sign in; \
             serve the hub over https or use a localhost URL for development"
        ));
    }
    if config.payment_provider == PaymentProvider::Fake {
        out.push(if crate::auth::is_local_host(base) {
            "payment provider is fake: no money moves, and the portal's test checkout page \
             marks orders paid; never sell with it"
                .to_string()
        } else {
            format!(
                "payment provider is fake on non-local --public-base-url {base}: checkout, \
                 billing and test payments are refused; use --payment-provider stripe"
            )
        });
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

pub fn parse_base_url(value: &str) -> Result<Url, ConfigError> {
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

/// `--stripe-api-base`: https, or http only on a local host (so a typo can
/// never send the secret key over plain http), with a host and no user
/// name, password, query or fragment.
pub fn parse_stripe_api_base(value: &str) -> Result<Url, ConfigError> {
    let bad = |reason: &str| ConfigError::BadStripeApiBase {
        value: value.to_string(),
        reason: reason.to_string(),
    };
    let url = Url::parse(value.trim()).map_err(|e| bad(&e.to_string()))?;
    if url.host_str().map_or(true, str::is_empty) {
        return Err(bad("there is no host"));
    }
    match url.scheme() {
        "https" => {}
        "http" if crate::auth::is_local_host(&url) => {}
        "http" => return Err(bad("plain http is allowed only on a local host")),
        _ => return Err(bad("the scheme must be https")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("it must not carry a user name or password"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(bad("it must not have a query or fragment"));
    }
    Ok(url)
}

/// `--site-origin` as the string a browser sends in `Origin`: an http(s) URL
/// with a host and nothing after it but an optional `/` (no other path, no
/// query, fragment or userinfo). The default port is dropped and the host
/// lowercased, as browsers do, so the CORS check can compare strings.
pub fn parse_site_origin(value: &str) -> Result<String, ConfigError> {
    let bad = |reason: &str| ConfigError::BadSiteOrigin {
        value: value.to_string(),
        reason: reason.to_string(),
    };
    let url = Url::parse(value.trim()).map_err(|e| bad(&e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(bad("the scheme must be http or https"));
    }
    if url.host_str().map_or(true, str::is_empty) {
        return Err(bad("there is no host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("an origin has no user name or password"));
    }
    if url.path() != "/" {
        return Err(bad("an origin has no path"));
    }
    if url.query().is_some() {
        return Err(bad("an origin has no query"));
    }
    if url.fragment().is_some() {
        return Err(bad("an origin has no fragment"));
    }
    Ok(url.origin().ascii_serialization())
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
            mail_from: DEFAULT_MAIL_FROM.into(),
            public_base_url: DEFAULT_PUBLIC_BASE_URL.into(),
            payment_provider: PaymentProvider::Fake,
            stripe_webhook_secret: None,
            stripe_secret_key: None,
            stripe_api_base: DEFAULT_STRIPE_API_BASE.into(),
            vendor_root_seed: None,
            portal_dir: None,
            site_origin: DEFAULT_SITE_ORIGIN.into(),
            artifact_dir: ARTIFACTS.path().to_path_buf(),
        }
    }

    /// One artifact directory for every unit test here, so none creates
    /// `.wipe-hub/artifacts` under the crate.
    static ARTIFACTS: std::sync::LazyLock<tempfile::TempDir> =
        std::sync::LazyLock::new(|| tempfile::tempdir().unwrap());

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
    fn stripe_without_secret_key_names_the_env_var() {
        for key in [None, Some(String::new()), Some(" \n".into())] {
            let mut a = args();
            a.payment_provider = PaymentProvider::Stripe;
            a.stripe_webhook_secret = Some("whsec_test".into());
            a.stripe_secret_key = key;
            let err = Config::from_args(a).unwrap_err();
            assert!(matches!(err, ConfigError::MissingStripeSecretKey), "{err}");
            assert!(err.to_string().contains(STRIPE_SECRET_KEY_ENV), "{err}");
        }
    }

    #[test]
    fn stripe_with_secrets_starts_and_redacts_them() {
        let mut a = args();
        a.payment_provider = PaymentProvider::Stripe;
        a.stripe_webhook_secret = Some(" whsec_test\n".into());
        a.stripe_secret_key = Some(" sk_test_secret\n".into());
        let c = Config::from_args(a).unwrap();
        assert_eq!(c.payment_provider, PaymentProvider::Stripe);
        assert_eq!(c.stripe_webhook_secret.as_deref(), Some("whsec_test"));
        assert_eq!(c.stripe_secret_key.as_deref(), Some("sk_test_secret"));
        assert_eq!(c.stripe_api_base.as_str(), "https://api.stripe.com/");
        let debug = format!("{c:?}");
        assert!(!debug.contains("whsec_test"), "{debug}");
        assert!(!debug.contains("sk_test_secret"), "{debug}");
        assert!(debug.contains("https://api.stripe.com"), "{debug}");
    }

    #[test]
    fn stripe_secrets_hide_their_env_values() {
        use clap::CommandFactory;
        let cmd = Args::command();
        for id in ["stripe_secret_key", "stripe_webhook_secret", "database_url"] {
            let arg = cmd
                .get_arguments()
                .find(|a| a.get_id().as_str() == id)
                .unwrap();
            assert!(arg.is_hide_env_values_set(), "--{id}");
        }
    }

    #[test]
    fn stripe_api_base_must_be_https_or_local_http() {
        for good in [
            "https://api.stripe.com",
            "https://stripe-proxy.example.com/base/",
            "http://127.0.0.1:12111",
            "http://localhost:12111",
            "http://[::1]:12111",
        ] {
            let mut a = args();
            a.stripe_api_base = good.into();
            let c = Config::from_args(a).unwrap_or_else(|e| panic!("{good}: {e}"));
            assert!(c
                .stripe_api_base
                .as_str()
                .starts_with(good.trim_end_matches('/')));
        }
        for bad in [
            "http://api.example.com",
            "http://api.stripe.com",
            "ftp://api.stripe.com",
            "file:///tmp/stripe",
            "api.stripe.com",
            "",
            "https://",
            "https://user:pw@api.stripe.com",
            "https://api.stripe.com/?x=1",
        ] {
            let mut a = args();
            a.stripe_api_base = bad.into();
            let err = Config::from_args(a).unwrap_err();
            assert!(
                matches!(err, ConfigError::BadStripeApiBase { .. }),
                "{bad}: {err}"
            );
            assert!(err.to_string().contains("--stripe-api-base"), "{err}");
        }
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
    fn site_origin_is_serialized_as_browsers_send_it() {
        let c = Config::from_args(args()).unwrap();
        assert_eq!(c.site_origin, DEFAULT_SITE_ORIGIN);
        for (value, origin) in [
            ("http://localhost:4321", "http://localhost:4321"),
            ("http://localhost:4321/", "http://localhost:4321"),
            ("https://WWW.Example.com", "https://www.example.com"),
            ("https://www.example.com:443/", "https://www.example.com"),
            ("http://127.0.0.1:8080", "http://127.0.0.1:8080"),
            ("http://[::1]:4321", "http://[::1]:4321"),
        ] {
            let mut a = args();
            a.site_origin = value.into();
            let c = Config::from_args(a).unwrap();
            assert_eq!(c.site_origin, origin, "{value}");
            assert!(format!("{c:?}").contains(origin));
        }
    }

    #[test]
    fn bad_site_origin_is_rejected_naming_the_value() {
        for bad in [
            "https://x.example/path",
            "https://x.example/path/",
            "https://x.example/?q=1",
            "https://x.example/?",
            "https://x.example/#top",
            "https://user@x.example",
            "https://user:pw@x.example",
            "ftp://x.example",
            "file:///tmp",
            "x.example",
            "",
            "http://",
        ] {
            let mut a = args();
            a.site_origin = bad.into();
            let err = Config::from_args(a).unwrap_err();
            assert!(
                matches!(err, ConfigError::BadSiteOrigin { .. }),
                "{bad}: {err}"
            );
            let msg = err.to_string();
            assert!(msg.contains("--site-origin"), "{msg}");
            assert!(msg.contains(&format!("{bad:?}")), "{msg}");
        }
    }

    #[test]
    fn mail_from_accepts_mailboxes() {
        for (value, address) in [
            (DEFAULT_MAIL_FROM, "no-reply@wipestation.localhost"),
            ("ops@example.com", "ops@example.com"),
            (" Sales Team <sales@example.com> ", "sales@example.com"),
        ] {
            let mut a = args();
            a.mail_from = value.into();
            let c = Config::from_args(a).unwrap();
            assert_eq!(c.mail_from.email.to_string(), address, "{value}");
        }
    }

    #[test]
    fn bad_mail_from_is_rejected_naming_the_value() {
        for bad in ["", "not a mailbox", "Name <no-at-sign>", "a@b@c"] {
            let mut a = args();
            a.mail_from = bad.into();
            let err = Config::from_args(a).unwrap_err();
            assert!(
                matches!(err, ConfigError::BadMailFrom { .. }),
                "{bad}: {err}"
            );
            assert!(err.to_string().contains("--mail-from"), "{err}");
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
        let expected: [(&str, Option<&str>); 14] = [
            ("addr", Some(DEFAULT_ADDR)),
            ("database_url", Some(DEFAULT_DATABASE_URL)),
            ("smtp_host", Some(DEFAULT_SMTP_HOST)),
            ("smtp_port", Some(&smtp_port)),
            ("mail_from", Some(DEFAULT_MAIL_FROM)),
            ("public_base_url", Some(DEFAULT_PUBLIC_BASE_URL)),
            ("payment_provider", Some("fake")),
            ("stripe_webhook_secret", None),
            ("stripe_secret_key", None),
            ("stripe_api_base", Some(DEFAULT_STRIPE_API_BASE)),
            ("vendor_root_seed", None),
            ("portal_dir", None),
            ("site_origin", Some(DEFAULT_SITE_ORIGIN)),
            ("artifact_dir", Some(DEFAULT_ARTIFACT_DIR)),
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

    #[test]
    fn admin_create_shares_serves_connection_flags() {
        use clap::CommandFactory;
        let serve = Args::command();
        let admin = AdminCreateArgs::command();
        let shared = [
            "database_url",
            "smtp_host",
            "smtp_port",
            "mail_from",
            "public_base_url",
        ];
        for id in shared {
            let find = |cmd: &clap::Command| {
                let arg = cmd
                    .get_arguments()
                    .find(|a| a.get_id().as_str() == id)
                    .unwrap_or_else(|| panic!("--{id} missing"))
                    .clone();
                (
                    arg.get_env().map(|e| e.to_os_string()),
                    arg.get_default_values().to_vec(),
                )
            };
            assert_eq!(find(&admin), find(&serve), "--{id}");
        }
        for arg in admin.get_arguments() {
            let id = arg.get_id().as_str();
            assert!(
                shared.contains(&id) || matches!(id, "email" | "name" | "help"),
                "unexpected --{id}"
            );
        }
        assert!(AdminCreateArgs::try_parse_from(["create"]).is_err());
        let a = AdminCreateArgs::try_parse_from(["create", "--email", "a@b.com"]).unwrap();
        assert_eq!(a.name, None);
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
    fn the_fake_payment_provider_always_warns() {
        let fake_warnings = |base: &str, provider: PaymentProvider| {
            let mut a = args();
            a.public_base_url = base.into();
            a.payment_provider = provider;
            a.stripe_webhook_secret = Some("whsec_test".into());
            a.stripe_secret_key = Some("sk_test_x".into());
            let c = Config::from_args(a).unwrap();
            startup_warnings(&c)
                .into_iter()
                .filter(|w| w.contains("payment provider is fake"))
                .collect::<Vec<_>>()
        };
        let local = fake_warnings("http://127.0.0.1:7900", PaymentProvider::Fake);
        assert_eq!(local.len(), 1, "{local:?}");
        assert!(local[0].contains("no money moves"), "{local:?}");
        let remote = fake_warnings("https://hub.example.com", PaymentProvider::Fake);
        assert_eq!(remote.len(), 1, "{remote:?}");
        assert!(remote[0].contains("https://hub.example.com"), "{remote:?}");
        assert!(remote[0].contains("refused"), "{remote:?}");
        assert!(fake_warnings("http://127.0.0.1:7900", PaymentProvider::Stripe).is_empty());
        assert_eq!(PaymentProvider::Fake.as_str(), "fake");
        assert_eq!(PaymentProvider::Stripe.as_str(), "stripe");
    }

    #[test]
    fn http_on_a_non_local_host_warns_about_secure_cookies() {
        let warned = |base: &str| {
            let mut a = args();
            a.public_base_url = base.into();
            let c = Config::from_args(a).unwrap();
            startup_warnings(&c)
                .into_iter()
                .filter(|w| w.contains("Secure"))
                .collect::<Vec<_>>()
        };
        let w = warned("http://hub.example.com");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("http://hub.example.com"), "{w:?}");
        assert!(w[0].contains("browsers will not keep"), "{w:?}");
        for quiet in [
            "http://127.0.0.1:7900",
            "http://localhost:5174",
            "http://portal.localhost",
            "https://hub.example.com",
            "https://localhost",
        ] {
            assert!(warned(quiet).is_empty(), "{quiet}");
        }
    }

    #[test]
    fn artifact_dir_is_created_under_the_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = resolve_artifact_dir(Path::new(DEFAULT_ARTIFACT_DIR), tmp.path()).unwrap();
        assert_eq!(dir, tmp.path().join(".wipe-hub/artifacts"));
        assert!(dir.is_dir());
        // Existing is fine, and an absolute path ignores the cwd.
        let again = resolve_artifact_dir(&dir, Path::new("/nonexistent")).unwrap();
        assert_eq!(again, dir);
    }

    #[test]
    fn an_artifact_dir_that_is_a_file_or_cannot_be_made_refuses_to_start() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a-file");
        fs::write(&file, "x").unwrap();
        for path in [file.clone(), file.join("below")] {
            let mut a = args();
            a.artifact_dir = path.clone();
            let err = Config::from_args(a).unwrap_err();
            assert!(
                matches!(err, ConfigError::ArtifactDirInvalid { .. }),
                "{err}"
            );
            let msg = err.to_string();
            assert!(msg.contains("--artifact-dir"), "{msg}");
            assert!(msg.contains(&path.display().to_string()), "{msg}");
        }
        let mut a = args();
        a.artifact_dir = PathBuf::new();
        assert!(matches!(
            Config::from_args(a).unwrap_err(),
            ConfigError::ArtifactDirInvalid { .. }
        ));
    }

    #[test]
    fn the_artifact_limit_is_a_gibibyte_and_in_debug() {
        let c = Config::from_args(args()).unwrap();
        assert_eq!(c.artifact_max_bytes, 1 << 30);
        let debug = format!("{c:?}");
        assert!(
            debug.contains(&c.artifact_dir.display().to_string()),
            "{debug}"
        );
    }

    #[test]
    fn release_publish_shares_serves_flags() {
        use clap::CommandFactory;
        let serve = Args::command();
        let publish = ReleasePublishArgs::command();
        let shared = ["database_url", "artifact_dir"];
        for id in shared {
            let find = |cmd: &clap::Command| {
                let arg = cmd
                    .get_arguments()
                    .find(|a| a.get_id().as_str() == id)
                    .unwrap_or_else(|| panic!("--{id} missing"))
                    .clone();
                (
                    arg.get_env().map(|e| e.to_os_string()),
                    arg.get_default_values().to_vec(),
                    arg.is_hide_env_values_set(),
                )
            };
            assert_eq!(find(&publish), find(&serve), "--{id}");
        }
        publish.clone().debug_assert();
        let base = ["publish", "--version", "0.2.0", "--notes", "n"];
        assert!(
            ReleasePublishArgs::try_parse_from(base).is_err(),
            "--file is required"
        );
        let a = ReleasePublishArgs::try_parse_from(
            base.iter()
                .copied()
                .chain(["--file", "a=x", "--file", "b=y"]),
        )
        .unwrap();
        assert_eq!(a.channel, "stable");
        assert_eq!(a.files, ["a=x", "b=y"]);
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
