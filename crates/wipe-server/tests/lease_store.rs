//! Lease state persistence (ADR-0005 §3a through the ADR-0003 tiers).
//!
//! A restart must neither reset the clock watermark nor refill a `Count`
//! quota. A lease file that cannot be read or saved refuses starts on a
//! licence that counts credits (the reservation cannot be kept), and never
//! makes a start under an `Unlimited` licence uncovered (ADR-0013).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use wipe_common::JobLicence;
use wipe_common::{Classification, DeviceId, Intent, JobSpec, OperatorRef};
use wipe_license::{AttestationChain, LeaseState, LeaseStatus, Quota};
use wipe_server::store::{
    detect_lease_store, lease_state_path, EphemeralLeaseStore, LeaseStore, LocalFileLeaseStore,
    StoreConfig, StoreTier,
};
use wipe_server::{AppState, LicenseContext, StartRefusal, StartRequest};

const RESET_WORDING: &str = "quota count and clock-rollback protection reset on reboot";

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wipestation-lease-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cfg_in(dir: &Path) -> StoreConfig {
    StoreConfig {
        explicit_path: Some(dir.join("bay-topology.json")),
        control_plane_url: None,
        station_id: "station-1".into(),
    }
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn chain(license_id: &str, quota: Quota, instance_key_id: &str) -> AttestationChain {
    let root = wipe_license::VendorRoot::generate();
    let ent = wipe_license::Entitlements {
        customer_id: "c".into(),
        customer_name: "C".into(),
        quota,
        scope: wipe_license::Scope::Site {
            site_id: "s".into(),
        },
        not_before: now() - Duration::days(30),
        not_after: now() + Duration::days(30),
        features: vec![],
        allowed_methods: wipe_license::AllowedMethods::All,
        machine_binding: None,
    };
    AttestationChain::new(root.issue(license_id, instance_key_id, ent, now()).unwrap())
}

fn unlimited(license_id: &str) -> AttestationChain {
    chain(license_id, Quota::Unlimited, "ed25519:test")
}

fn counted(license_id: &str, erasures: u64) -> AttestationChain {
    chain(license_id, Quota::Count { erasures }, "ed25519:test")
}

/// A one-drive start on a real backend.
fn decide(ctx: &LicenseContext, at: OffsetDateTime) -> Result<JobLicence, StartRefusal> {
    ctx.decide_start(
        &StartRequest {
            method: None,
            drives: 1,
            simulated: false,
        },
        at,
    )
}

fn stored(license_id: Option<&str>, used: u64, watermark: OffsetDateTime) -> LeaseState {
    let mut s = LeaseState::new(watermark);
    s.erasures_used = used;
    s.license_id = license_id.map(str::to_string);
    s
}

#[cfg(unix)]
fn set_mode(dir: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mut p = std::fs::metadata(dir).unwrap().permissions();
    p.set_mode(mode);
    std::fs::set_permissions(dir, p).unwrap();
}

/// Make `dir` read-only (0o500) and report whether that really stops a
/// write. Root, and some filesystems, ignore mode bits; a test that needs a
/// read-only directory skips when this is false (the mode is restored).
#[cfg(unix)]
fn make_read_only(dir: &Path) -> bool {
    set_mode(dir, 0o500);
    let probe = dir.join(".mode-probe");
    match std::fs::write(&probe, b"x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            set_mode(dir, 0o700);
            false
        }
        Err(_) => true,
    }
}

// --- matrix rows -------------------------------------------------------------

#[test]
fn first_start_is_fresh_and_writes_the_file_on_the_first_start_decision() {
    let dir = tmpdir("first");
    let cfg = cfg_in(&dir);
    let path = lease_state_path(&cfg);
    assert_eq!(path, dir.join("lease-state.json"));

    let t = now();
    let ctx = LicenseContext::open(
        "station-1",
        Some(unlimited("lic")),
        detect_lease_store(&cfg),
        t,
    );
    let lease = ctx.lease_state();
    assert_eq!(lease.erasures_used, 0);
    assert_eq!(lease.time_watermark, t);
    assert!(!lease.counter_is_monotonic, "a file is not hardware-backed");
    assert!(!path.exists(), "nothing written before a start decision");

    let decision = decide(&ctx, t).unwrap();
    assert!(!decision.evaluation);
    let on_disk: LeaseState =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk.license_id.as_deref(), Some("lic"));

    let p = ctx.lease_persistence();
    assert_eq!(p.tier, StoreTier::LocalFile);
    assert!(p.survives_reboot);
    assert_eq!(p.location, path.display().to_string());
    // Only the lease file: no temp files left beside it.
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["lease-state.json".to_string()]);
}

