//! The station's credit reserve and automatic top-up (controlled-licensing
//! story 4, ADR-0013 §3): every station row of the story's I/O matrix over
//! HTTP. A `Pool` licence draws on credit grants the station holds: from a
//! licence file (`POST /api/license`, `--license`) or a top-up from a fake
//! hub that verifies every request as the hub does
//! (`wipe_license::station_request::verify` over a challenge it issued),
//! reads and answers in the wire types the real hub uses
//! (`wipe_license::station_credits`), and answers from a script.
//!
//! "Real" is a backend reporting `is_simulated() == false` (the mock
//! underneath), so the start gate refuses.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing, Json, Router,
};
use serde_json::{json, Value};
use wipe_cert::SigningKey;
use wipe_engine_mock::{MockBackend, MockTiming};
use wipe_license::station_credits::{TopUpAnswer, TopUpRequest, STATION_CREDITS_PATH};
use wipe_license::station_request::{
    self, SignedStationRequest, HEADER_CHALLENGE, HEADER_KEY_ID, HEADER_SIGNATURE,
};
use wipe_license::{CreditGrant, LicenseFile, Quota, VendorRoot};
use wipe_server::bootstrap::{build_station, Fleet, Persistence, StationConfig};
use wipe_server::credit_topup::top_up_once;
use wipe_server::licence_fetch::HubCheck;
use wipe_server::store::{save_station_settings, StationSettings};
use wipe_server::{router, AppState};

const ORG: &str = "org-1";
const NO_CREDITS: &str = "This station has no credits left. Connect it to the platform so it \
                          can top up, or install a licence file carrying credits, to start new \
                          jobs.";

// ---------------------------------------------------------------------------
// Documents

fn pool_entitlements(customer_id: &str) -> wipe_license::Entitlements {
    let now = time::OffsetDateTime::now_utc();
    wipe_license::Entitlements {
        customer_id: customer_id.into(),
        customer_name: "Acme Recycling".into(),
        quota: Quota::Pool,
        scope: wipe_license::Scope::Site {
            site_id: "s".into(),
        },
        not_before: now - time::Duration::days(1),
        not_after: now + time::Duration::days(100),
        features: vec![],
        allowed_methods: wipe_license::AllowedMethods::All,
        machine_binding: None,
        paid_through: None,
    }
}

fn grant(root: &VendorRoot, id: &str, key_id: &str, credits: u64) -> CreditGrant {
    root.issue_credit_grant(id, ORG, key_id, credits, time::OffsetDateTime::now_utc())
        .unwrap()
}

/// A licence file: `Pool` licence `licence_id` for `key_id` from `root`,
/// with `grant` beside it.
fn licence_file(
    root: &VendorRoot,
    licence_id: &str,
    key_id: &str,
    grant: Option<CreditGrant>,
) -> String {
    let license = root
        .issue(
            licence_id,
            key_id,
            pool_entitlements(ORG),
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    serde_json::to_string(&LicenseFile {
        license,
        credit_grant: grant,
    })
    .unwrap()
}

// ---------------------------------------------------------------------------
// Fake hub

/// What the hub answers to a valid top-up.
#[derive(Clone)]
enum Answer {
    /// 200 with a new grant of `credits` for the requesting key.
    Grant { credits: u64, available: u64 },
    /// 200 with a new grant of `credits` from another organization's pool.
    GrantFor { organization: String, credits: u64 },
    /// 200 with this exact grant (a resend).
    Resend(CreditGrant),
    /// 200, no grant.
    Nothing { available: u64 },
    /// 503.
    Fail,
}

struct Script {
    root: Arc<VendorRoot>,
    answers: VecDeque<Answer>,
    /// The offering's terms every answer carries: `(threshold, target)`.
    terms: (u64, u64),
    issued: Vec<String>,
    /// Every verified request body, as the hub's own type parses it.
    reports: Vec<TopUpRequest>,
    bad_requests: usize,
    grants: usize,
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

/// Whether `body` is signed by the key it names over a challenge this hub
/// issued (spending it) for `path`, as the hub checks.
fn verified(s: &mut Script, origin: &str, path: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let req: Value = serde_json::from_slice(body).unwrap_or_default();
    let signed = SignedStationRequest {
        instance_key_id: header(headers, HEADER_KEY_ID),
        challenge: header(headers, HEADER_CHALLENGE),
        signature_b64: header(headers, HEADER_SIGNATURE),
    };
    let verified = req["instance_public_key"]
        .as_str()
        .and_then(|k| wipe_license::enrollment::decode_public_key(k).ok())
        .is_some_and(|key| {
            station_request::verify(&key, &signed, origin, "POST", path, body).is_ok()
        });
    let fresh = s
        .issued
        .iter()
        .position(|c| *c == signed.challenge)
        .map(|i| s.issued.remove(i))
        .is_some();
    verified && fresh
}

fn refused(s: &mut Script) -> Response {
    s.bad_requests += 1;
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "bad request"})),
    )
        .into_response()
}

