//! The station's client for the hub (ADR-0008, station-enrollment story 3).
//!
//! One implementation for every station-side caller: `wipestation enroll`,
//! the bench app's station routes ([`crate::enrollment`], story 4), and the
//! periodic licence fetch ([`crate::licence_fetch`], story 5).
//!
//! - [`HubOrigin`]: the only hub address a station accepts. `https`, or
//!   `http` only on `localhost`, `*.localhost`, `127.0.0.0/8` or `::1`
//!   (the hub's own local-host rule, reimplemented here because no station
//!   crate depends on `wipe-hub`); no user name, query or fragment; path empty
//!   or `/`. Kept as `scheme://host[:port]`, no trailing slash.
//! - [`ProxySettings`]: `HTTPS_PROXY` and `NO_PROXY` (ADR-0008 §5), read once
//!   and given to the client explicitly, so no other proxy source applies:
//!   an `https` hub is reached through the proxy unless `NO_PROXY` matches.
//! - [`HubClient`]: connect timeout 10 s, request timeout 30 s, no redirects
//!   (a station talks to its hub's origin and nothing else). [`HubClient::start`]
//!   and [`HubClient::poll`] send the signed bodies of
//!   [`wipe_license::enrollment`].
//! - [`enroll`]: start, then poll at the hub's `interval` (and `slow_down`'s
//!   new one), retrying a network error or 5xx at the current interval, until
//!   approval, denial or `expires_in` ([`poll_until_done`] is the polling
//!   half, for a caller that sends the start itself). An approved licence is
//!   checked against **only the roots the caller gives**: the hub delivers
//!   licences and can never introduce a root (ADR-0008 §4).
//! - [`install_on_station`]: hand a verified licence to a running `serve`
//!   through `POST /api/license` (the runtime install, memory-only).
//! - [`HubClient::challenge`] and [`HubClient::fetch_licence`] (story 5): a
//!   single-use challenge, then `POST /api/station/licence {instance_public_key,
//!   licence_id}` signed with [`wipe_license::station_request`] over it, the
//!   hub origin, `POST`, the path and the body. The fetched licence is not
//!   checked here: the caller checks it with [`check_licence`].
//!
//! Nothing here gates, delays or interrupts an erasure.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{NoProxy, Proxy, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use url::{Host, Url};
use wipe_cert::{SigningKey, VerifyingKey};
use wipe_license::{InstallProblem, LicenseCertificate};

/// How long a connection to the hub may take to open.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one request to the hub may take, end to end.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Where a station starts enrolling (`wipe-hub`'s route).
pub const ENROLLMENT_START_PATH: &str = "/api/enrollments";
/// Where a station polls its enrollment (`wipe-hub`'s route).
pub const ENROLLMENT_POLL_PATH: &str = "/api/enrollments/poll";
/// Where a station asks for a single-use challenge (`wipe-hub`'s route).
pub const STATION_CHALLENGE_PATH: &str = "/api/station/challenge";
/// Where a station fetches its licence (`wipe-hub`'s route).
pub const STATION_LICENCE_PATH: &str = "/api/station/licence";

/// The rule a hub address must meet, stated in every refusal.
pub const HUB_ORIGIN_RULE: &str = "A hub address is an https origin such as \
    https://hub.example.com (http is allowed only on localhost, *.localhost, 127.0.0.0/8 or \
    ::1, for development), with no user name, password, path, query or fragment.";

// ---------------------------------------------------------------------------
// Hub origin

/// A hub address this station accepts, as `scheme://host[:port]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubOrigin(String);

/// Why a hub address was refused. The text states the rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{raw}` is not a hub address this station accepts: {why}. {HUB_ORIGIN_RULE}")]
pub struct OriginError {
    raw: String,
    why: String,
}