#[test]
fn a_restart_keeps_the_count_and_the_watermark() {
    let dir = tmpdir("restart");
    let cfg = cfg_in(&dir);
    let w = now() - Duration::hours(1);
    LocalFileLeaseStore::new(lease_state_path(&cfg))
        .save(&stored(Some("lic"), 3, w))
        .unwrap();

    let ctx = LicenseContext::open(
        "station-1",
        Some(unlimited("lic")),
        detect_lease_store(&cfg),
        now(),
    );
    let lease = ctx.lease_state();
    assert_eq!(lease.erasures_used, 3);
    assert_eq!(lease.time_watermark, w);
}

#[test]
fn a_clock_rolled_back_after_restart_is_still_detected() {
    let dir = tmpdir("rollback");
    let cfg = cfg_in(&dir);
    let w = now();
    LocalFileLeaseStore::new(lease_state_path(&cfg))
        .save(&stored(Some("lic"), 0, w))
        .unwrap();

    let rewound = w - Duration::days(1);
    let ctx = LicenseContext::open(
        "station-1",
        Some(unlimited("lic")),
        detect_lease_store(&cfg),
        rewound,
    );
    match ctx.status(rewound) {
        LeaseStatus::ClockRollback { watermark, .. } => assert_eq!(watermark, w),
        other => panic!("expected ClockRollback, got {other:?}"),
    }
    assert!(matches!(
        decide(&ctx, rewound),
        Err(StartRefusal::ClockRollback { .. })
    ));
    assert_eq!(ctx.lease_state().time_watermark, w, "watermark stays W");
}

#[test]
fn a_different_licence_resets_the_count_and_keeps_the_watermark() {
    let dir = tmpdir("relicence");
    let cfg = cfg_in(&dir);
    let w = now() - Duration::hours(2);
    LocalFileLeaseStore::new(lease_state_path(&cfg))
        .save(&stored(Some("old"), 5, w))
        .unwrap();

    let ctx = LicenseContext::open(
        "station-1",
        Some(unlimited("new")),
        detect_lease_store(&cfg),
        now(),
    );
    let lease = ctx.lease_state();
    assert_eq!(lease.erasures_used, 0);
    assert_eq!(lease.time_watermark, w);
    assert_eq!(lease.license_id.as_deref(), Some("new"));
}

#[test]
fn a_different_licence_keeps_the_evaluation_count() {
    let dir = tmpdir("relicence-eval");
    let cfg = cfg_in(&dir);
    let mut old = stored(Some("old"), 5, now() - Duration::hours(2));
    old.evaluation_erasures = 4;
    LocalFileLeaseStore::new(lease_state_path(&cfg))
        .save(&old)
        .unwrap();

    let ctx = LicenseContext::open(
        "station-1",
        Some(unlimited("new")),
        detect_lease_store(&cfg),
        now(),
    );
    let lease = ctx.lease_state();
    assert_eq!(lease.erasures_used, 0);
    assert_eq!(lease.evaluation_erasures, 4, "a station-lifetime count");
}

#[test]
fn an_unlicensed_start_leaves_a_licensed_count_alone() {
    let dir = tmpdir("unlicensed");
    let cfg = cfg_in(&dir);
    let w = now() - Duration::hours(2);
    LocalFileLeaseStore::new(lease_state_path(&cfg))
        .save(&stored(Some("old"), 5, w))
        .unwrap();

    let ctx = LicenseContext::open("station-1", None, detect_lease_store(&cfg), now());
    assert_eq!(decide(&ctx, now()), Err(StartRefusal::NoLicence));
    assert_eq!(ctx.view(now(), false).status, LeaseStatus::Unlicensed);
    let lease = ctx.lease_state();
    assert_eq!(lease.erasures_used, 5);
    assert_eq!(lease.license_id.as_deref(), Some("old"));

    // And what it saved still holds the licensed count.
    let back = LocalFileLeaseStore::new(lease_state_path(&cfg))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(back.erasures_used, 5);
    assert_eq!(back.license_id.as_deref(), Some("old"));
}