async fn credits(
    State((hub, origin)): State<(Hub, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut s = hub.lock().unwrap();
    if !verified(&mut s, &origin, STATION_CREDITS_PATH, &headers, &body) {
        return refused(&mut s);
    }
    let Ok(req) = serde_json::from_slice::<TopUpRequest>(&body) else {
        return refused(&mut s);
    };
    let key_id = header(&headers, HEADER_KEY_ID);
    s.reports.push(req);
    let (grant, available) = match s.answers.pop_front() {
        None => (None, 0),
        Some(Answer::Fail) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "down for maintenance"})),
            )
                .into_response()
        }
        Some(Answer::Nothing { available }) => (None, available),
        Some(Answer::Resend(g)) => (Some(g), 0),
        Some(Answer::Grant { credits, available }) => {
            s.grants += 1;
            let id = format!("topup-{}", s.grants);
            (Some(grant(&s.root, &id, &key_id, credits)), available)
        }
        Some(Answer::GrantFor {
            organization,
            credits,
        }) => {
            s.grants += 1;
            let id = format!("topup-{}", s.grants);
            let g = s
                .root
                .issue_credit_grant(
                    id,
                    organization,
                    key_id,
                    credits,
                    time::OffsetDateTime::now_utc(),
                )
                .unwrap();
            (Some(g), 100)
        }
    };
    let (threshold, target_reserve) = s.terms;
    Json(TopUpAnswer {
        credit_grant: grant,
        threshold,
        target_reserve,
        available,
    })
    .into_response()
}