impl HubOrigin {
    /// Check `raw` against the rule ([`HUB_ORIGIN_RULE`]).
    pub fn parse(raw: &str) -> Result<Self, OriginError> {
        let refuse = |why: &str| OriginError {
            raw: raw.to_string(),
            why: why.to_string(),
        };
        let url = Url::parse(raw.trim()).map_err(|e| refuse(&format!("not a URL ({e})")))?;
        match url.scheme() {
            "https" => {}
            "http" if is_local_host(&url) => {}
            "http" => return Err(refuse("http is allowed only on a loopback host")),
            other => return Err(refuse(&format!("the scheme is {other}, not https"))),
        }
        if url.host().is_none() {
            return Err(refuse("it names no host"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(refuse("it carries a user name or password"));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(refuse("it carries a query or fragment"));
        }
        if url.path() != "/" && !url.path().is_empty() {
            return Err(refuse("it carries a path"));
        }
        Ok(Self(url.origin().ascii_serialization()))
    }

    /// `scheme://host[:port]`, no trailing slash.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.0)
    }
}

impl std::fmt::Display for HubOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `localhost`, `*.localhost`, `127.0.0.0/8` or `::1` (including the
/// IPv4-mapped form). The same rule as the hub's `auth::is_local_host`.
fn is_local_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => std::net::IpAddr::V6(ip).to_canonical().is_loopback(),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Proxy

/// The proxy a station uses to reach an `https` hub (ADR-0008 §5). Read once
/// with [`from_env`](Self::from_env) and given to the client explicitly: no
/// other proxy source (system settings, `HTTP_PROXY`, `ALL_PROXY`) applies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxySettings {
    /// `HTTPS_PROXY`, else `https_proxy`.
    pub https_proxy: Option<String>,
    /// `NO_PROXY`, else `no_proxy`: hosts reached directly.
    pub no_proxy: Option<String>,
}

impl ProxySettings {
    /// From the process environment. Empty values count as unset.
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// From `lookup`, which answers an environment variable's value.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let first = |names: [&str; 2]| {
            names
                .into_iter()
                .filter_map(&lookup)
                .map(|v| v.trim().to_string())
                .find(|v| !v.is_empty())
        };
        Self {
            https_proxy: first(["HTTPS_PROXY", "https_proxy"]),
            no_proxy: first(["NO_PROXY", "no_proxy"]),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors

/// Why enrollment ended without a licence. Every message is written for the
/// operator at the bench and says whether anything was saved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnrollError {
    /// The HTTP client could not be built (a malformed `HTTPS_PROXY`).
    #[error("cannot set up the connection to the hub: {0}")]
    Client(String),
    /// The hub could not be reached to start.
    #[error(
        "could not reach the hub at {origin}: {reason}. Check the address, and that this station \
         can reach it (directly, or through HTTPS_PROXY and NO_PROXY)."
    )]
    HubUnreachable { origin: String, reason: String },
    /// The hub answered the start with an error status.
    #[error(
        "the hub at {origin} refused to start an enrollment (HTTP {status}): {message}{}",
        retry_hint(*retry_after_secs)
    )]
    StartRefused {
        origin: String,
        status: u16,
        message: String,
        retry_after_secs: Option<u64>,
    },
    /// The hub's answer could not be read.
    #[error("the hub at {origin} sent an answer this station does not understand: {reason}")]
    BadResponse { origin: String, reason: String },
    /// An owner denied the enrollment.
    #[error(
        "an owner denied this enrollment at {origin}; nothing was saved or installed. Run \
         wipestation enroll again to start a new one."
    )]
    Denied { origin: String },
    /// The code expired before approval, or the hub says it has.
    #[error(
        "the enrollment code expired before an owner approved it; nothing was saved or \
         installed. Run wipestation enroll again to get a new code."
    )]
    Expired,
    /// The hub refused a poll for a reason other than the RFC 8628 outcomes.
    #[error(
        "the hub at {origin} refused this enrollment ({reason}); nothing was saved or installed. \
         Run wipestation enroll again to start a new one."
    )]
    PollRefused { origin: String, reason: String },
    /// The licence does not verify against the station's own roots.
    #[error(
        "the hub at {origin} sent a licence this station does not trust ({reason}). This station \
         trusts only the vendor roots it was given with --vendor-root-b64, so nothing was saved \
         or installed."
    )]
    UntrustedLicence { origin: String, reason: String },
    /// The licence verifies but names another instance key.
    #[error(
        "the hub at {origin} sent a licence for another key: it names instance key {found}, but \
         this station's key is {expected}. Nothing was saved or installed."
    )]
    WrongKey {
        origin: String,
        expected: String,
        found: String,
    },
}

