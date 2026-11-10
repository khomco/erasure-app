//! Enrollment from the bench app through the station's HTTP routes
//! (station-enrollment story 4): `GET`/`POST /api/enrollment` and
//! `POST /api/enrollment/cancel`, against a fake hub.
//!
//! The fake hub verifies every start and poll signature with
//! `wipe_license::enrollment` (the hub's own verifier), issues a new device
//! code per start, and answers polls from a script. It asks for a 1-second
//! poll interval, so an attempt ends within a few seconds of real time.

use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    path::{Path, PathBuf},
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
use wipe_cert::{SigningKey, VerifyingKey};
use wipe_engine_mock::{MockBackend, MockTiming};
use wipe_license::enrollment::{verify_poll, verify_start, PollRequest, StartRequest};
use wipe_server::{hub_client::HUB_ORIGIN_RULE, router, AppState};

// ---------------------------------------------------------------------------
// Fake hub

#[derive(Default)]
struct Script {
    /// Overrides the start answer: status, body, Retry-After.
    start: Option<(u16, Value, Option<u64>)>,
    expires_in: Option<u64>,
    /// Overrides `verification_uri_complete` in the start answer.
    complete_link: Option<String>,
    /// Poll answers in order; once empty, `authorization_pending`.
    polls: VecDeque<(u16, Value)>,
    public_key_b64: Option<String>,
    starts: Vec<StartRequest>,
    /// Polls seen, by device code.
    polls_by_code: HashMap<String, usize>,
    bad_signatures: usize,
}

type Hub = Arc<Mutex<Script>>;

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
    let n = s.starts.len();
    let user_code = format!("BCDF-GHJ{}", ["K", "L", "M", "N"][(n - 1) % 4]);
    Json(json!({
        "device_code": format!("device-code-{n}"),
        "user_code": user_code,
        "verification_uri": "http://hub.test/activate",
        "verification_uri_complete": s
            .complete_link
            .clone()
            .unwrap_or_else(|| format!("http://hub.test/activate?code={user_code}")),
        "expires_in": s.expires_in.unwrap_or(600),
        "interval": 1,
    }))
    .into_response()
}

