//! The station's hub client against a fake hub (station-enrollment story 3).
//!
//! The fake hub verifies every start and poll signature with
//! `wipe_license::enrollment`, the hub's own verifier, and answers polls from
//! a script. Time is a fake clock, so nothing here waits.

use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wipe_cert::{SigningKey, VerifyingKey};
use wipe_engine_mock::{MockBackend, MockTiming};
use wipe_license::enrollment::{verify_poll, verify_start, PollRequest, StartRequest};
use wipe_server::hub_client::{
    enroll, install_on_station, Clock, EnrollError, Enrollment, HubClient, HubOrigin,
    ProxySettings, StationInstall,
};

// ---------------------------------------------------------------------------
// Fake hub

#[derive(Default)]
struct Script {
    /// Overrides the start answer: status, body, Retry-After.
    start: Option<(u16, Value, Option<u64>)>,
    expires_in: Option<u64>,
    /// Poll answers in order; once empty, `authorization_pending`.
    polls: VecDeque<(u16, Value)>,
    public_key_b64: Option<String>,
    starts: Vec<StartRequest>,
    polls_seen: usize,
    bad_signatures: usize,
}

type Hub = Arc<Mutex<Script>>;

/// A hub that answers polls with `answers`, in order.
fn polls<const N: usize>(answers: [(u16, Value); N]) -> Script {
    Script {
        polls: VecDeque::from(answers),
        ..Script::default()
    }
}

async fn start(State(hub): State<Hub>, Json(req): Json<StartRequest>) -> Response {
    let mut s = hub.lock().unwrap();
    if verify_start(&req).is_err() {
        s.bad_signatures += 1;
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "bad signature"})),
        )
            .into_response();
    }
    s.public_key_b64 = Some(req.public_key_b64.clone());
    s.starts.push(req);
    if let Some((status, body, retry)) = s.start.clone() {
        let mut headers = HeaderMap::new();
        if let Some(r) = retry {
            headers.insert("retry-after", HeaderValue::from(r));
        }
        return (StatusCode::from_u16(status).unwrap(), headers, Json(body)).into_response();
    }
    Json(json!({
        "device_code": "device-code-1",
        "user_code": "BCDF-GHJK",
        "verification_uri": "http://hub.test/activate",
        "verification_uri_complete": "http://hub.test/activate?code=BCDF-GHJK",
        "expires_in": s.expires_in.unwrap_or(600),
        "interval": 5,
    }))
    .into_response()
}

async fn poll(State(hub): State<Hub>, Json(req): Json<PollRequest>) -> Response {
    let mut s = hub.lock().unwrap();
    s.polls_seen += 1;
    let pk = s.public_key_b64.clone().unwrap_or_default();
    if verify_poll(&pk, &req).is_err() || req.device_code != "device-code-1" {
        s.bad_signatures += 1;
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    }
    match s.polls.pop_front() {
        Some((status, body)) => (StatusCode::from_u16(status).unwrap(), Json(body)).into_response(),
        None => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "authorization_pending"})),
        )
            .into_response(),
    }
}