/// The licence fetch: nothing newer (204), once the request verifies.
async fn licence(
    State((hub, origin)): State<(Hub, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut s = hub.lock().unwrap();
    if !verified(&mut s, &origin, "/api/station/licence", &headers, &body) {
        return refused(&mut s);
    }
    StatusCode::NO_CONTENT.into_response()
}

/// A fake hub signing with `root`, answering `answers` with the default
/// terms (threshold 10, target 50).
async fn fake_hub(root: Arc<VendorRoot>, answers: Vec<Answer>) -> (String, Hub) {
    fake_hub_with_terms(root, answers, (10, 50)).await
}

/// As [`fake_hub`], answering with the offering terms `(threshold, target)`.
async fn fake_hub_with_terms(
    root: Arc<VendorRoot>,
    answers: Vec<Answer>,
    terms: (u64, u64),
) -> (String, Hub) {
    let hub: Hub = Arc::new(Mutex::new(Script {
        root,
        answers: answers.into(),
        terms,
        issued: Vec::new(),
        reports: Vec::new(),
        bad_requests: 0,
        grants: 0,
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/api/station/challenge", routing::get(challenge))
        .with_state(hub.clone())
        .merge(
            Router::new()
                .route(STATION_CREDITS_PATH, routing::post(credits))
                .route("/api/station/licence", routing::post(licence))
                .with_state((hub.clone(), origin.clone())),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (origin, hub)
}

fn reports(hub: &Hub) -> Vec<TopUpRequest> {
    hub.lock().unwrap().reports.clone()
}

/// Wait until the station has finished a top-up and none is running: its
/// outcome.
async fn settled(state: &AppState) -> HubCheck {
    for _ in 0..400 {
        if !state.credit_topup.in_flight() {
            if let Some(check) = state.credit_topup.last() {
                return check;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the station never finished a top-up");
}

/// Wait until the hub has answered `n` top-ups and the station has finished
/// with the last: its outcome.
async fn top_ups_done(state: &AppState, hub: &Hub, n: usize) -> HubCheck {
    for _ in 0..400 {
        if reports(hub).len() >= n && !state.credit_topup.in_flight() {
            if let Some(check) = state.credit_topup.last() {
                return check;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "the station never finished {n} top-ups: {} reports, {} refused, last {:?}",
        reports(hub).len(),
        hub.lock().unwrap().bad_requests,
        state.credit_topup.last()
    );
}

/// An address nothing listens on.
async fn closed_port() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    format!("http://{}", l.local_addr().unwrap())
}

// ---------------------------------------------------------------------------
// Station

/// A backend that touches "real" hardware: the mock underneath, reporting
/// `is_simulated() == false`.
struct RealBackend(Arc<MockBackend>);

#[async_trait::async_trait]
impl wipe_engine::DeviceBackend for RealBackend {
    async fn enumerate(&self) -> wipe_common::WipeResult<Vec<wipe_common::Device>> {
        self.0.enumerate().await
    }
    async fn capabilities(
        &self,
        id: &wipe_common::DeviceId,
    ) -> wipe_common::WipeResult<wipe_common::Capabilities> {
        self.0.capabilities(id).await
    }
    async fn unfreeze(&self, id: &wipe_common::DeviceId) -> wipe_common::WipeResult<()> {
        self.0.unfreeze(id).await
    }
    async fn issue(
        &self,
        id: &wipe_common::DeviceId,
        method: &wipe_common::Method,
    ) -> wipe_common::WipeResult<wipe_engine::BackendHandle> {
        self.0.issue(id, method).await
    }
    async fn poll(
        &self,
        handle: &wipe_engine::BackendHandle,
    ) -> wipe_common::WipeResult<wipe_engine::BackendProgress> {
        self.0.poll(handle).await
    }
    async fn cancel(&self, handle: &wipe_engine::BackendHandle) -> wipe_common::WipeResult<()> {
        self.0.cancel(handle).await
    }
    async fn verify(
        &self,
        id: &wipe_common::DeviceId,
        method: wipe_common::VerificationMethod,
        samples: u32,
    ) -> wipe_common::WipeResult<wipe_common::VerificationReport> {
        self.0.verify(id, method, samples).await
    }
}

struct Station {
    state: AppState,
    base: String,
    key: Arc<SigningKey>,
    root: Arc<VendorRoot>,
    dir: PathBuf,
}

impl Drop for Station {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wipestation-credit-reserve-{tag}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save_hub(dir: &Path, hub: &str) {
    save_station_settings(
        &dir.join("station-settings.json"),
        &StationSettings {
            hub_origin: Some(hub.to_string()),
        },
    )
    .unwrap();
}

/// A served, unlicensed station on a real backend trusting `root`, its
/// settings in a temp directory naming `hub` when given.
async fn station(root: Arc<VendorRoot>, hub: Option<&str>, timing: MockTiming) -> Station {
    let mock = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        timing,
    ));
    let key = Arc::new(SigningKey::generate());
    let dir = tmpdir("station");
    if let Some(hub) = hub {
        save_hub(&dir, hub);
    }
    let state = AppState::new(Arc::new(RealBackend(mock)), None, key.clone())
        .with_vendor_roots(vec![root.verifying_key()])
        .with_station_id("bench-1")
        .with_settings_path(Some(dir.join("station-settings.json")))
        .with_instance_key_kept(true);
    let base = serve(state.clone()).await;
    Station {
        state,
        base,
        key,
        root,
        dir,
    }
}

async fn serve(state: AppState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    format!("http://{addr}")
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

async fn post(base: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let req = http().post(format!("{base}{path}"));
    let req = match body {
        Some(b) => req.json(&b),
        None => req,
    };
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

async fn get(base: &str, path: &str) -> (u16, Value) {
    let resp = http().get(format!("{base}{path}")).send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

async fn install(base: &str, file: &str) -> (u16, Value) {
    let resp = http()
        .post(format!("{base}/api/license"))
        .header("content-type", "application/json")
        .body(file.to_string())
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

async fn view(base: &str) -> Value {
    let (status, v) = get(base, "/api/license").await;
    assert_eq!(status, 200);
    v
}

async fn remaining(base: &str) -> Value {
    view(base).await["status"]["remaining_erasures"].clone()
}

fn op() -> Value {
    json!({ "id": "op-1", "display_name": "Alice", "email": "alice@example.com" })
}

/// A queued Job erasing `device` for reuse.
async fn create_job(base: &str, device: &str) -> String {
    let (status, v) = post(
        base,
        "/api/jobs",
        Some(json!({
            "device_id": device, "classification": "moderate", "intent": "reuse",
            "operator": op(),
        })),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    v["job_id"].as_str().unwrap().to_string()
}

async fn start(base: &str, job: &str) -> (u16, Value) {
    post(base, &format!("/api/jobs/{job}/start"), None).await
}

/// Record `n` scanned drives for destruction in one group: one credit each.
async fn record_scanned(base: &str, n: usize) -> (u16, Value) {
    let drives: Vec<Value> = (0..n)
        .map(|i| json!({ "serial": format!("S-{}-{i}", uuid::Uuid::new_v4()) }))
        .collect();
    post(
        base,
        "/api/destruction/records",
        Some(json!({
            "method": "shred", "equipment_id": "SHR-2", "location": "Bay 3",
            "operator": op(), "full_name": "Alice Erasure", "title": "Bench lead",
            "attest": true, "notes": null, "classification": "moderate", "drives": drives,
        })),
    )
    .await
}

async fn wait_for_certificate(base: &str, job: &str) -> Value {
    for _ in 0..400 {
        let (status, body) = get(base, &format!("/api/jobs/{job}/certificate")).await;
        if status == 200 {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no certificate for job {job}");
}

/// Wait until `f` holds of the station's lease state.
async fn wait_lease(state: &AppState, what: &str, f: impl Fn(&wipe_license::LeaseState) -> bool) {
    for _ in 0..400 {
        if f(&state.license().lease_state()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "lease never reached: {what}: {:?}",
        state.license().lease_state()
    );
}

fn root() -> Arc<VendorRoot> {
    Arc::new(VendorRoot::generate())
}

// ---------------------------------------------------------------------------
// The matrix

/// Grant in licence file: 25 credits cover exactly 25 starts.
#[tokio::test]
async fn a_grant_in_a_licence_file_covers_exactly_its_credits() {
    let st = station(root(), None, MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 25)));
    let (status, v) = install(&st.base, &file).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["status"]["remaining_erasures"], 25, "{v}");
    assert_eq!(remaining(&st.base).await, 25);

    // 24 scanned drives and one erasure: 25 starts.
    let (status, v) = record_scanned(&st.base, 24).await;
    assert_eq!(status, 200, "{v}");
    let job = create_job(&st.base, "dev-nvme-0").await;
    let (status, v) = start(&st.base, &job).await;
    assert_eq!(status, 200, "{v}");
    let v = view(&st.base).await;
    assert_eq!(v["status"]["status"], "quota_exhausted", "{v}");
    assert_eq!(v["status"]["used"], 25);
    assert_eq!(v["status"]["allowed"], 25);
    assert_eq!(v["new_jobs_refused"], NO_CREDITS);

    // The 26th is refused with the connect/install text.
    let other = create_job(&st.base, "dev-nvme-1").await;
    let (status, v) = start(&st.base, &other).await;
    assert_eq!(status, 409, "{v}");
    assert_eq!(v["error"], NO_CREDITS);
    let (status, v) = record_scanned(&st.base, 2).await;
    assert_eq!(status, 409, "{v}");
    assert_eq!(
        v["error"],
        "This start needs 2 credits, and this station holds 0. Connect it to the platform so \
         it can top up, or install a licence file carrying credits, to start it."
    );

    // The erasure is certified: its credit is spent from the grant.
    let cert = wait_for_certificate(&st.base, &job).await;
    assert_eq!(cert["certificate"]["evaluation"], false);
    wait_lease(&st.state, "one credit spent", |l| {
        l.credit_grants[0].spent == 1 && l.pool_credits_reserved == 24
    })
    .await;
    // The decision names the pool.
    let (_, j) = get(&st.base, &format!("/api/jobs/{job}")).await;
    assert_eq!(j["licence"]["credit_pool"], ORG, "{j}");
    assert_eq!(j["licence"]["credits_reserved"], 1);
}

/// Duplicate grant: the same file again changes nothing; foreign grants
/// refuse the whole file.
#[tokio::test]
async fn a_held_grant_is_ignored_and_a_foreign_grant_refuses_the_whole_file() {
    let st = station(root(), None, MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 25)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    let (status, v) = install(&st.base, &file).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["status"]["remaining_erasures"], 25);
    assert_eq!(st.state.license().lease_state().credit_grants.len(), 1);

    // A grant for another key.
    let other = SigningKey::generate().public_key_id();
    let file = licence_file(
        &st.root,
        "lic-2",
        &k,
        Some(grant(&st.root, "g-2", &other, 10)),
    );
    let (status, v) = install(&st.base, &file).await;
    assert_eq!(status, 422, "{v}");
    let text = v["error"].as_str().unwrap();
    assert!(
        text.starts_with(&format!("credit grant was issued to instance key {other}")),
        "{text}"
    );
    // A grant signed by a root this station does not trust.
    let stranger = VendorRoot::generate();
    let file = licence_file(&st.root, "lic-3", &k, Some(grant(&stranger, "g-3", &k, 10)));
    let (status, v) = install(&st.base, &file).await;
    assert_eq!(status, 422, "{v}");
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .starts_with("credit grant did not verify: "),
        "{v}"
    );
    // Nothing was installed: the first licence and grant stand.
    let ctx = st.state.license();
    assert_eq!(ctx.license_id(), Some("lic-1"));
    assert_eq!(ctx.lease_state().credit_grants.len(), 1);
    assert_eq!(remaining(&st.base).await, 25);
}

/// Spend / abort: three scanned drives reserve three; aborting returns them;
/// a certified erasure spends from the oldest grant first.
#[tokio::test]
async fn reservations_are_returned_on_abort_and_spent_oldest_grant_first() {
    let st = station(root(), None, MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 1)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-2", &k, 5)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    assert_eq!(remaining(&st.base).await, 6);

    let (status, v) = record_scanned(&st.base, 3).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(remaining(&st.base).await, 3);
    assert_eq!(st.state.license().lease_state().pool_credits_reserved, 3);
    for id in v["job_ids"].as_array().unwrap() {
        let (status, body) = post(
            &st.base,
            &format!("/api/jobs/{}/abort", id.as_str().unwrap()),
            None,
        )
        .await;
        assert_eq!(status, 200, "{body}");
    }
    wait_lease(&st.state, "reservations returned", |l| {
        l.pool_credits_reserved == 0
    })
    .await;
    assert_eq!(remaining(&st.base).await, 6);

    // Two erasures: the 1-credit grant is spent first, then the 5-credit one.
    for device in ["dev-nvme-0", "dev-nvme-1"] {
        let job = create_job(&st.base, device).await;
        assert_eq!(start(&st.base, &job).await.0, 200);
        wait_for_certificate(&st.base, &job).await;
    }
    wait_lease(&st.state, "two spent", |l| {
        l.credit_grants
            .iter()
            .map(|g| (g.grant_id.as_str(), g.spent))
            .collect::<Vec<_>>()
            == [("g-1", 1), ("g-2", 1)]
    })
    .await;
    assert_eq!(remaining(&st.base).await, 4);
}

/// Licence switch mid-run: a renewal installed while a pool Job runs keeps
/// the reserve, and the Job spends from it.
#[tokio::test]
async fn a_licence_switch_mid_run_keeps_the_reserve() {
    let slow = MockTiming {
        nvme_sanitize_secs: 1.5,
        ata_secure_erase_secs: 1.5,
        block_overwrite_secs: 1.5,
        deterministic: true,
    };
    let st = station(root(), None, slow).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 10)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    let job = create_job(&st.base, "dev-nvme-0").await;
    assert_eq!(start(&st.base, &job).await.0, 200);
    assert_eq!(remaining(&st.base).await, 9);

    // The renewal arrives as a bare licence: no grant, a new id.
    let renewal = licence_file(&st.root, "lic-2", &k, None);
    let (status, v) = install(&st.base, &renewal).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["status"]["remaining_erasures"], 9, "{v}");

    wait_for_certificate(&st.base, &job).await;
    wait_lease(&st.state, "spent under the new licence", |l| {
        l.pool_credits_reserved == 0 && l.credit_grants[0].spent == 1
    })
    .await;
    assert_eq!(st.state.license().license_id(), Some("lic-2"));
    assert_eq!(remaining(&st.base).await, 9);
}

/// Top-up at threshold: 9 held, the hub grants 41. The licence install
/// nudges the first top-up (the station does not know its terms yet).
#[tokio::test]
async fn a_station_below_the_threshold_tops_up_and_reports_it() {
    let r = root();
    let (origin, hub) = fake_hub(
        r.clone(),
        vec![Answer::Grant {
            credits: 41,
            available: 950,
        }],
    )
    .await;
    let st = station(r, Some(&origin), MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 9)));
    assert_eq!(install(&st.base, &file).await.0, 200);

    let check = top_ups_done(&st.state, &hub, 1).await;
    assert!(check.ok, "{check:?}");
    assert!(
        check.message.contains("received 41 credits"),
        "{}",
        check.message
    );
    assert!(
        check.message.contains("has 50 credits available"),
        "{}",
        check.message
    );
    assert_eq!(remaining(&st.base).await, 50);
    assert_eq!(
        reports(&hub),
        [TopUpRequest {
            instance_public_key: st.key.verifying_key().to_base64(),
            licence_id: Some("lic-1".into()),
            balance: 9,
            credits_spent: 0,
            last_grant_id: None,
        }]
    );
    assert_eq!(hub.lock().unwrap().bad_requests, 0);
    let lease = st.state.license().lease_state();
    assert_eq!(lease.last_topup_grant_id.as_deref(), Some("topup-1"));
    assert_eq!(
        lease.credit_terms,
        Some(wipe_license::CreditTerms {
            threshold: 10,
            target: 50
        })
    );
    let v = view(&st.base).await;
    assert_eq!(v["credit_top_up"]["ok"], true, "{v}");
    assert_eq!(v["credit_top_up"]["message"], check.message);

    // Above the threshold now: no request.
    assert!(top_up_once(&st.state).await.is_none());
    assert_eq!(reports(&hub).len(), 1);
}

/// Above threshold: once the station knows its terms, 30 held sends
/// nothing.
#[tokio::test]
async fn a_station_above_the_threshold_sends_nothing() {
    let r = root();
    let (origin, hub) = fake_hub(r.clone(), vec![Answer::Nothing { available: 970 }]).await;
    let st = station(r, Some(&origin), MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 30)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    // Asked once to learn the terms.
    let check = top_ups_done(&st.state, &hub, 1).await;
    assert!(check.ok, "{check:?}");
    assert!(check.message.starts_with("No top-up needed"), "{check:?}");
    assert_eq!(reports(&hub)[0].balance, 30);

    assert!(top_up_once(&st.state).await.is_none());
    assert_eq!(reports(&hub).len(), 1);
    assert!(hub.lock().unwrap().issued.is_empty());
}

/// The hub's own threshold (30) is the one used: 30 available sends
/// nothing, 20 (10 reserved by a running destruction group) is sent.
#[tokio::test]
async fn the_hub_s_threshold_decides_when_a_top_up_is_sent() {
    let r = root();
    let (origin, hub) = fake_hub_with_terms(
        r.clone(),
        vec![
            Answer::Nothing { available: 500 },
            Answer::Nothing { available: 500 },
        ],
        (30, 60),
    )
    .await;
    let st = station(r, Some(&origin), MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 30)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    top_ups_done(&st.state, &hub, 1).await;
    assert_eq!(
        st.state.license().lease_state().credit_terms,
        Some(wipe_license::CreditTerms {
            threshold: 30,
            target: 60
        })
    );
    assert!(top_up_once(&st.state).await.is_none());

    let (status, v) = record_scanned(&st.base, 10).await;
    assert_eq!(status, 200, "{v}");
    top_ups_done(&st.state, &hub, 2).await;
    let sent = reports(&hub);
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[1].balance, sent[1].credits_spent), (20, 0));
}

/// Reserved credits: 12 held and a 12-drive group running leaves nothing a
/// start may reserve, so the station reports 0 and gets the target.
#[tokio::test]
async fn credits_reserved_by_running_jobs_are_not_reported_as_available() {
    let r = root();
    let (origin, hub) = fake_hub(
        r.clone(),
        vec![
            Answer::Nothing { available: 1000 },
            Answer::Grant {
                credits: 50,
                available: 950,
            },
        ],
    )
    .await;
    let st = station(r, Some(&origin), MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 12)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    top_ups_done(&st.state, &hub, 1).await;

    let (status, v) = record_scanned(&st.base, 12).await;
    assert_eq!(status, 200, "{v}");
    let check = top_ups_done(&st.state, &hub, 2).await;
    assert!(check.ok, "{check:?}");
    assert_eq!(reports(&hub)[1].balance, 0);
    assert_eq!(remaining(&st.base).await, 50);
}

/// Empty pool: the hub sends nothing, the status says so, and a start with
/// no credits is refused with the connect/install text.
#[tokio::test]
async fn an_empty_pool_is_reported_and_starts_are_refused_once_credits_are_gone() {
    let r = root();
    let (origin, hub) = fake_hub(r.clone(), vec![Answer::Nothing { available: 0 }]).await;
    let st = station(r, Some(&origin), MockTiming::fast()).await;
    let k = st.key.public_key_id();
    assert_eq!(
        install(&st.base, &licence_file(&st.root, "lic-1", &k, None))
            .await
            .0,
        200
    );
    let check = top_ups_done(&st.state, &hub, 1).await;
    assert!(!check.ok, "{check:?}");
    assert!(
        check.message.contains("credit pool on the platform at")
            && check.message.contains("is empty"),
        "{}",
        check.message
    );
    let v = view(&st.base).await;
    assert_eq!(v["credit_top_up"]["ok"], false);
    assert_eq!(v["new_jobs_refused"], NO_CREDITS);
    let job = create_job(&st.base, "dev-nvme-0").await;
    let (status, v) = start(&st.base, &job).await;
    assert_eq!(status, 409, "{v}");
    assert_eq!(v["error"], NO_CREDITS);
}

/// Unreachable hub: starts succeed until the reserve is empty; the failed
/// top-up is only a status line.
#[tokio::test]
async fn an_unreachable_hub_never_refuses_a_start_while_credits_remain() {
    let origin = closed_port().await;
    let st = station(root(), Some(&origin), MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 5)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    // The install's nudge asks the closed port.
    let check = settled(&st.state).await;
    assert!(!check.ok, "{check:?}");
    assert!(
        check.message.starts_with("Could not reach the platform at"),
        "{}",
        check.message
    );
    assert!(
        check.message.contains("has 5 credits available"),
        "{}",
        check.message
    );

    let (status, v) = record_scanned(&st.base, 4).await;
    assert_eq!(status, 200, "{v}");
    let job = create_job(&st.base, "dev-nvme-0").await;
    assert_eq!(start(&st.base, &job).await.0, 200);
    let other = create_job(&st.base, "dev-nvme-1").await;
    let (status, v) = start(&st.base, &other).await;
    assert_eq!(status, 409, "{v}");
    assert_eq!(v["error"], NO_CREDITS);
    let v = view(&st.base).await;
    assert_eq!(v["credit_top_up"]["ok"], false, "{v}");
}

/// A failed top-up is not retried by the starts that follow: nudges wait
/// out the quiet period.
#[tokio::test]
async fn a_failed_top_up_is_not_retried_by_an_immediate_start() {
    let r = root();
    let (origin, hub) = fake_hub(r.clone(), vec![Answer::Fail]).await;
    let st = station(r, Some(&origin), MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 5)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    let check = top_ups_done(&st.state, &hub, 1).await;
    assert!(!check.ok, "{check:?}");
    assert!(check.message.contains("HTTP 503"), "{}", check.message);

    // A start, and a refusal for lack of credits: neither asks again.
    let (status, v) = record_scanned(&st.base, 5).await;
    assert_eq!(status, 200, "{v}");
    let job = create_job(&st.base, "dev-nvme-0").await;
    assert_eq!(start(&st.base, &job).await.0, 409);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(reports(&hub).len(), 1);
    assert!(hub.lock().unwrap().issued.is_empty());
}

/// A pool start that leaves the station below the threshold nudges a
/// top-up on its own task.
#[tokio::test]
async fn a_pool_start_below_the_threshold_nudges_a_top_up() {
    let r = root();
    let (origin, hub) = fake_hub(
        r.clone(),
        vec![
            Answer::Nothing { available: 100 },
            Answer::Grant {
                credits: 41,
                available: 59,
            },
        ],
    )
    .await;
    let st = station(r, Some(&origin), MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 10)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    top_ups_done(&st.state, &hub, 1).await;
    let job = create_job(&st.base, "dev-nvme-0").await;
    assert_eq!(start(&st.base, &job).await.0, 200);
    wait_lease(&st.state, "topped up by the nudge", |l| {
        l.credit_grants.len() == 2
    })
    .await;
    assert_eq!(reports(&hub)[1].balance, 9);
}

/// A spend nudges a top-up too: here the start's nudge found no hub
/// address yet, and the certificate's spend is what asks.
#[tokio::test]
async fn a_spend_nudges_a_top_up() {
    let slow = MockTiming {
        nvme_sanitize_secs: 1.0,
        ata_secure_erase_secs: 1.0,
        block_overwrite_secs: 1.0,
        deterministic: true,
    };
    let r = root();
    let (origin, hub) = fake_hub(
        r.clone(),
        vec![Answer::Grant {
            credits: 41,
            available: 59,
        }],
    )
    .await;
    let st = station(r, None, slow).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 10)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    // The terms a previous answer taught it.
    st.state.license().record_top_up(
        wipe_license::CreditTerms {
            threshold: 10,
            target: 50,
        },
        None,
    );
    let job = create_job(&st.base, "dev-nvme-0").await;
    assert_eq!(start(&st.base, &job).await.0, 200);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!st.state.credit_topup.in_flight());
    // Connected while the erasure runs.
    save_hub(&st.dir, &origin);
    wait_for_certificate(&st.base, &job).await;
    let check = top_ups_done(&st.state, &hub, 1).await;
    assert!(check.ok, "{check:?}");
    let sent = reports(&hub);
    assert_eq!((sent[0].balance, sent[0].credits_spent), (9, 1));
    assert_eq!(remaining(&st.base).await, 50);
}

/// A start refused for lack of credits nudges a top-up: the station ran
/// dry while unconnected, then is connected and an operator tries a start.
#[tokio::test]
async fn a_start_refused_for_lack_of_credits_nudges_a_top_up() {
    let r = root();
    let (origin, hub) = fake_hub(
        r.clone(),
        vec![Answer::Grant {
            credits: 25,
            available: 975,
        }],
    )
    .await;
    let st = station(r, None, MockTiming::fast()).await;
    let k = st.key.public_key_id();
    assert_eq!(
        install(&st.base, &licence_file(&st.root, "lic-1", &k, None))
            .await
            .0,
        200
    );
    st.state.license().record_top_up(
        wipe_license::CreditTerms {
            threshold: 10,
            target: 50,
        },
        None,
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!st.state.credit_topup.in_flight());
    save_hub(&st.dir, &origin);

    let job = create_job(&st.base, "dev-nvme-0").await;
    let (status, v) = start(&st.base, &job).await;
    assert_eq!(status, 409, "{v}");
    assert_eq!(v["error"], NO_CREDITS);
    let check = top_ups_done(&st.state, &hub, 1).await;
    assert!(check.ok, "{check:?}");
    assert_eq!(reports(&hub)[0].balance, 0);
    assert_eq!(remaining(&st.base).await, 25);
}

/// A successful top-up that still leaves the station below the threshold
/// (the pool had only 3 left) quiets nudges: the next start asks nothing.
#[tokio::test]
async fn a_top_up_that_leaves_the_station_low_quiets_the_next_nudges() {
    let r = root();
    let (origin, hub) = fake_hub(
        r.clone(),
        vec![Answer::Grant {
            credits: 3,
            available: 0,
        }],
    )
    .await;
    let st = station(r, Some(&origin), MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 2)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    let check = top_ups_done(&st.state, &hub, 1).await;
    assert!(check.ok, "{check:?}");
    assert_eq!(remaining(&st.base).await, 5);

    let job = create_job(&st.base, "dev-nvme-0").await;
    assert_eq!(start(&st.base, &job).await.0, 200);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(reports(&hub).len(), 1);
    assert!(hub.lock().unwrap().issued.is_empty());
}

/// A lost answer resent by the hub: the grant already held is not counted
/// twice, and its id is remembered so the hub stops resending it.
#[tokio::test]
async fn a_resent_grant_already_held_is_not_counted_twice() {
    let r = root();
    let st_key = Arc::new(SigningKey::generate());
    let held = grant(&r, "topup-x", &st_key.public_key_id(), 4);
    let (origin, hub) = fake_hub(r.clone(), vec![Answer::Resend(held.clone())]).await;
    let mut st = station(r, Some(&origin), MockTiming::fast()).await;
    // Rebuild the state around the key the grant names.
    let mock = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    st.state = AppState::new(Arc::new(RealBackend(mock)), None, st_key.clone())
        .with_vendor_roots(vec![st.root.verifying_key()])
        .with_settings_path(Some(st.dir.join("station-settings.json")));
    st.base = serve(st.state.clone()).await;
    let k = st_key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(held));
    assert_eq!(install(&st.base, &file).await.0, 200);

    let check = top_ups_done(&st.state, &hub, 1).await;
    assert!(check.ok, "{check:?}");
    assert!(check.message.contains("already holds"), "{}", check.message);
    assert_eq!(reports(&hub)[0].last_grant_id, None);
    let lease = st.state.license().lease_state();
    assert_eq!(lease.credit_grants.len(), 1);
    assert_eq!(lease.last_topup_grant_id.as_deref(), Some("topup-x"));
    assert_eq!(remaining(&st.base).await, 4);
}

/// A grant from another organization's pool is not added, and the last
/// top-up grant id is not moved.
#[tokio::test]
async fn a_grant_for_another_organization_is_not_added() {
    let r = root();
    let (origin, hub) = fake_hub(
        r.clone(),
        vec![Answer::GrantFor {
            organization: "org-2".into(),
            credits: 50,
        }],
    )
    .await;
    let st = station(r, Some(&origin), MockTiming::fast()).await;
    let k = st.key.public_key_id();
    let file = licence_file(&st.root, "lic-1", &k, Some(grant(&st.root, "g-1", &k, 5)));
    assert_eq!(install(&st.base, &file).await.0, 200);
    let check = top_ups_done(&st.state, &hub, 1).await;
    assert!(!check.ok, "{check:?}");
    assert!(
        check.message.contains("for another organization (org-2)"),
        "{}",
        check.message
    );
    let lease = st.state.license().lease_state();
    assert_eq!(lease.credit_grants.len(), 1);
    assert_eq!(lease.last_topup_grant_id, None);
    // The terms are kept all the same.
    assert!(lease.credit_terms.is_some());
    assert_eq!(remaining(&st.base).await, 5);
    assert_eq!(view(&st.base).await["credit_top_up"]["ok"], false);
}

/// AC 2: a connected station with no credits tops up in the licence-fetch
/// loop's first round (no direct top-up call), then covers exactly the
/// credits granted.
#[tokio::test]
async fn the_licence_fetch_loop_tops_up_a_station_with_no_credits() {
    let r = root();
    let (origin, hub) = fake_hub(
        r.clone(),
        vec![Answer::Grant {
            credits: 25,
            available: 0,
        }],
    )
    .await;
    let mock = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let key = Arc::new(SigningKey::generate());
    let dir = tmpdir("loop");
    save_hub(&dir, &origin);
    let license = r
        .issue(
            "lic-1",
            key.public_key_id(),
            pool_entitlements(ORG),
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    let ctx = wipe_server::LicenseContext::open(
        "bench-1",
        Some(wipe_license::AttestationChain::new(license)),
        Arc::new(wipe_server::store::EphemeralLeaseStore::new("test".into())),
        time::OffsetDateTime::now_utc(),
    );
    let state = AppState::new(Arc::new(RealBackend(mock)), None, key.clone())
        .with_license(Arc::new(ctx))
        .with_vendor_roots(vec![r.verifying_key()])
        .with_station_id("bench-1")
        .with_settings_path(Some(dir.join("station-settings.json")))
        .with_instance_key_kept(true);
    let st = Station {
        base: serve(state.clone()).await,
        state,
        key,
        root: r,
        dir,
    };
    assert_eq!(view(&st.base).await["new_jobs_refused"], NO_CREDITS);
    assert!(wipe_server::licence_fetch::spawn(&st.state));

    let check = top_ups_done(&st.state, &hub, 1).await;
    assert!(check.ok, "{check:?}");
    assert_eq!(remaining(&st.base).await, 25);
    assert_eq!(hub.lock().unwrap().bad_requests, 0);

    let (status, v) = record_scanned(&st.base, 24).await;
    assert_eq!(status, 200, "{v}");
    let job = create_job(&st.base, "dev-nvme-0").await;
    assert_eq!(start(&st.base, &job).await.0, 200);
    let other = create_job(&st.base, "dev-nvme-1").await;
    let (status, v) = start(&st.base, &other).await;
    assert_eq!(status, 409, "{v}");
    assert_eq!(v["error"], NO_CREDITS);
}

// ---------------------------------------------------------------------------
// Restart

fn file_config(
    dir: &Path,
    license: Option<PathBuf>,
    roots: Vec<wipe_cert::VerifyingKey>,
) -> StationConfig {
    StationConfig {
        fleet: Fleet::Off,
        key_path: Some(dir.join("station.key")),
        persistence: Persistence::Detect {
            control_plane_url: None,
            ephemeral_config: false,
            config_dir: Some(dir.to_path_buf()),
        },
        license,
        vendor_roots: roots,
        ..StationConfig::new("127.0.0.1:0".parse().unwrap())
    }
}

/// Restart on a file lease store with `--license` carrying a grant: the
/// same balance, reservations returned, the grant never counted twice; a
/// file whose grant fails is a start-up error.
#[tokio::test]
async fn a_restart_keeps_the_reserve_and_returns_reservations() {
    let dir = tmpdir("restart");
    let root = VendorRoot::generate();
    // A first start creates the key.
    let first = build_station(file_config(&dir, None, vec![]), &mut |_| {}).unwrap();
    let k = first.signing_key.public_key_id();
    drop(first);
    let path = dir.join("licence.json");
    std::fs::write(
        &path,
        licence_file(&root, "lic-1", &k, Some(grant(&root, "g-1", &k, 20))),
    )
    .unwrap();
    let cfg = || file_config(&dir, Some(path.clone()), vec![root.verifying_key()]);

    let mut lines = Vec::new();
    let st = build_station(cfg(), &mut |l| lines.push(l)).unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("credit grant g-1: 20 credits added")),
        "{lines:?}"
    );
    let ctx = st.license();
    let req = wipe_server::StartRequest {
        method: None,
        drives: 3,
        simulated: false,
    };
    let decision = ctx
        .decide_start(&req, time::OffsetDateTime::now_utc())
        .unwrap();
    assert_eq!(decision.credit_pool.as_deref(), Some(ORG));
    ctx.spend(&wipe_common::JobLicence {
        credits_reserved: 2,
        ..decision.clone()
    });
    ctx.decide_start(&req, time::OffsetDateTime::now_utc())
        .unwrap();
    let before = ctx.lease_state();
    assert_eq!(before.pool_credits_reserved, 4);
    assert_eq!(before.pool_balance(ORG), 18);
    drop(ctx);
    drop(st);

    let mut lines = Vec::new();
    let st = build_station(cfg(), &mut |l| lines.push(l)).unwrap();
    assert!(
        !lines.iter().any(|l| l.contains("credits added")),
        "the held grant was added again: {lines:?}"
    );
    let after = st.license().lease_state();
    assert_eq!(after.credit_grants, before.credit_grants);
    assert_eq!(after.pool_credits_reserved, 0);
    assert_eq!(after.pool_available(ORG), 18);
    drop(st);

    // A file whose grant names another key refuses to start.
    let other = SigningKey::generate().public_key_id();
    std::fs::write(
        &path,
        licence_file(&root, "lic-1", &k, Some(grant(&root, "g-2", &other, 5))),
    )
    .unwrap();
    let err = build_station(cfg(), &mut |_| {}).err().unwrap().to_string();
    assert!(
        err.contains("credit grant was issued to instance key"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