fn retry_hint(secs: Option<u64>) -> String {
    match secs {
        Some(s) => format!(" Try again in {s} seconds."),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Client

/// `POST /api/enrollments`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StartResponse {
    /// Secret: only ever sent back to the hub, signed, in polls.
    pub device_code: String,
    /// `XXXX-XXXX`, for the owner to enter.
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    /// Seconds until the codes expire.
    pub expires_in: u64,
    /// Seconds to wait between polls.
    pub interval: u64,
}

/// One poll's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum PollOutcome {
    /// `authorization_pending`.
    Pending,
    /// `slow_down`, with the hub's new interval when it sent one.
    SlowDown { interval: Option<u64> },
    /// `access_denied`.
    Denied,
    /// `expired_token`.
    Expired,
    /// Approved: the registered station and its signed licence, unverified.
    Collected { station: Value, licence: Value },
    /// A network error or a 5xx: worth asking again.
    Transient(String),
    /// Any other refusal (`invalid_grant`, an unexpected status or body).
    Refused(String),
}

/// One licence fetch's answer ([`HubClient::fetch_licence`]).
#[derive(Debug, Clone, PartialEq)]
pub enum FetchOutcome {
    /// 204: nothing newer than the licence the station holds.
    Current,
    /// 200: the station's latest licence, unverified.
    Licence(Value),
    /// 404: no organization has a station with this key.
    NotRegistered,
    /// The hub could not be reached, or did not answer in time.
    Unreachable(String),
    /// Any other answer: the hub's `error` text, or what was wrong with it.
    Refused(String),
}

/// A client for one hub.
#[derive(Debug, Clone)]
pub struct HubClient {
    http: reqwest::Client,
    origin: HubOrigin,
}

impl HubClient {
    /// A client for `origin`, through `proxy` when the hub is `https`.
    pub fn new(origin: HubOrigin, proxy: &ProxySettings) -> Result<Self, EnrollError> {
        Self::with_timeout(origin, proxy, REQUEST_TIMEOUT)
    }

    /// As [`new`](Self::new), with `request_timeout` instead of
    /// [`REQUEST_TIMEOUT`] (tests use a short one against a hub that never
    /// answers).
    pub fn with_timeout(
        origin: HubOrigin,
        proxy: &ProxySettings,
        request_timeout: Duration,
    ) -> Result<Self, EnrollError> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT.min(request_timeout))
            .timeout(request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy();
        if let Some(url) = &proxy.https_proxy {
            let p = Proxy::https(url.as_str())
                .map_err(|e| EnrollError::Client(format!("HTTPS_PROXY `{url}`: {e}")))?
                .no_proxy(proxy.no_proxy.as_deref().and_then(NoProxy::from_string));
            builder = builder.proxy(p);
        }
        let http = builder
            .build()
            .map_err(|e| EnrollError::Client(e.to_string()))?;
        Ok(Self { http, origin })
    }

    pub fn origin(&self) -> &HubOrigin {
        &self.origin
    }

    /// Ask the hub to enroll `key`'s station `station_id`.
    pub async fn start(
        &self,
        key: &SigningKey,
        station_id: &str,
        label: Option<&str>,
    ) -> Result<StartResponse, EnrollError> {
        let body = wipe_license::enrollment::sign_start(key, station_id, label);
        let origin = self.origin.to_string();
        let resp = self
            .http
            .post(self.origin.url(ENROLLMENT_START_PATH))
            .json(&body)
            .send()
            .await
            .map_err(|e| EnrollError::HubUnreachable {
                origin: origin.clone(),
                reason: error_chain(&e),
            })?;
        let status = resp.status();
        let retry_after_secs = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse().ok());
        let text = resp.text().await.map_err(|e| EnrollError::HubUnreachable {
            origin: origin.clone(),
            reason: error_chain(&e),
        })?;
        if !status.is_success() {
            return Err(EnrollError::StartRefused {
                origin,
                status: status.as_u16(),
                message: error_text(&text, status),
                retry_after_secs,
            });
        }
        serde_json::from_str(&text).map_err(|e| EnrollError::BadResponse {
            origin,
            reason: e.to_string(),
        })
    }

    /// Ask how the enrollment for `device_code` stands.
    pub async fn poll(&self, key: &SigningKey, device_code: &str) -> PollOutcome {
        let body = wipe_license::enrollment::sign_poll(key, device_code);
        let resp = match self
            .http
            .post(self.origin.url(ENROLLMENT_POLL_PATH))
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return PollOutcome::Transient(error_chain(&e)),
        };
        let status = resp.status();
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => return PollOutcome::Transient(error_chain(&e)),
        };
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            return PollOutcome::Transient(format!("HTTP {status}"));
        }
        if status == StatusCode::OK {
            #[derive(Deserialize)]
            struct Collected {
                station: Value,
                licence: Value,
            }
            return match serde_json::from_str::<Collected>(&text) {
                Ok(c) => PollOutcome::Collected {
                    station: c.station,
                    licence: c.licence,
                },
                Err(e) => {
                    PollOutcome::Refused(format!("an approval this station cannot read: {e}"))
                }
            };
        }
        #[derive(Deserialize)]
        struct ErrorBody {
            error: String,
            #[serde(default)]
            interval: Option<u64>,
        }
        match (status, serde_json::from_str::<ErrorBody>(&text)) {
            (StatusCode::BAD_REQUEST, Ok(b)) => match b.error.as_str() {
                "authorization_pending" => PollOutcome::Pending,
                "slow_down" => PollOutcome::SlowDown {
                    interval: b.interval,
                },
                "access_denied" => PollOutcome::Denied,
                "expired_token" => PollOutcome::Expired,
                other => PollOutcome::Refused(other.to_string()),
            },
            _ => PollOutcome::Refused(format!("HTTP {status}: {}", error_text(&text, status))),
        }
    }
}