async fn serve_on(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn fake_hub(script: Script) -> (HubOrigin, Hub) {
    let hub: Hub = Arc::new(Mutex::new(script));
    let app = Router::new()
        .route("/api/enrollments", post(start))
        .route("/api/enrollments/poll", post(poll))
        .with_state(hub.clone());
    let addr = serve_on(app).await;
    (HubOrigin::parse(&format!("http://{addr}")).unwrap(), hub)
}

/// A port nothing listens on.
async fn closed_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

// ---------------------------------------------------------------------------
// Licences and the clock

fn licence(root: &wipe_license::VendorRoot, key_id: &str) -> Value {
    let now = time::OffsetDateTime::now_utc();
    let ent = wipe_license::Entitlements {
        customer_id: "org-1".into(),
        customer_name: "Acme Recycling".into(),
        quota: wipe_license::Quota::Unlimited,
        scope: wipe_license::Scope::Site {
            site_id: "s".into(),
        },
        not_before: now - time::Duration::days(1),
        not_after: now + time::Duration::days(365),
        features: vec![],
        allowed_methods: wipe_license::AllowedMethods::All,
        machine_binding: None,
    };
    serde_json::to_value(root.issue("lic-enrolled", key_id, ent, now).unwrap()).unwrap()
}

fn collected(licence: Value) -> (u16, Value) {
    (
        200,
        json!({"station": {"station_id": "bench-1"}, "licence": licence}),
    )
}

fn error(code: &str) -> (u16, Value) {
    (400, json!({"error": code}))
}

/// Fake time: `sleep` moves `now` and records the duration.
#[derive(Default)]
struct FakeClock {
    now: Duration,
    sleeps: Vec<Duration>,
}

#[async_trait::async_trait]
impl Clock for FakeClock {
    fn now(&self) -> Duration {
        self.now
    }
    async fn sleep(&mut self, d: Duration) {
        self.now += d;
        self.sleeps.push(d);
    }
}

fn secs(list: &[u64]) -> Vec<Duration> {
    list.iter().map(|s| Duration::from_secs(*s)).collect()
}

struct Run {
    result: Result<wipe_server::hub_client::Enrolled, EnrollError>,
    clock: FakeClock,
    shown: Option<String>,
}

async fn run(
    origin: HubOrigin,
    proxy: &ProxySettings,
    key: &SigningKey,
    roots: &[VerifyingKey],
) -> Run {
    let client = HubClient::new(origin, proxy).unwrap();
    let mut clock = FakeClock::default();
    let mut shown = None;
    let who = Enrollment {
        key,
        station_id: "bench-1",
        label: Some("Bench 1"),
        roots,
    };
    let result = enroll(&client, &who, &mut clock, &mut |s| {
        shown = Some(s.user_code.clone())
    })
    .await;
    Run {
        result,
        clock,
        shown,
    }
}

// ---------------------------------------------------------------------------
// Outcomes

#[tokio::test]
async fn an_approved_enrollment_yields_a_licence_verified_against_the_stations_root() {
    let key = SigningKey::generate();
    let root = wipe_license::VendorRoot::generate();
    let script = polls([
        error("authorization_pending"),
        error("authorization_pending"),
        collected(licence(&root, &key.public_key_id())),
    ]);
    let (origin, hub) = fake_hub(script).await;
    let r = run(
        origin,
        &ProxySettings::default(),
        &key,
        &[root.verifying_key()],
    )
    .await;
    let enrolled = r.result.unwrap();
    assert_eq!(enrolled.licence.body.license_id, "lic-enrolled");
    assert_eq!(enrolled.station["station_id"], "bench-1");
    let reparsed = wipe_license::parse_license(&enrolled.licence_json).unwrap();
    assert_eq!(reparsed.body.instance_public_key_id, key.public_key_id());
    assert_eq!(r.shown.as_deref(), Some("BCDF-GHJK"));
    assert_eq!(r.clock.sleeps, secs(&[5, 5, 5]));
    let s = hub.lock().unwrap();
    assert_eq!(s.polls_seen, 3);
    assert_eq!(s.bad_signatures, 0);
    assert_eq!(s.starts[0].station_id, "bench-1");
    assert_eq!(s.starts[0].label.as_deref(), Some("Bench 1"));
}

#[tokio::test]
async fn a_denied_enrollment_says_so() {
    let key = SigningKey::generate();
    let script = polls([error("access_denied")]);
    let (origin, _) = fake_hub(script).await;
    let err = run(origin, &ProxySettings::default(), &key, &[])
        .await
        .result
        .unwrap_err();
    assert!(matches!(err, EnrollError::Denied { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(
        msg.contains("denied") && msg.contains("enroll again"),
        "{msg}"
    );
}

#[tokio::test]
async fn an_expired_token_says_start_again() {
    let key = SigningKey::generate();
    let script = polls([error("expired_token")]);
    let (origin, _) = fake_hub(script).await;
    let err = run(origin, &ProxySettings::default(), &key, &[])
        .await
        .result
        .unwrap_err();
    assert_eq!(err, EnrollError::Expired);
    assert!(err.to_string().contains("expired"), "{err}");
}

#[tokio::test]
async fn polling_stops_at_expires_in() {
    let key = SigningKey::generate();
    let script = Script {
        expires_in: Some(12),
        ..Script::default()
    };
    let (origin, hub) = fake_hub(script).await;
    let r = run(origin, &ProxySettings::default(), &key, &[]).await;
    assert_eq!(r.result.unwrap_err(), EnrollError::Expired);
    // 5 s, poll, 5 s, poll, then the last 2 s run out the code.
    assert_eq!(r.clock.sleeps, secs(&[5, 5, 2]));
    assert_eq!(hub.lock().unwrap().polls_seen, 2);
}

#[tokio::test]
async fn slow_down_moves_to_the_hubs_new_interval() {
    let key = SigningKey::generate();
    let root = wipe_license::VendorRoot::generate();
    let script = polls([
        (400, json!({"error": "slow_down", "interval": 10})),
        error("authorization_pending"),
        collected(licence(&root, &key.public_key_id())),
    ]);
    let (origin, _) = fake_hub(script).await;
    let r = run(
        origin,
        &ProxySettings::default(),
        &key,
        &[root.verifying_key()],
    )
    .await;
    r.result.unwrap();
    assert_eq!(r.clock.sleeps, secs(&[5, 10, 10]));
}

#[tokio::test]
async fn a_transient_hub_error_is_retried_at_the_interval() {
    let key = SigningKey::generate();
    let root = wipe_license::VendorRoot::generate();
    let script = polls([
        (503, json!({"error": "unavailable"})),
        (500, json!({})),
        collected(licence(&root, &key.public_key_id())),
    ]);
    let (origin, hub) = fake_hub(script).await;
    let r = run(
        origin,
        &ProxySettings::default(),
        &key,
        &[root.verifying_key()],
    )
    .await;
    r.result.unwrap();
    assert_eq!(r.clock.sleeps, secs(&[5, 5, 5]));
    assert_eq!(hub.lock().unwrap().polls_seen, 3);
}

#[tokio::test]
async fn an_invalid_grant_ends_the_enrollment() {
    let key = SigningKey::generate();
    let script = polls([error("invalid_grant")]);
    let (origin, _) = fake_hub(script).await;
    let err = run(origin, &ProxySettings::default(), &key, &[])
        .await
        .result
        .unwrap_err();
    assert!(matches!(err, EnrollError::PollRefused { .. }), "{err:?}");
    assert!(err.to_string().contains("invalid_grant"), "{err}");
}

#[tokio::test]
async fn a_licence_from_an_untrusted_root_is_refused() {
    let key = SigningKey::generate();
    let trusted = wipe_license::VendorRoot::generate();
    let other = wipe_license::VendorRoot::generate();
    let script = polls([collected(licence(&other, &key.public_key_id()))]);
    let (origin, _) = fake_hub(script).await;
    let err = run(
        origin,
        &ProxySettings::default(),
        &key,
        &[trusted.verifying_key()],
    )
    .await
    .result
    .unwrap_err();
    assert!(
        matches!(err, EnrollError::UntrustedLicence { .. }),
        "{err:?}"
    );
    let msg = err.to_string();
    assert!(msg.contains("does not trust"), "{msg}");
    assert!(msg.contains("--vendor-root-b64"), "{msg}");
    assert!(msg.contains("nothing was saved or installed"), "{msg}");
}

#[tokio::test]
async fn a_licence_for_another_key_names_both_keys() {
    let key = SigningKey::generate();
    let other = SigningKey::generate();
    let root = wipe_license::VendorRoot::generate();
    let script = polls([collected(licence(&root, &other.public_key_id()))]);
    let (origin, _) = fake_hub(script).await;
    let err = run(
        origin,
        &ProxySettings::default(),
        &key,
        &[root.verifying_key()],
    )
    .await
    .result
    .unwrap_err();
    assert!(matches!(err, EnrollError::WrongKey { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains(&key.public_key_id()), "{msg}");
    assert!(msg.contains(&other.public_key_id()), "{msg}");
}

#[tokio::test]
async fn a_throttled_start_is_refused_with_the_hubs_message_and_retry_after() {
    let key = SigningKey::generate();
    let script = Script {
        start: Some((
            429,
            json!({"error": "Too many enrollments from this address."}),
            Some(3600),
        )),
        ..Script::default()
    };
    let (origin, hub) = fake_hub(script).await;
    let r = run(origin, &ProxySettings::default(), &key, &[]).await;
    let err = r.result.unwrap_err();
    assert!(
        matches!(err, EnrollError::StartRefused { status: 429, .. }),
        "{err:?}"
    );
    let msg = err.to_string();
    assert!(msg.contains("Too many enrollments"), "{msg}");
    assert!(msg.contains("3600 seconds"), "{msg}");
    assert!(r.shown.is_none(), "no code is shown");
    assert_eq!(hub.lock().unwrap().polls_seen, 0);
}

#[tokio::test]
async fn an_unreachable_hub_at_start_names_the_hub() {
    let key = SigningKey::generate();
    let origin = HubOrigin::parse(&format!("http://127.0.0.1:{}", closed_port().await)).unwrap();
    let r = run(origin.clone(), &ProxySettings::default(), &key, &[]).await;
    let err = r.result.unwrap_err();
    assert!(matches!(err, EnrollError::HubUnreachable { .. }), "{err:?}");
    assert!(err.to_string().contains(origin.as_str()), "{err}");
}

// ---------------------------------------------------------------------------
// Proxy

/// A TCP listener standing in for a proxy: the first line of each
/// connection, sent on `lines`; the connection is then closed.
async fn fake_proxy() -> (u16, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 1024];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = tx.send(text.lines().next().unwrap_or_default().to_string());
            let _ = sock
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\n\r\n")
                .await;
        }
    });
    (port, rx)
}

#[tokio::test]
async fn an_https_hub_is_reached_through_https_proxy() {
    let (port, mut lines) = fake_proxy().await;
    let proxy = ProxySettings {
        https_proxy: Some(format!("http://127.0.0.1:{port}")),
        no_proxy: None,
    };
    let origin = HubOrigin::parse("https://hub.example").unwrap();
    let client = HubClient::new(origin, &proxy).unwrap();
    let err = client
        .start(&SigningKey::generate(), "bench-1", None)
        .await
        .unwrap_err();
    assert!(matches!(err, EnrollError::HubUnreachable { .. }), "{err:?}");
    let first = tokio::time::timeout(Duration::from_secs(5), lines.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first, "CONNECT hub.example:443 HTTP/1.1");
}

#[tokio::test]
async fn no_proxy_sends_a_matching_hub_direct() {
    let (port, mut lines) = fake_proxy().await;
    let hub_port = closed_port().await;
    let proxy = ProxySettings {
        https_proxy: Some(format!("http://127.0.0.1:{port}")),
        no_proxy: Some("other.example, 127.0.0.1".into()),
    };
    let origin = HubOrigin::parse(&format!("https://127.0.0.1:{hub_port}")).unwrap();
    let client = HubClient::new(origin, &proxy).unwrap();
    let err = client
        .start(&SigningKey::generate(), "bench-1", None)
        .await
        .unwrap_err();
    assert!(matches!(err, EnrollError::HubUnreachable { .. }), "{err:?}");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), lines.recv())
            .await
            .is_err(),
        "the proxy was used despite NO_PROXY"
    );

    // Without the NO_PROXY match, the same hub goes through the proxy.
    let proxy = ProxySettings {
        no_proxy: Some("other.example".into()),
        ..proxy
    };
    let origin = HubOrigin::parse(&format!("https://127.0.0.1:{hub_port}")).unwrap();
    let client = HubClient::new(origin, &proxy).unwrap();
    let _ = client.start(&SigningKey::generate(), "bench-1", None).await;
    let first = tokio::time::timeout(Duration::from_secs(5), lines.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first, format!("CONNECT 127.0.0.1:{hub_port} HTTP/1.1"));
}

