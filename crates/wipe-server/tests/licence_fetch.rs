//! A connected station fetches its own renewed licence (station-enrollment
//! story 5, ADR-0008 §3), against a fake hub that verifies every request
//! with `wipe_license::station_request::verify` (the hub's own verifier)
//! over a challenge it issued, and answers from a script.
//!
//! Every station row of the story's I/O matrix: an installed renewal, up to
//! date, not registered, an untrusted root, an unreachable hub, a hanging
//! hub while a Job starts and finishes on the mock backend, no hub address;
//! the loop's sleeps under a fake clock; and `build_station` checking at
//! start-up.

use std::{
    collections::VecDeque,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use wipe_cert::SigningKey;
use wipe_engine_mock::{MockBackend, MockTiming};
use wipe_license::station_request::{
    self, SignedStationRequest, HEADER_CHALLENGE, HEADER_KEY_ID, HEADER_SIGNATURE,
};
use wipe_license::VendorRoot;
use wipe_server::bootstrap::{build_station, Fleet, Persistence, StationConfig};
use wipe_server::hub_client::Clock;
use wipe_server::licence_fetch::{
    check_once, check_once_with, run_loop, spawn, CHECK_INTERVAL, STILL_APPLIES,
};
use wipe_server::store::{save_station_settings, StationSettings, ENROLLED_LICENCE_FILE};
use wipe_server::{router, AppState};

// ---------------------------------------------------------------------------
// Fake hub

/// What the hub answers to a valid fetch.
#[derive(Clone)]
enum Answer {
    /// 200 with a licence `root` signs for the requesting key, `days` long.
    Licence {
        root: Arc<VendorRoot>,
        id: &'static str,
        days: i64,
    },
    /// Any status and body.
    Raw(u16, Value),
}

#[derive(Default)]
struct Script {
    /// In order; once empty, 204.
    answers: VecDeque<Answer>,
    issued: Vec<String>,
    /// Every verified request body.
    fetched: Vec<Value>,
    bad_requests: usize,
}

type Hub = Arc<Mutex<Script>>;

async fn challenge(State(hub): State<Hub>) -> Json<Value> {
    let mut s = hub.lock().unwrap();
    let c = format!("challenge-{}", s.issued.len());
    s.issued.push(c.clone());
    Json(json!({"challenge": c, "expires_in": 60}))
}

fn header(h: &HeaderMap, name: &str) -> String {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Verified as the hub verifies: the key from the body, its id, the
/// signature over this hub's origin, and a challenge used once.
async fn licence(
    State((hub, origin)): State<(Hub, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut s = hub.lock().unwrap();
    let req: Value = serde_json::from_slice(&body).unwrap_or_default();
    let signed = SignedStationRequest {
        instance_key_id: header(&headers, HEADER_KEY_ID),
        challenge: header(&headers, HEADER_CHALLENGE),
        signature_b64: header(&headers, HEADER_SIGNATURE),
    };
    let verified = req["instance_public_key"]
        .as_str()
        .and_then(|k| wipe_license::enrollment::decode_public_key(k).ok())
        .is_some_and(|key| {
            station_request::verify(
                &key,
                &signed,
                &origin,
                "POST",
                "/api/station/licence",
                &body,
            )
            .is_ok()
        });
    let fresh = s
        .issued
        .iter()
        .position(|c| *c == signed.challenge)
        .map(|i| s.issued.remove(i))
        .is_some();
    if !verified || !fresh {
        s.bad_requests += 1;
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "bad request"})),
        )
            .into_response();
    }
    s.fetched.push(req);
    match s.answers.pop_front() {
        None => StatusCode::NO_CONTENT.into_response(),
        Some(Answer::Raw(status, body)) => {
            (StatusCode::from_u16(status).unwrap(), Json(body)).into_response()
        }
        Some(Answer::Licence { root, id, days }) => Json(json!({
            "licence": signed_licence(&root, id, &signed.instance_key_id, days)
        }))
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

/// A fake hub answering `answers`: its origin and script.
async fn fake_hub(answers: Vec<Answer>) -> (String, Hub) {
    let hub: Hub = Arc::new(Mutex::new(Script {
        answers: answers.into(),
        ..Script::default()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let origin = format!("http://{addr}");
    let app = Router::new()
        .route("/api/station/challenge", get(challenge))
        .with_state(hub.clone())
        .merge(
            Router::new()
                .route("/api/station/licence", post(licence))
                .with_state((hub.clone(), origin.clone())),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (origin, hub)
}

/// A hub that accepts connections and never answers.
async fn hanging_hub() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (conn, _) = listener.accept().await.unwrap();
            held.push(conn);
        }
    });
    format!("http://{addr}")
}

/// An address nothing listens on.
async fn closed_port() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    format!("http://{}", l.local_addr().unwrap())
}