impl HubClient {
    /// A single-use challenge to sign the next station request over.
    pub async fn challenge(&self) -> Result<String, FetchOutcome> {
        let resp = self
            .http
            .get(self.origin.url(STATION_CHALLENGE_PATH))
            .send()
            .await
            .map_err(|e| FetchOutcome::Unreachable(error_chain(&e)))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| FetchOutcome::Unreachable(error_chain(&e)))?;
        if !status.is_success() {
            return Err(FetchOutcome::Refused(format!(
                "HTTP {}: {}",
                status.as_u16(),
                error_text(&text, status)
            )));
        }
        #[derive(Deserialize)]
        struct Challenge {
            challenge: String,
        }
        serde_json::from_str::<Challenge>(&text)
            .map(|c| c.challenge)
            .map_err(|e| {
                FetchOutcome::Refused(format!("a challenge this station cannot read: {e}"))
            })
    }

    /// Ask the hub for `key`'s latest licence, saying which one the station
    /// holds (`held_id`). Gets its own challenge first.
    pub async fn fetch_licence(&self, key: &SigningKey, held_id: Option<&str>) -> FetchOutcome {
        let challenge = match self.challenge().await {
            Ok(c) => c,
            Err(outcome) => return outcome,
        };
        let body = serde_json::json!({
            "instance_public_key": key.verifying_key().to_base64(),
            "licence_id": held_id,
        })
        .to_string();
        let signed = wipe_license::station_request::sign(
            key,
            &challenge,
            self.origin.as_str(),
            "POST",
            STATION_LICENCE_PATH,
            body.as_bytes(),
        );
        use wipe_license::station_request::{HEADER_CHALLENGE, HEADER_KEY_ID, HEADER_SIGNATURE};
        let resp = match self
            .http
            .post(self.origin.url(STATION_LICENCE_PATH))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(HEADER_KEY_ID, &signed.instance_key_id)
            .header(HEADER_CHALLENGE, &signed.challenge)
            .header(HEADER_SIGNATURE, &signed.signature_b64)
            .body(body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return FetchOutcome::Unreachable(error_chain(&e)),
        };
        let status = resp.status();
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => return FetchOutcome::Unreachable(error_chain(&e)),
        };
        match status {
            StatusCode::NO_CONTENT => FetchOutcome::Current,
            StatusCode::NOT_FOUND => FetchOutcome::NotRegistered,
            StatusCode::OK => {
                #[derive(Deserialize)]
                struct Fetched {
                    licence: Value,
                }
                match serde_json::from_str::<Fetched>(&text) {
                    Ok(f) => FetchOutcome::Licence(f.licence),
                    Err(e) => {
                        FetchOutcome::Refused(format!("an answer this station cannot read: {e}"))
                    }
                }
            }
            other => FetchOutcome::Refused(format!(
                "HTTP {}: {}",
                other.as_u16(),
                error_text(&text, other)
            )),
        }
    }
}

/// The `error` of a `{"error": ...}` body, else the status's reason.
fn error_text(body: &str, status: StatusCode) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_string())
}

/// `e` and its sources, joined: reqwest's own text is only "error sending
/// request".
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        let s_text = s.to_string();
        if !out.contains(&s_text) {
            out.push_str(": ");
            out.push_str(&s_text);
        }
        src = s.source();
    }
    out
}

// ---------------------------------------------------------------------------
// Enrollment loop