#[tokio::test]
async fn an_http_loopback_hub_never_uses_the_https_proxy() {
    let (port, mut lines) = fake_proxy().await;
    let proxy = ProxySettings {
        https_proxy: Some(format!("http://127.0.0.1:{port}")),
        no_proxy: None,
    };
    let key = SigningKey::generate();
    let root = wipe_license::VendorRoot::generate();
    let script = polls([collected(licence(&root, &key.public_key_id()))]);
    let (origin, _) = fake_hub(script).await;
    run(origin, &proxy, &key, &[root.verifying_key()])
        .await
        .result
        .unwrap();
    assert!(lines.try_recv().is_err(), "the proxy saw a request");
}

#[tokio::test]
async fn a_malformed_https_proxy_is_refused() {
    let proxy = ProxySettings {
        https_proxy: Some("::not a url::".into()),
        no_proxy: None,
    };
    let err = HubClient::new(HubOrigin::parse("https://hub.example").unwrap(), &proxy).unwrap_err();
    assert!(err.to_string().contains("HTTPS_PROXY"), "{err}");
}

// ---------------------------------------------------------------------------
// Installing into a running station

async fn station(roots: Vec<VerifyingKey>) -> (String, String) {
    let key = SigningKey::generate();
    let key_id = key.public_key_id();
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let state = wipe_server::AppState::new(backend, None, Arc::new(key)).with_vendor_roots(roots);
    let addr = serve_on(wipe_server::router(state)).await;
    (format!("http://{addr}"), key_id)
}