async fn poll(State(hub): State<Hub>, Json(req): Json<PollRequest>) -> Response {
    let mut s = hub.lock().unwrap();
    *s.polls_by_code.entry(req.device_code.clone()).or_default() += 1;
    let pk = s.public_key_b64.clone().unwrap_or_default();
    if verify_poll(&pk, &req).is_err() || !req.device_code.starts_with("device-code-") {
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

async fn fake_hub(script: Script) -> (String, Hub) {
    let hub: Hub = Arc::new(Mutex::new(script));
    let app = Router::new()
        .route("/api/enrollments", post(start))
        .route("/api/enrollments/poll", post(poll))
        .with_state(hub.clone());
    let addr = serve_on(app).await;
    (format!("http://{addr}"), hub)
}

async fn closed_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

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

// ---------------------------------------------------------------------------
// The station

struct Station {
    base: String,
    key: Arc<SigningKey>,
    /// The settings directory, when the station keeps configuration on disk.
    dir: Option<PathBuf>,
}

impl Drop for Station {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn tmpdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wipestation-station-enrollment-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A station trusting `roots`, licensed as `bench-1`, with its instance key
/// kept in a key file; `on_disk` gives it a settings directory (Detect), else
/// it keeps nothing (InMemory).
async fn station(roots: Vec<VerifyingKey>, on_disk: bool) -> Station {
    station_with(roots, on_disk, true).await
}

/// As [`station`]; `key_kept: false` is `serve` without `--key-path`.
async fn station_with(roots: Vec<VerifyingKey>, on_disk: bool, key_kept: bool) -> Station {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let key = Arc::new(SigningKey::generate());
    let dir = on_disk.then(tmpdir);
    let state = AppState::new(backend, None, key.clone())
        .with_vendor_roots(roots)
        .with_station_id("bench-1")
        .with_settings_path(dir.as_ref().map(|d| d.join("station-settings.json")))
        .with_instance_key_kept(key_kept);
    let addr = serve_on(router(state)).await;
    Station {
        base: format!("http://{addr}"),
        key,
        dir,
    }
}

/// A client for the local station: never through a proxy (and so without the
/// system proxy lookup, which can take seconds on macOS). One per request:
/// each test has its own runtime, and a pooled connection must not outlive it.
fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

async fn get(base: &str, path: &str) -> (u16, Value) {
    let r = http().get(format!("{base}{path}")).send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap())
}

async fn post_json(base: &str, path: &str, body: Value) -> (u16, Value) {
    let r = http()
        .post(format!("{base}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap())
}

async fn start_enrollment(base: &str, hub: &str) -> (u16, Value) {
    post_json(
        base,
        "/api/enrollment",
        json!({"hub_origin": hub, "label": "Bench 1"}),
    )
    .await
}

/// `GET /api/enrollment` until the attempt is no longer pending.
async fn ended(base: &str) -> Value {
    for _ in 0..100 {
        let (status, view) = get(base, "/api/enrollment").await;
        assert_eq!(status, 200, "{view}");
        if view["attempt"]["state"] != "pending" {
            return view["attempt"].clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the attempt never ended");
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// Matrix

#[tokio::test]
async fn an_approved_enrollment_installs_and_saves_the_licence_and_hub() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (hub_origin, hub) =
        fake_hub(polls([collected(licence(&root, &st.key.public_key_id()))])).await;

    let (status, view) = get(&st.base, "/api/enrollment").await;
    assert_eq!(status, 200);
    assert_eq!(
        view,
        json!({"saved_hub_origin": null, "can_enroll": true, "cannot_enroll_reason": null,
               "attempt": null})
    );

    let (status, view) = start_enrollment(&st.base, &hub_origin).await;
    assert_eq!(status, 200, "{view}");
    let attempt = &view["attempt"];
    assert_eq!(attempt["state"], "pending", "{view}");
    assert_eq!(attempt["hub_origin"], hub_origin);
    assert_eq!(attempt["user_code"], "BCDF-GHJK");
    assert_eq!(attempt["verification_uri"], "http://hub.test/activate");
    assert_eq!(
        attempt["verification_uri_complete"],
        "http://hub.test/activate?code=BCDF-GHJK"
    );
    assert_eq!(attempt["interval"], 1);
    let left = attempt["seconds_left"].as_u64().unwrap();
    assert!((595..=600).contains(&left), "{left}");
    assert!(
        !view.to_string().contains("device-code"),
        "the device code leaked: {view}"
    );

    let attempt = ended(&st.base).await;
    assert_eq!(attempt["state"], "installed", "{attempt}");
    assert_eq!(attempt["licence_id"], "lic-enrolled");
    assert_eq!(attempt["customer_name"], "Acme Recycling");
    assert!(attempt["not_after"].as_str().unwrap().ends_with('Z'));
    let dir = st.dir.clone().unwrap();
    let saved = dir.join("enrolled-licence.json");
    assert_eq!(attempt["saved_to"], saved.display().to_string());
    let message = attempt["message"].as_str().unwrap();
    assert!(message.contains("until it restarts"), "{message}");
    assert!(
        message.contains(&format!("--license {}", saved.display())),
        "{message}"
    );

    // The licence is installed, saved, and the hub address kept.
    let (_, licence_view) = get(&st.base, "/api/license").await;
    assert_eq!(licence_view["status"]["status"], "valid", "{licence_view}");
    assert_eq!(
        attempt["operator_message"],
        licence_view["operator_message"]
    );
    let on_disk = wipe_license::parse_license(&std::fs::read_to_string(&saved).unwrap()).unwrap();
    assert_eq!(on_disk.body.license_id, "lic-enrolled");
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("station-settings.json")).unwrap())
            .unwrap();
    assert_eq!(settings, json!({"hub_origin": hub_origin}));
    let (_, view) = get(&st.base, "/api/enrollment").await;
    assert_eq!(view["saved_hub_origin"], hub_origin);

    // The hub saw the station's id, label and signatures.
    let s = hub.lock().unwrap();
    assert_eq!(s.starts.len(), 1);
    assert_eq!(s.starts[0].station_id, "bench-1");
    assert_eq!(s.starts[0].label.as_deref(), Some("Bench 1"));
    assert_eq!(s.starts[0].instance_key_id, st.key.public_key_id());
    assert_eq!(s.bad_signatures, 0);
}

#[tokio::test]
async fn a_denied_enrollment_ends_denied() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (hub_origin, _) = fake_hub(polls([error("access_denied")])).await;
    let (status, _) = start_enrollment(&st.base, &hub_origin).await;
    assert_eq!(status, 200);
    let attempt = ended(&st.base).await;
    assert_eq!(attempt["state"], "denied", "{attempt}");
    assert_eq!(attempt["hub_origin"], hub_origin);
    let message = attempt["message"].as_str().unwrap();
    assert!(message.contains("denied"), "{message}");
    assert!(
        message.contains("Nothing was saved or installed"),
        "{message}"
    );
    // Worded for the bench app, which offers Start again.
    assert!(!message.contains("wipestation"), "{message}");
    assert_eq!(files_in(st.dir.as_ref().unwrap()), Vec::<String>::new());
}

#[tokio::test]
async fn an_expired_token_ends_expired() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (hub_origin, _) = fake_hub(polls([error("expired_token")])).await;
    start_enrollment(&st.base, &hub_origin).await;
    let attempt = ended(&st.base).await;
    assert_eq!(attempt["state"], "expired", "{attempt}");
    assert!(attempt["message"].as_str().unwrap().contains("expired"));
}

#[tokio::test]
async fn an_attempt_past_expires_in_ends_expired() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (hub_origin, _) = fake_hub(Script {
        expires_in: Some(1),
        ..Script::default()
    })
    .await;
    let (_, view) = start_enrollment(&st.base, &hub_origin).await;
    assert!(view["attempt"]["seconds_left"].as_u64().unwrap() <= 1);
    let attempt = ended(&st.base).await;
    assert_eq!(attempt["state"], "expired", "{attempt}");
}

#[tokio::test]
async fn an_unreachable_hub_answers_200_with_the_attempt_unreachable() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let hub_origin = format!("http://127.0.0.1:{}", closed_port().await);
    let (status, view) = start_enrollment(&st.base, &hub_origin).await;
    assert_eq!(status, 200, "{view}");
    let attempt = &view["attempt"];
    assert_eq!(attempt["state"], "unreachable", "{view}");
    assert!(
        attempt["message"].as_str().unwrap().contains(&hub_origin),
        "{view}"
    );
}