/// Time as the poll loop sees it, so tests run without waiting.
#[async_trait]
pub trait Clock: Send {
    /// Monotonic time since some fixed point.
    fn now(&self) -> Duration;
    async fn sleep(&mut self, d: Duration);
}

/// Real time.
pub struct SystemClock(tokio::time::Instant);

impl Default for SystemClock {
    fn default() -> Self {
        Self(tokio::time::Instant::now())
    }
}

#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    async fn sleep(&mut self, d: Duration) {
        tokio::time::sleep(d).await
    }
}

/// Who is enrolling, and what it trusts.
pub struct Enrollment<'a> {
    /// The station's instance key: it signs every request and the licence
    /// must name it.
    pub key: &'a SigningKey,
    /// Must match `serve --station-id`: a machine licence is bound to it.
    pub station_id: &'a str,
    pub label: Option<&'a str>,
    /// The only roots the licence may verify against.
    pub roots: &'a [VerifyingKey],
}

/// A verified licence from an approved enrollment.
#[derive(Debug, Clone)]
pub struct Enrolled {
    pub licence: LicenseCertificate,
    /// The hub's licence re-serialized as JSON (pretty-printed), for saving
    /// and installing: the same content, not the hub's bytes byte for byte.
    pub licence_json: String,
    /// The hub's record of the registered station.
    pub station: Value,
}

/// Start an enrollment, call `on_started` with the codes to show, then poll
/// until it ends. See the module doc for the rules.
pub async fn enroll(
    client: &HubClient,
    who: &Enrollment<'_>,
    clock: &mut dyn Clock,
    on_started: &mut (dyn FnMut(&StartResponse) + Send),
) -> Result<Enrolled, EnrollError> {
    let started = client.start(who.key, who.station_id, who.label).await?;
    on_started(&started);
    poll_until_done(client, who, &started, clock).await
}

/// The longest enrollment a station waits for, in seconds, whatever the
/// hub's `expires_in` says (the hub sends 600): a day.
pub const MAX_EXPIRES_IN_SECS: u64 = 86_400;

/// Poll the enrollment `started` began until it ends: the second half of
/// [`enroll`], for a caller that sends the start itself (the station's
/// enrollment routes answer with the start before polling begins). Expiry
/// counts from this call, capped at [`MAX_EXPIRES_IN_SECS`].
pub async fn poll_until_done(
    client: &HubClient,
    who: &Enrollment<'_>,
    started: &StartResponse,
    clock: &mut dyn Clock,
) -> Result<Enrolled, EnrollError> {
    let t0 = clock.now();
    let deadline = Duration::from_secs(started.expires_in.min(MAX_EXPIRES_IN_SECS));
    let mut interval = Duration::from_secs(started.interval.max(1));
    let origin = client.origin().to_string();
    loop {
        let left = deadline.saturating_sub(clock.now().saturating_sub(t0));
        if left.is_zero() {
            return Err(EnrollError::Expired);
        }
        clock.sleep(interval.min(left)).await;
        if clock.now().saturating_sub(t0) >= deadline {
            return Err(EnrollError::Expired);
        }
        match client.poll(who.key, &started.device_code).await {
            PollOutcome::Pending => {}
            PollOutcome::SlowDown { interval: next } => {
                interval = match next {
                    Some(s) => Duration::from_secs(s).max(interval),
                    None => interval + Duration::from_secs(5),
                };
            }
            PollOutcome::Transient(reason) => {
                tracing::warn!(%origin, %reason, "enrollment poll failed; retrying");
            }
            PollOutcome::Denied => return Err(EnrollError::Denied { origin }),
            PollOutcome::Expired => return Err(EnrollError::Expired),
            PollOutcome::Refused(reason) => {
                return Err(EnrollError::PollRefused { origin, reason })
            }
            PollOutcome::Collected { station, licence } => {
                let (licence, licence_json) =
                    check_licence(&origin, &licence, &who.key.public_key_id(), who.roots)?;
                return Ok(Enrolled {
                    licence,
                    licence_json,
                    station,
                });
            }
        }
    }
}