#[test]
fn a_corrupt_lease_file_is_reported_and_never_overwritten_and_an_unlimited_licence_stays_licensed()
{
    let dir = tmpdir("corrupt");
    let cfg = cfg_in(&dir);
    let path = lease_state_path(&cfg);
    std::fs::write(&path, b"not json").unwrap();

    let ctx = LicenseContext::open(
        "station-1",
        Some(unlimited("lic")),
        detect_lease_store(&cfg),
        now(),
    );
    let decision = decide(&ctx, now()).unwrap();
    assert!(
        !decision.evaluation,
        "a lease fault never uncovers a valid licence"
    );
    let status = ctx.view(now(), false).status;
    match &status {
        LeaseStatus::LeaseStateUnavailable { detail } => {
            assert!(detail.contains(&path.display().to_string()), "{detail}")
        }
        other => panic!("expected LeaseStateUnavailable, got {other:?}"),
    }
    assert!(status
        .operator_message()
        .contains(&path.display().to_string()));
    ctx.spend(&decision);
    decide(&ctx, now()).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"not json", "file untouched");
    assert!(ctx
        .lease_persistence()
        .detail
        .contains(&path.display().to_string()));
}

#[cfg(unix)]
#[test]
fn a_count_reservation_that_cannot_be_saved_is_refused_and_the_next_save_retries() {
    let dir = tmpdir("save-fails");
    let cfg = cfg_in(&dir);
    let ctx = LicenseContext::open(
        "station-1",
        Some(counted("lic", 10)),
        detect_lease_store(&cfg),
        now(),
    );
    assert!(!decide(&ctx, now()).unwrap().evaluation);

    if !make_read_only(&dir) {
        return; // mode bits do not stop this user (root)
    }
    let failed = decide(&ctx, now());
    set_mode(&dir, 0o700);

    match failed {
        Err(StartRefusal::LeaseUnavailable { detail }) => {
            assert!(detail.contains("lease-state.json"), "{detail}")
        }
        other => panic!("expected LeaseUnavailable, got {other:?}"),
    }
    assert_eq!(
        ctx.lease_state().credits_reserved,
        1,
        "the refused one undone"
    );

    // Writable again: the next save succeeds and starts are licensed again.
    assert!(!decide(&ctx, now()).unwrap().evaluation);
    assert_eq!(ctx.lease_state().credits_reserved, 2);
}

#[cfg(unix)]
#[test]
fn an_unlimited_licence_whose_save_fails_stays_licensed() {
    let dir = tmpdir("save-fails-unlimited");
    let cfg = cfg_in(&dir);
    let ctx = LicenseContext::open(
        "station-1",
        Some(unlimited("lic")),
        detect_lease_store(&cfg),
        now(),
    );
    assert!(!decide(&ctx, now()).unwrap().evaluation);
    if !make_read_only(&dir) {
        return;
    }
    let decision = decide(&ctx, now());
    let status = ctx.view(now(), false);
    set_mode(&dir, 0o700);
    let decision = decision.unwrap();
    assert!(!decision.evaluation);
    assert!(decision.attestation.is_some());
    assert!(matches!(
        status.status,
        LeaseStatus::LeaseStateUnavailable { .. }
    ));
    assert_eq!(status.new_jobs_refused, None);
}

#[cfg(unix)]
#[test]
fn an_unwritable_directory_runs_lease_state_in_ram_and_says_what_resets() {
    let cfg = StoreConfig {
        explicit_path: Some(PathBuf::from("/proc/definitely/not/writable/x.json")),
        control_plane_url: Some("https://hub.example.com".into()),
        station_id: "station-1".into(),
    };
    let store = detect_lease_store(&cfg);
    assert_eq!(store.tier(), StoreTier::Ephemeral);

    let ctx = LicenseContext::open("station-1", Some(unlimited("lic")), store, now());
    // RAM-only is not a fault: licensed starts still work this boot.
    assert!(!decide(&ctx, now()).unwrap().evaluation);
    let p = ctx.lease_persistence();
    assert_eq!(p.tier, StoreTier::Ephemeral);
    assert!(!p.survives_reboot);
    assert!(p.detail.contains(RESET_WORDING), "{}", p.detail);
    assert!(
        p.detail.contains("does not hold lease state"),
        "{}",
        p.detail
    );
}