#[tokio::test]
async fn a_hub_refusing_the_start_ends_refused_with_a_retry_hint() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (hub_origin, _) = fake_hub(Script {
        start: Some((429, json!({"error": "too many enrollments"}), Some(30))),
        ..Script::default()
    })
    .await;
    let (status, view) = start_enrollment(&st.base, &hub_origin).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["attempt"]["state"], "refused", "{view}");
    let message = view["attempt"]["message"].as_str().unwrap();
    assert!(message.contains("Try again in 30 seconds"), "{message}");
}

#[tokio::test]
async fn an_untrusted_licence_is_refused_and_neither_saved_nor_installed() {
    let root = wipe_license::VendorRoot::generate();
    let other = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (hub_origin, _) =
        fake_hub(polls([collected(licence(&other, &st.key.public_key_id()))])).await;
    start_enrollment(&st.base, &hub_origin).await;
    let attempt = ended(&st.base).await;
    assert_eq!(attempt["state"], "refused", "{attempt}");
    assert!(
        attempt["message"]
            .as_str()
            .unwrap()
            .contains("does not trust"),
        "{attempt}"
    );
    assert_eq!(files_in(st.dir.as_ref().unwrap()), Vec::<String>::new());
    let (_, licence_view) = get(&st.base, "/api/license").await;
    assert_eq!(licence_view["status"]["status"], "unlicensed");
}

#[tokio::test]
async fn a_licence_for_another_key_is_refused() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (hub_origin, _) =
        fake_hub(polls([collected(licence(&root, "ed25519:someone-else"))])).await;
    start_enrollment(&st.base, &hub_origin).await;
    let attempt = ended(&st.base).await;
    assert_eq!(attempt["state"], "refused", "{attempt}");
    assert!(attempt["message"]
        .as_str()
        .unwrap()
        .contains("ed25519:someone-else"));
    assert_eq!(files_in(st.dir.as_ref().unwrap()), Vec::<String>::new());
}