/// Parse the hub's licence and check it against `roots` and this station's
/// key, as `serve --license` and `POST /api/license` do. The licence and its
/// JSON.
pub fn check_licence(
    origin: &str,
    licence: &Value,
    station_key_id: &str,
    roots: &[VerifyingKey],
) -> Result<(LicenseCertificate, String), EnrollError> {
    let json = serde_json::to_string_pretty(licence).map_err(|e| EnrollError::BadResponse {
        origin: origin.to_string(),
        reason: e.to_string(),
    })?;
    let parsed = wipe_license::parse_license(&json).map_err(|e| EnrollError::BadResponse {
        origin: origin.to_string(),
        reason: e.to_string(),
    })?;
    match wipe_license::check_installable(&parsed, station_key_id, roots) {
        Ok(()) => Ok((parsed, json)),
        Err(InstallProblem::NotTrusted(reason)) => Err(EnrollError::UntrustedLicence {
            origin: origin.to_string(),
            reason,
        }),
        Err(InstallProblem::WrongInstanceKey { expected, found }) => Err(EnrollError::WrongKey {
            origin: origin.to_string(),
            expected,
            found,
        }),
    }
}

// ---------------------------------------------------------------------------
// Installing into a running station

/// What a running station said to `POST /api/license`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StationInstall {
    /// Installed; the station's licence status message.
    Installed { operator_message: String },
    /// Nothing answered at that address.
    NoStation { reason: String },
    /// The station answered with an error; its `error` text.
    Refused { status: u16, error: String },
}

/// Install `licence_json` into the station serving at `station_base` (for
/// example `http://127.0.0.1:7878`) through its runtime install route. Never
/// through a proxy: the station is local.
pub async fn install_on_station(station_base: &str, licence_json: &str) -> StationInstall {
    let url = format!("{}/api/license", station_base.trim_end_matches('/'));
    let http = match reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return StationInstall::Refused {
                status: 0,
                error: e.to_string(),
            }
        }
    };
    let resp = match http
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(licence_json.to_string())
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_connect() => {
            return StationInstall::NoStation {
                reason: error_chain(&e),
            }
        }
        Err(e) => {
            return StationInstall::Refused {
                status: 0,
                error: error_chain(&e),
            }
        }
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status == StatusCode::OK {
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("operator_message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "installed".to_string());
        return StationInstall::Installed {
            operator_message: message,
        };
    }
    StationInstall::Refused {
        status: status.as_u16(),
        error: error_text(&text, status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_origins_follow_the_rule() {
        for (raw, want) in [
            ("https://hub.example", "https://hub.example"),
            ("https://hub.example/", "https://hub.example"),
            ("HTTPS://Hub.Example:443/", "https://hub.example"),
            ("https://hub.example:8443", "https://hub.example:8443"),
            ("http://localhost:7900", "http://localhost:7900"),
            ("http://hub.localhost", "http://hub.localhost"),
            ("http://127.0.0.1:7900/", "http://127.0.0.1:7900"),
            ("http://127.9.9.9", "http://127.9.9.9"),
            ("http://[::1]:7900", "http://[::1]:7900"),
            ("http://[::ffff:127.0.0.1]", "http://[::ffff:7f00:1]"),
        ] {
            assert_eq!(HubOrigin::parse(raw).unwrap().as_str(), want, "{raw}");
        }
        for raw in [
            "http://hub.example",
            "http://192.168.1.10:7900",
            "http://[::2]",
            "ftp://hub.example",
            "hub.example",
            "https://user@hub.example",
            "https://user:pw@hub.example",
            "https://hub.example/api",
            "https://hub.example/?x=1",
            "https://hub.example/#f",
            "https://hub.example?",
            "",
        ] {
            let err = HubOrigin::parse(raw).unwrap_err().to_string();
            assert!(err.contains(HUB_ORIGIN_RULE), "{raw}: {err}");
        }
    }

    #[test]
    fn proxy_settings_prefer_upper_case_and_skip_empty_values() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            ProxySettings::from_lookup(env(&[
                ("HTTPS_PROXY", "http://a:1"),
                ("https_proxy", "http://b:1"),
                ("no_proxy", "x.example"),
            ])),
            ProxySettings {
                https_proxy: Some("http://a:1".into()),
                no_proxy: Some("x.example".into()),
            }
        );
        assert_eq!(
            ProxySettings::from_lookup(env(&[("HTTPS_PROXY", ""), ("https_proxy", "http://b:1")])),
            ProxySettings {
                https_proxy: Some("http://b:1".into()),
                no_proxy: None,
            }
        );
        // HTTP_PROXY and ALL_PROXY are not sources.
        assert_eq!(
            ProxySettings::from_lookup(env(&[("HTTP_PROXY", "http://c:1"), ("ALL_PROXY", "x")])),
            ProxySettings::default()
        );
    }
}