#[cfg(unix)]
#[test]
fn a_read_only_config_directory_is_detected_as_ram_only() {
    let dir = tmpdir("ro-dir");
    if !make_read_only(&dir) {
        return;
    }
    let store = detect_lease_store(&cfg_in(&dir));
    set_mode(&dir, 0o700);
    assert_eq!(store.tier(), StoreTier::Ephemeral);
    assert!(store.detail().contains(RESET_WORDING));
}

#[tokio::test]
async fn with_no_store_wired_lease_state_is_ram_only_and_says_so() {
    let p = LicenseContext::unlicensed("station-1", now()).lease_persistence();
    assert_eq!(p.tier, StoreTier::Ephemeral);
    assert!(!p.survives_reboot);
    assert!(p.detail.contains(RESET_WORDING), "{}", p.detail);

    let backend = Arc::new(wipe_engine_mock::MockBackend::fast_catalog());
    let key = Arc::new(wipe_cert::SigningKey::generate());
    let state = AppState::new(backend, None, key);
    let p = state.license().lease_persistence();
    assert_eq!(p.tier, StoreTier::Ephemeral);
    assert!(p.detail.contains(RESET_WORDING), "{}", p.detail);

    let ephemeral = EphemeralLeaseStore::new("x.".into());
    assert!(ephemeral.load().unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn a_read_only_directory_holding_a_lease_file_starts_from_its_count() {
    let dir = tmpdir("ro-seeded");
    let cfg = cfg_in(&dir);
    let w = now() - Duration::hours(1);
    LocalFileLeaseStore::new(lease_state_path(&cfg))
        .save(&stored(Some("lic"), 4, w))
        .unwrap();
    if !make_read_only(&dir) {
        return;
    }
    let store = detect_lease_store(&cfg);
    set_mode(&dir, 0o700);
    assert_eq!(store.tier(), StoreTier::Ephemeral);
    assert!(store.detail().contains("read from"), "{}", store.detail());

    let ctx = LicenseContext::open("station-1", Some(unlimited("lic")), store, now());
    let lease = ctx.lease_state();
    assert_eq!(lease.erasures_used, 4, "a read-only boot must not refill");
    assert_eq!(lease.time_watermark, w);
}

#[cfg(unix)]
#[test]
fn a_read_only_directory_holding_a_corrupt_lease_file_is_a_sticky_fault() {
    let dir = tmpdir("ro-corrupt");
    let cfg = cfg_in(&dir);
    let path = lease_state_path(&cfg);
    std::fs::write(&path, b"not json").unwrap();
    if !make_read_only(&dir) {
        return;
    }
    let store = detect_lease_store(&cfg);
    set_mode(&dir, 0o700);
    assert_eq!(store.tier(), StoreTier::Ephemeral);

    // A licence that counts credits never counts from a fresh lease.
    let ctx = LicenseContext::open("station-1", Some(counted("lic", 10)), store, now());
    match decide(&ctx, now()) {
        Err(StartRefusal::LeaseUnavailable { detail }) => {
            assert!(detail.contains(&path.display().to_string()), "{detail}")
        }
        other => panic!("expected LeaseUnavailable, got {other:?}"),
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"not json", "file untouched");
}

#[test]
fn status_on_a_corrupt_lease_file_is_unavailable_and_names_the_file() {
    let dir = tmpdir("corrupt-status");
    let cfg = cfg_in(&dir);
    let path = lease_state_path(&cfg);
    std::fs::write(&path, b"{").unwrap();

    let ctx = LicenseContext::open(
        "station-1",
        Some(unlimited("lic")),
        detect_lease_store(&cfg),
        now(),
    );
    match ctx.status(now()) {
        LeaseStatus::LeaseStateUnavailable { detail } => {
            let shown = path.display().to_string();
            assert_eq!(detail.matches(&shown).count(), 1, "path once: {detail}");
            assert!(detail.contains("restart the station"), "{detail}");
            assert!(!detail.contains("configuration"), "{detail}");
        }
        other => panic!("expected LeaseStateUnavailable, got {other:?}"),
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"{", "file untouched");
}

/// A store whose `save` blocks until the test opens the gate: a hung disk.
struct GatedStore {
    open: std::sync::Mutex<bool>,
    cv: std::sync::Condvar,
    inner: EphemeralLeaseStore,
}

impl GatedStore {
    fn new() -> Self {
        Self {
            open: std::sync::Mutex::new(false),
            cv: std::sync::Condvar::new(),
            inner: EphemeralLeaseStore::new("gated.".into()),
        }
    }

    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.cv.notify_all();
    }
}

impl LeaseStore for GatedStore {
    fn tier(&self) -> StoreTier {
        StoreTier::LocalFile
    }
    fn location(&self) -> String {
        "gated".into()
    }
    fn detail(&self) -> String {
        "gated".into()
    }
    fn load(&self) -> Result<Option<LeaseState>, wipe_server::store::StoreError> {
        Ok(None)
    }
    fn save(&self, state: &LeaseState) -> Result<(), wipe_server::store::StoreError> {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.cv.wait(open).unwrap();
        }
        self.inner.save(state)
    }
}