#[tokio::test]
async fn a_bad_hub_address_is_refused_before_any_request() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (status, body) = start_enrollment(&st.base, "http://hub.example").await;
    assert_eq!(status, 400);
    assert!(
        body["error"].as_str().unwrap().contains(HUB_ORIGIN_RULE),
        "{body}"
    );
    let (_, view) = get(&st.base, "/api/enrollment").await;
    assert_eq!(view["attempt"], Value::Null);
}

#[tokio::test]
async fn an_over_long_label_is_refused_before_any_request() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (hub_origin, hub) = fake_hub(Script::default()).await;
    let (status, body) = post_json(
        &st.base,
        "/api/enrollment",
        json!({"hub_origin": hub_origin, "label": "x".repeat(201)}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"].as_str().unwrap().contains("200"), "{body}");
    assert!(hub.lock().unwrap().starts.is_empty());
    // 200 characters, and no label at all, are fine.
    let (status, _) = post_json(
        &st.base,
        "/api/enrollment",
        json!({"hub_origin": hub_origin, "label": "é".repeat(200)}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = post_json(
        &st.base,
        "/api/enrollment",
        json!({"hub_origin": hub_origin}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(hub.lock().unwrap().starts[1].label, None);
}

#[tokio::test]
async fn a_station_with_no_vendor_root_cannot_enroll() {
    let st = station(vec![], true).await;
    let (hub_origin, hub) = fake_hub(Script::default()).await;
    let (_, view) = get(&st.base, "/api/enrollment").await;
    assert_eq!(view["can_enroll"], false);
    let reason = view["cannot_enroll_reason"].as_str().unwrap();
    assert!(reason.contains("--vendor-root-b64"), "{reason}");
    let (status, body) = start_enrollment(&st.base, &hub_origin).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"], reason);
    assert!(hub.lock().unwrap().starts.is_empty());
}

#[tokio::test]
async fn a_second_start_replaces_the_pending_attempt() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (hub_origin, hub) = fake_hub(Script::default()).await;
    let (_, first) = start_enrollment(&st.base, &hub_origin).await;
    assert_eq!(first["attempt"]["user_code"], "BCDF-GHJK");
    // Let the first attempt poll at least once.
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let (status, second) = start_enrollment(&st.base, &hub_origin).await;
    assert_eq!(status, 200);
    assert_eq!(second["attempt"]["state"], "pending");
    assert_eq!(second["attempt"]["user_code"], "BCDF-GHJL");
    let first_polls = hub.lock().unwrap().polls_by_code["device-code-1"];
    assert!(first_polls >= 1);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let s = hub.lock().unwrap();
    assert_eq!(
        s.polls_by_code["device-code-1"], first_polls,
        "the replaced attempt kept polling"
    );
    assert!(s.polls_by_code.get("device-code-2").copied().unwrap_or(0) >= 1);
    assert_eq!(s.starts.len(), 2);
}

#[tokio::test]
async fn cancel_stops_polling_and_clears_the_attempt() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (hub_origin, hub) = fake_hub(Script::default()).await;
    start_enrollment(&st.base, &hub_origin).await;
    let (status, view) = post_json(&st.base, "/api/enrollment/cancel", json!(null)).await;
    assert_eq!(status, 200);
    assert_eq!(view["attempt"], Value::Null);
    let seen = hub.lock().unwrap().polls_by_code.values().sum::<usize>();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        hub.lock().unwrap().polls_by_code.values().sum::<usize>(),
        seen,
        "polling continued after cancel"
    );
    let (_, view) = get(&st.base, "/api/enrollment").await;
    assert_eq!(view["attempt"], Value::Null);
}

#[tokio::test]
async fn an_in_memory_station_installs_but_saves_nothing() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], false).await;
    let (hub_origin, _) =
        fake_hub(polls([collected(licence(&root, &st.key.public_key_id()))])).await;
    start_enrollment(&st.base, &hub_origin).await;
    let attempt = ended(&st.base).await;
    assert_eq!(attempt["state"], "installed", "{attempt}");
    assert_eq!(attempt["saved_to"], Value::Null);
    let message = attempt["message"].as_str().unwrap();
    assert!(message.contains("could not be saved"), "{message}");
    assert!(!message.contains("--license /"), "{message}");
    let (_, licence_view) = get(&st.base, "/api/license").await;
    assert_eq!(licence_view["status"]["status"], "valid");
    let (_, view) = get(&st.base, "/api/enrollment").await;
    assert_eq!(view["saved_hub_origin"], Value::Null);
}