fn signed_licence(root: &VendorRoot, id: &str, key_id: &str, days: i64) -> Value {
    let now = time::OffsetDateTime::now_utc();
    let ent = wipe_license::Entitlements {
        customer_id: "org-1".into(),
        customer_name: "Acme Recycling".into(),
        quota: wipe_license::Quota::Unlimited,
        scope: wipe_license::Scope::Site {
            site_id: "s".into(),
        },
        not_before: now - time::Duration::days(1),
        not_after: now + time::Duration::days(days),
        features: vec![],
        allowed_methods: wipe_license::AllowedMethods::All,
        machine_binding: None,
    };
    serde_json::to_value(root.issue(id, key_id, ent, now).unwrap()).unwrap()
}

// ---------------------------------------------------------------------------
// Station

struct Station {
    state: AppState,
    base: String,
    key: Arc<SigningKey>,
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
        "wipestation-licence-fetch-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A served station trusting `root`, holding a licence `lic-old` (100 days)
/// from it, with its settings in a temp directory naming `hub` when given.
async fn station(root: &VendorRoot, hub: Option<&str>) -> Station {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let key = Arc::new(SigningKey::generate());
    let dir = tmpdir();
    let settings = dir.join("station-settings.json");
    if let Some(hub) = hub {
        save_station_settings(
            &settings,
            &StationSettings {
                hub_origin: Some(hub.to_string()),
            },
        )
        .unwrap();
    }
    let state = AppState::new(backend, None, key.clone())
        .with_vendor_roots(vec![root.verifying_key()])
        .with_station_id("bench-1")
        .with_settings_path(Some(settings))
        .with_instance_key_kept(true);
    let old = signed_licence(root, "lic-old", &key.public_key_id(), 100);
    state
        .install_license(&serde_json::to_string(&old).unwrap())
        .unwrap();
    let base = format!("http://{}", serve_on(router(state.clone())).await);
    Station {
        state,
        base,
        key,
        dir: Some(dir),
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

async fn license_view(base: &str) -> Value {
    let r = http()
        .get(format!("{base}/api/license"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    r.json().await.unwrap()
}

fn root() -> Arc<VendorRoot> {
    Arc::new(VendorRoot::generate())
}

// ---------------------------------------------------------------------------
// The matrix

#[tokio::test]
async fn a_renewed_licence_is_verified_installed_saved_and_reported() {
    let root = root();
    let (origin, hub) = fake_hub(vec![Answer::Licence {
        root: root.clone(),
        id: "lic-new",
        days: 465,
    }])
    .await;
    let st = station(&root, Some(&origin)).await;
    let before = license_view(&st.base).await;
    assert!(before.get("hub_check").is_none(), "{before}");

    let check = check_once(&st.state).await.unwrap();
    assert!(check.ok, "{check:?}");
    assert!(check
        .message
        .starts_with("Installed a new licence from the platform at "));
    assert!(
        check.message.contains("start the station with --license "),
        "{}",
        check.message
    );

    // The hub was asked with the held licence id and this station's key.
    let fetched = hub.lock().unwrap().fetched.clone();
    assert_eq!(
        fetched,
        [json!({
            "instance_public_key": st.key.verifying_key().to_base64(),
            "licence_id": "lic-old",
        })]
    );
    assert_eq!(hub.lock().unwrap().bad_requests, 0);

    let after = license_view(&st.base).await;
    assert_eq!(st.state.license().license_id(), Some("lic-new"));
    assert_ne!(after["not_after"], before["not_after"]);
    assert_eq!(after["hub_check"]["ok"], true);
    assert_eq!(after["hub_check"]["message"], check.message.as_str());
    assert!(after["hub_check"]["checked_at"].is_string());
    // Saved where an enrollment saves it.
    let saved = st.dir.as_ref().unwrap().join(ENROLLED_LICENCE_FILE);
    let saved = wipe_license::parse_license(&std::fs::read_to_string(saved).unwrap()).unwrap();
    assert_eq!(saved.body.license_id, "lic-new");
}

#[tokio::test]
async fn an_up_to_date_station_changes_nothing() {
    let root = root();
    let (origin, _hub) = fake_hub(vec![]).await;
    let st = station(&root, Some(&origin)).await;
    let check = check_once(&st.state).await.unwrap();
    assert!(check.ok);
    assert!(check.message.starts_with("Up to date"), "{}", check.message);
    assert_eq!(st.state.license().license_id(), Some("lic-old"));
    assert!(!st
        .dir
        .as_ref()
        .unwrap()
        .join(ENROLLED_LICENCE_FILE)
        .exists());
    assert_eq!(license_view(&st.base).await["hub_check"]["ok"], true);
}

#[tokio::test]
async fn an_unregistered_station_reports_it() {
    let root = root();
    let (origin, _hub) = fake_hub(vec![Answer::Raw(
        404,
        json!({"error": "this station is not registered with the platform"}),
    )])
    .await;
    let st = station(&root, Some(&origin)).await;
    let check = check_once(&st.state).await.unwrap();
    assert!(!check.ok);
    assert!(
        check.message.contains("not registered with the platform"),
        "{}",
        check.message
    );
    assert!(check.message.ends_with(STILL_APPLIES), "{}", check.message);
    assert_eq!(st.state.license().license_id(), Some("lic-old"));
}

#[tokio::test]
async fn a_licence_from_another_root_is_not_installed() {
    let root = root();
    let (origin, _hub) = fake_hub(vec![Answer::Licence {
        root: Arc::new(VendorRoot::generate()),
        id: "lic-rogue",
        days: 465,
    }])
    .await;
    let st = station(&root, Some(&origin)).await;
    let check = check_once(&st.state).await.unwrap();
    assert!(!check.ok);
    assert!(
        check.message.contains("does not trust"),
        "{}",
        check.message
    );
    assert!(check.message.ends_with(STILL_APPLIES));
    assert_eq!(st.state.license().license_id(), Some("lic-old"));
    assert!(!st
        .dir
        .as_ref()
        .unwrap()
        .join(ENROLLED_LICENCE_FILE)
        .exists());
}

#[tokio::test]
async fn a_licence_for_another_stations_key_is_not_installed() {
    let root = root();
    let other = SigningKey::generate().public_key_id();
    let (origin, _hub) = fake_hub(vec![Answer::Raw(
        200,
        json!({ "licence": signed_licence(&root, "lic-other", &other, 465) }),
    )])
    .await;
    let st = station(&root, Some(&origin)).await;
    let check = check_once(&st.state).await.unwrap();
    assert!(!check.ok);
    assert!(
        check.message.contains("for another station"),
        "{}",
        check.message
    );
    assert!(check.message.ends_with(STILL_APPLIES));
    assert_eq!(st.state.license().license_id(), Some("lic-old"));
    assert!(!st
        .dir
        .as_ref()
        .unwrap()
        .join(ENROLLED_LICENCE_FILE)
        .exists());
}

#[tokio::test]
async fn a_licence_ending_no_later_than_the_installed_one_is_not_installed() {
    let root = root();
    let (origin, _hub) = fake_hub(vec![Answer::Licence {
        root: root.clone(),
        id: "lic-shorter",
        days: 465,
    }])
    .await;
    let st = station(&root, Some(&origin)).await;
    // An operator installed a longer licence (say, a vendor-issued one).
    let long = signed_licence(&root, "lic-long", &st.key.public_key_id(), 1000);
    st.state
        .install_license(&serde_json::to_string(&long).unwrap())
        .unwrap();

    let check = check_once(&st.state).await.unwrap();
    assert!(check.ok, "{check:?}");
    assert!(
        check
            .message
            .contains("offered no licence newer than this station's"),
        "{}",
        check.message
    );
    assert!(
        check.message.contains("nothing was installed"),
        "{}",
        check.message
    );
    assert_eq!(st.state.license().license_id(), Some("lic-long"));
    assert!(!st
        .dir
        .as_ref()
        .unwrap()
        .join(ENROLLED_LICENCE_FILE)
        .exists());
}

#[tokio::test]
async fn a_station_trusting_no_vendor_root_does_not_ask_the_hub() {
    let (origin, hub) = fake_hub(vec![]).await;
    let dir = tmpdir();
    let settings = dir.join("station-settings.json");
    save_station_settings(
        &settings,
        &StationSettings {
            hub_origin: Some(origin.clone()),
        },
    )
    .unwrap();
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let state = AppState::new(backend, None, Arc::new(SigningKey::generate()))
        .with_settings_path(Some(settings));
    let check = check_once(&state).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!check.ok);
    assert!(
        check
            .message
            .starts_with("This station trusts no vendor root"),
        "{}",
        check.message
    );
    assert!(check.message.ends_with(STILL_APPLIES));
    let script = hub.lock().unwrap();
    assert!(script.issued.is_empty());
    assert!(script.fetched.is_empty());
    assert_eq!(script.bad_requests, 0);
}

#[tokio::test]
async fn an_unwritable_settings_dir_installs_but_says_it_was_not_saved() {
    use std::os::unix::fs::PermissionsExt;
    let root = root();
    let (origin, _hub) = fake_hub(vec![Answer::Licence {
        root: root.clone(),
        id: "lic-new",
        days: 465,
    }])
    .await;
    let st = station(&root, Some(&origin)).await;
    let dir = st.dir.clone().unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(dir.join("probe"), b"x").is_ok() {
        // Running as root: permissions do not bind, so the case cannot be made.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let check = check_once(&st.state).await.unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(check.ok, "{check:?}");
    assert!(
        check.message.contains("could not be saved"),
        "{}",
        check.message
    );
    assert_eq!(st.state.license().license_id(), Some("lic-new"));
    assert!(!dir.join(ENROLLED_LICENCE_FILE).exists());
}

#[tokio::test]
async fn a_refusal_names_the_hubs_reason() {
    let root = root();
    let (origin, _hub) = fake_hub(vec![Answer::Raw(
        409,
        json!({"error": "registered in more than one organization"}),
    )])
    .await;
    let st = station(&root, Some(&origin)).await;
    let check = check_once(&st.state).await.unwrap();
    assert!(!check.ok);
    assert!(
        check
            .message
            .contains("HTTP 409: registered in more than one organization"),
        "{}",
        check.message
    );
}

#[tokio::test]
async fn an_unreachable_hub_is_a_failed_check_and_nothing_else() {
    let root = root();
    let st = station(&root, Some(&closed_port().await)).await;
    let check = check_once(&st.state).await.unwrap();
    assert!(!check.ok);
    assert!(
        check.message.starts_with("Could not reach the platform"),
        "{}",
        check.message
    );
    assert!(check.message.ends_with(STILL_APPLIES));
    let view = license_view(&st.base).await;
    assert_eq!(view["hub_check"]["ok"], false);
    assert_eq!(view["status"]["status"], "valid");
}

#[tokio::test]
async fn a_job_starts_and_finishes_while_a_check_hangs() {
    let root = root();
    let st = station(&root, Some(&hanging_hub().await)).await;
    let state = st.state.clone();
    let check = tokio::spawn(async move { check_once_with(&state, Duration::from_secs(3)).await });
    // Let the check reach the hub and wait there.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!check.is_finished());

    let client = http();
    let base = &st.base;
    let t0 = Instant::now();
    let created: Value = client
        .post(format!("{base}/api/jobs"))
        .json(&json!({
            "device_id": "dev-nvme-0",
            "classification": "high",
            "intent": "reuse",
            "operator": {"id": "op-1", "display_name": "Alice", "email": "alice@example.com"},
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let job_id = created["job_id"].as_str().unwrap().to_string();
    let started = client
        .post(format!("{base}/api/jobs/{job_id}/start"))
        .send()
        .await
        .unwrap();
    assert!(started.status().is_success(), "{}", started.status());
    assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
    let mut erased = false;
    for _ in 0..400 {
        let job: Value = client
            .get(format!("{base}/api/jobs/{job_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if job["state"]["state"] == "erased" {
            erased = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(erased, "the job did not finish");

    // Then the check gives up, and says the licence still applies.
    let check = check.await.unwrap().unwrap();
    assert!(!check.ok);
    assert!(
        check.message.starts_with("Could not reach the platform"),
        "{}",
        check.message
    );
    let view = license_view(base).await;
    assert_eq!(view["hub_check"]["ok"], false);
    assert!(view["hub_check"]["message"]
        .as_str()
        .unwrap()
        .ends_with(STILL_APPLIES));
}

#[tokio::test]
async fn no_hub_address_means_no_check() {
    let root = root();
    let st = station(&root, None).await;
    assert!(check_once(&st.state).await.is_none());
    assert!(license_view(&st.base).await.get("hub_check").is_none());

    // A station that keeps nothing on disk runs no loop at all.
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let memory = AppState::new(backend, None, Arc::new(SigningKey::generate()));
    assert!(check_once(&memory).await.is_none());
    assert!(!spawn(&memory));
}

// ---------------------------------------------------------------------------
// The loop

/// Records each sleep; after `limit` sleeps, never wakes.
struct FakeClock {
    sleeps: Arc<Mutex<Vec<Duration>>>,
    limit: usize,
}

#[async_trait::async_trait]
impl Clock for FakeClock {
    fn now(&self) -> Duration {
        self.sleeps.lock().unwrap().iter().sum()
    }
    async fn sleep(&mut self, d: Duration) {
        let n = {
            let mut s = self.sleeps.lock().unwrap();
            s.push(d);
            s.len()
        };
        if n >= self.limit {
            std::future::pending::<()>().await;
        }
    }
}

#[tokio::test]
async fn the_loop_checks_at_start_then_every_24_hours() {
    let root = root();
    let (origin, hub) = fake_hub(vec![]).await;
    let st = station(&root, Some(&origin)).await;
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let mut clock = FakeClock {
        sleeps: sleeps.clone(),
        limit: 3,
    };
    let state = st.state.clone();
    let task = tokio::spawn(async move { run_loop(state, &mut clock).await });
    for _ in 0..200 {
        if sleeps.lock().unwrap().len() == 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(*sleeps.lock().unwrap(), vec![CHECK_INTERVAL; 3]);
    assert_eq!(CHECK_INTERVAL, Duration::from_secs(86_400));
    // A check before each sleep: at start, then after each 24 hours.
    assert_eq!(hub.lock().unwrap().fetched.len(), 3);
    task.abort();
}

#[tokio::test]
async fn a_station_built_with_a_saved_hub_checks_at_start_up() {
    let root = root();
    let (origin, hub) = fake_hub(vec![Answer::Licence {
        root: root.clone(),
        id: "lic-start",
        days: 465,
    }])
    .await;
    let dir = tmpdir();
    save_station_settings(
        &dir.join("station-settings.json"),
        &StationSettings {
            hub_origin: Some(origin.clone()),
        },
    )
    .unwrap();
    let mut lines = Vec::new();
    let state = build_station(
        StationConfig {
            fleet: Fleet::Off,
            key_path: Some(dir.join("station.key")),
            static_dir: Some(dir.join("no-ui")),
            persistence: Persistence::Detect {
                control_plane_url: None,
                ephemeral_config: false,
                config_dir: Some(dir.clone()),
            },
            vendor_roots: vec![root.verifying_key()],
            ..StationConfig::new("127.0.0.1:0".parse().unwrap())
        },
        &mut |l| lines.push(l),
    )
    .unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with(&format!("hub: {origin} (from "))
                && l.contains("checked for a renewed licence at start-up and every 24 hours")),
        "{lines:#?}"
    );
    let mut check = None;
    for _ in 0..200 {
        check = state.licence_fetch.last();
        if check.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let check = check.expect("no check within seconds of start-up");
    assert!(check.ok, "{check:?}");
    assert_eq!(state.license().license_id(), Some("lic-start"));
    // The station held no licence, so it asked with none.
    assert_eq!(hub.lock().unwrap().fetched[0]["licence_id"], Value::Null);
    let _ = std::fs::remove_dir_all(&dir);
}