#[test]
fn a_hung_lease_save_refuses_a_counted_start_within_a_bounded_wait() {
    let store = Arc::new(GatedStore::new());
    let ctx = LicenseContext::open("station-1", Some(counted("lic", 10)), store.clone(), now());

    let started = std::time::Instant::now();
    let decision = decide(&ctx, now());
    assert!(
        started.elapsed() < StdDuration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
    assert!(
        matches!(decision, Err(StartRefusal::LeaseUnavailable { .. })),
        "{decision:?}"
    );
    assert_eq!(ctx.lease_state().credits_reserved, 0);

    // The disk comes back: the queued saves land and licensed starts return.
    store.release();
    let recovered = decide(&ctx, now()).unwrap();
    assert!(!recovered.evaluation);
    assert_eq!(
        store.inner.load().unwrap().unwrap().license_id.as_deref(),
        Some("lic")
    );
}

// --- end to end: a Count quota survives a restart ------------------------------

fn spec(device: &str) -> JobSpec {
    JobSpec {
        device_id: DeviceId(device.into()),
        classification: Classification::High,
        intent: Intent::Reuse,
        operator: OperatorRef {
            id: "op-1".into(),
            display_name: "Alice".into(),
            email: "alice@example.com".into(),
        },
        asset_tag: None,
        site_label: None,
        ticket_ref: None,
        work_order_ref: None,
        customer_ref: None,
        contract_ref: None,
        sanitization_profile_ref: None,
    }
}

/// A station built from `dir`, as `wipestation serve` builds one.
fn station(dir: &Path, key: &Arc<wipe_cert::SigningKey>, chain: &AttestationChain) -> AppState {
    station_on(detect_lease_store(&cfg_in(dir)), key, Some(chain))
}

/// A station over `store`, licensed by `chain` or unlicensed (free tier).
fn station_on(
    store: Arc<dyn LeaseStore>,
    key: &Arc<wipe_cert::SigningKey>,
    chain: Option<&AttestationChain>,
) -> AppState {
    let backend = Arc::new(wipe_engine_mock::MockBackend::fast_catalog());
    let ctx = LicenseContext::open("station-1", chain.cloned(), store, now());
    AppState::new(backend, None, key.clone()).with_license(Arc::new(ctx))
}

/// Erase `device` to a signed certificate; returns its `evaluation` flag.
async fn erase_to_certificate(state: &AppState, device: &str) -> bool {
    let id: Uuid = state.runner.create_job(spec(device)).await.unwrap();
    state.start_job(id).await.unwrap();
    for _ in 0..400 {
        if let Some(signed) = state.certs.read().get(&id) {
            return signed.certificate.evaluation;
        }
        tokio::time::sleep(StdDuration::from_millis(25)).await;
    }
    panic!(
        "no certificate for {device}; job state {:?}",
        state.runner.get(id).map(|j| j.state)
    );
}

#[tokio::test]
async fn a_count_quota_used_up_before_a_restart_stays_used_up_after_it() {
    let dir = tmpdir("quota-e2e");
    let key = Arc::new(wipe_cert::SigningKey::generate());
    let chain = chain(
        "lic-count",
        Quota::Count { erasures: 3 },
        &key.public_key_id(),
    );

    let first = station(&dir, &key, &chain);
    for device in ["dev-nvme-0", "dev-nvme-1", "dev-hdd-0"] {
        assert!(
            !erase_to_certificate(&first, device).await,
            "{device} should be licensed"
        );
    }
    assert_eq!(first.license().lease_state().erasures_used, 3);
    let watermark = first.license().lease_state().time_watermark;
    drop(first);

    // "Reboot": a new station (and a fresh mock) over the same config
    // directory. dev-sata-0 is the mock's simulated failure, so it is not used.
    let second = station(&dir, &key, &chain);
    let lease = second.license().lease_state();
    assert_eq!(lease.erasures_used, 3, "restart must not refill the quota");
    assert_eq!(lease.time_watermark, watermark);
    assert_eq!(
        second.license().status(now()),
        LeaseStatus::QuotaExhausted {
            used: 3,
            allowed: 3
        }
    );
    assert!(
        erase_to_certificate(&second, "dev-nvme-0").await,
        "the next certificate is evaluation"
    );
}

// --- end to end: free-tier erasures are counted ---------------------------------

#[tokio::test]
async fn an_unlicensed_station_counts_its_evaluation_erasures_across_a_restart() {
    let dir = tmpdir("eval-e2e");
    let key = Arc::new(wipe_cert::SigningKey::generate());

    let first = station_on(detect_lease_store(&cfg_in(&dir)), &key, None);
    for device in ["dev-nvme-0", "dev-nvme-1"] {
        assert!(erase_to_certificate(&first, device).await, "{device}");
    }
    let lease = first.license().lease_state();
    assert_eq!(lease.evaluation_erasures, 2);
    assert_eq!(lease.erasures_used, 0);
    drop(first);

    let second = station_on(detect_lease_store(&cfg_in(&dir)), &key, None);
    let lease = second.license().lease_state();
    assert_eq!(lease.evaluation_erasures, 2, "restart keeps the count");
    assert_eq!(lease.erasures_used, 0);
}

#[tokio::test]
async fn a_licensed_station_past_its_quota_counts_the_rest_as_evaluation() {
    let dir = tmpdir("eval-quota");
    let key = Arc::new(wipe_cert::SigningKey::generate());
    let chain = chain(
        "lic-one",
        Quota::Count { erasures: 1 },
        &key.public_key_id(),
    );

    let state = station(&dir, &key, &chain);
    assert!(!erase_to_certificate(&state, "dev-nvme-0").await);
    assert!(erase_to_certificate(&state, "dev-nvme-1").await);
    let lease = state.license().lease_state();
    assert_eq!(lease.erasures_used, 1);
    assert_eq!(lease.evaluation_erasures, 1);
    drop(state);

    // Restarted with no licence: both counts survive, untouched.
    let unlicensed = station_on(detect_lease_store(&cfg_in(&dir)), &key, None);
    let lease = unlicensed.license().lease_state();
    assert_eq!(lease.erasures_used, 1);
    assert_eq!(lease.evaluation_erasures, 1);
    assert_eq!(lease.license_id.as_deref(), Some("lic-one"));
}

/// A store that loads nothing and refuses every save.
struct FailingStore;

impl LeaseStore for FailingStore {
    fn tier(&self) -> StoreTier {
        StoreTier::LocalFile
    }
    fn location(&self) -> String {
        "failing".into()
    }
    fn detail(&self) -> String {
        "failing".into()
    }
    fn load(&self) -> Result<Option<LeaseState>, wipe_server::store::StoreError> {
        Ok(None)
    }
    fn save(&self, _: &LeaseState) -> Result<(), wipe_server::store::StoreError> {
        Err(wipe_server::store::StoreError::Unavailable)
    }
}

#[tokio::test]
async fn a_failed_save_still_counts_the_evaluation_erasure_in_ram_and_signs() {
    let key = Arc::new(wipe_cert::SigningKey::generate());
    let state = station_on(Arc::new(FailingStore), &key, None);
    assert!(erase_to_certificate(&state, "dev-nvme-0").await);
    let lease = state.license().lease_state();
    assert_eq!(lease.evaluation_erasures, 1);
    assert_eq!(lease.erasures_used, 0);
}