#[tokio::test]
async fn a_verified_licence_installs_into_a_running_station() {
    let root = wipe_license::VendorRoot::generate();
    let (base, key_id) = station(vec![root.verifying_key()]).await;
    let json = serde_json::to_string(&licence(&root, &key_id)).unwrap();
    match install_on_station(&format!("{base}/"), &json).await {
        StationInstall::Installed { operator_message } => {
            assert!(!operator_message.is_empty())
        }
        other => panic!("{other:?}"),
    }
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let view: Value = http
        .get(format!("{base}/api/license"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(view["status"]["status"], "valid", "{view}");
}

#[tokio::test]
async fn a_station_refusal_and_no_station_are_told_apart() {
    let root = wipe_license::VendorRoot::generate();
    let (base, key_id) = station(Vec::new()).await;
    let json = serde_json::to_string(&licence(&root, &key_id)).unwrap();
    match install_on_station(&base, &json).await {
        StationInstall::Refused { status, error } => {
            assert_eq!(status, 409);
            assert!(error.contains("no vendor root"), "{error}");
        }
        other => panic!("{other:?}"),
    }
    let none = format!("http://127.0.0.1:{}", closed_port().await);
    assert!(matches!(
        install_on_station(&none, &json).await,
        StationInstall::NoStation { .. }
    ));
}