#[tokio::test]
async fn an_invalid_saved_hub_address_is_not_offered() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let path = st.dir.as_ref().unwrap().join("station-settings.json");
    std::fs::write(&path, r#"{"hub_origin":"http://hub.example"}"#).unwrap();
    let (_, view) = get(&st.base, "/api/enrollment").await;
    assert_eq!(view["saved_hub_origin"], Value::Null);
    std::fs::write(&path, r#"{"hub_origin":"https://hub.example/"}"#).unwrap();
    let (_, view) = get(&st.base, "/api/enrollment").await;
    assert_eq!(view["saved_hub_origin"], "https://hub.example");
}

#[tokio::test]
async fn a_station_whose_key_is_not_kept_cannot_enroll() {
    let root = wipe_license::VendorRoot::generate();
    let st = station_with(vec![root.verifying_key()], true, false).await;
    let (hub_origin, hub) = fake_hub(Script::default()).await;
    let (_, view) = get(&st.base, "/api/enrollment").await;
    assert_eq!(view["can_enroll"], false, "{view}");
    let reason = view["cannot_enroll_reason"].as_str().unwrap();
    assert!(reason.contains("--key-path"), "{reason}");
    let (status, body) = start_enrollment(&st.base, &hub_origin).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"], reason);
    assert!(
        hub.lock().unwrap().starts.is_empty(),
        "the hub was contacted"
    );
}

#[tokio::test]
async fn a_start_with_a_non_web_approval_link_ends_refused_without_polling() {
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let (hub_origin, hub) = fake_hub(Script {
        complete_link: Some("javascript:alert(document.domain)".into()),
        ..Script::default()
    })
    .await;
    let (status, view) = start_enrollment(&st.base, &hub_origin).await;
    assert_eq!(status, 200, "{view}");
    let attempt = &view["attempt"];
    assert_eq!(attempt["state"], "refused", "{view}");
    let message = attempt["message"].as_str().unwrap();
    assert!(message.contains("verification_uri_complete"), "{message}");
    assert!(!view.to_string().contains("javascript:"), "{view}");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        hub.lock().unwrap().polls_by_code.is_empty(),
        "a refused start was polled"
    );
    let (_, again) = get(&st.base, "/api/enrollment").await;
    assert_eq!(again["attempt"]["state"], "refused");
    assert_eq!(files_in(st.dir.as_ref().unwrap()), Vec::<String>::new());
}

#[cfg(unix)]
#[tokio::test]
async fn an_unwritable_settings_dir_installs_but_says_nothing_was_saved() {
    use std::os::unix::fs::PermissionsExt;
    let root = wipe_license::VendorRoot::generate();
    let st = station(vec![root.verifying_key()], true).await;
    let dir = st.dir.clone().unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(dir.join("probe"), b"x").is_ok() {
        // Running as root: permissions do not bind, so the case cannot be made.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let (hub_origin, _) =
        fake_hub(polls([collected(licence(&root, &st.key.public_key_id()))])).await;
    start_enrollment(&st.base, &hub_origin).await;
    let attempt = ended(&st.base).await;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(attempt["state"], "installed", "{attempt}");
    assert_eq!(attempt["saved_to"], Value::Null);
    let message = attempt["message"].as_str().unwrap();
    assert!(message.contains("could not be saved"), "{message}");
    assert!(!message.contains("--license"), "{message}");
    assert!(
        message.contains("platform address was not saved"),
        "{message}"
    );
    assert!(
        message.contains(&dir.join("enrolled-licence.json").display().to_string()),
        "the message does not name the licence file: {message}"
    );
    assert_eq!(files_in(&dir), Vec::<String>::new());
    let (_, licence_view) = get(&st.base, "/api/license").await;
    assert_eq!(licence_view["status"]["status"], "valid");
}
