//! End-to-end test: spin up the Axum server with the mock backend, drive
//! it via HTTP, and verify a signed certificate is produced for an
//! Erased Job and an aborted Job has no cert.

use std::{sync::Arc, time::Duration};

use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine as _};
use serde_json::json;
use wipe_cert::{SigningKey, VerifyingKey};
use wipe_engine_mock::{MockBackend, MockTiming};
use wipe_server::{router, AppState};

async fn spawn_server() -> String {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let signing_key = Arc::new(SigningKey::generate());
    let state = AppState::new(backend, None, signing_key);
    let app = router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn full_http_flow_completes_with_signed_cert() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();

    // Health.
    let r: serde_json::Value = client
        .get(format!("{base}/api/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["ok"], true);

    // Public key — capture for later offline verification.
    let pk: serde_json::Value = client
        .get(format!("{base}/api/public_key"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let vk_b64 = pk["public_key_b64"].as_str().unwrap().to_string();
    let vk_id = pk["public_key_id"].as_str().unwrap().to_string();

    // List devices.
    let devices: serde_json::Value = client
        .get(format!("{base}/api/devices"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(devices.as_array().unwrap().len() >= 4);

    // Create a Job (outer model — no inner method/verify fields).
    let create_resp: serde_json::Value = client
        .post(format!("{base}/api/jobs"))
        .json(&json!({
            "device_id": "dev-nvme-0",
            "classification": "high",
            "intent": "reuse",
            "operator": {
                "id": "op-1",
                "display_name": "Alice",
                "email": "alice@example.com"
            },
            "asset_tag": "ASSET-7",
            "site_label": "Lab",
            "ticket_ref": "TKT-99"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let job_id = create_resp["job_id"].as_str().unwrap().to_string();

    // Start it.
    client
        .post(format!("{base}/api/jobs/{job_id}/start"))
        .send()
        .await
        .unwrap();

    // Poll until the outer Job reaches Erased.
    let mut erased = false;
    for _ in 0..400 {
        let job: serde_json::Value = client
            .get(format!("{base}/api/jobs/{job_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let state = job["state"]["state"].as_str().unwrap_or("");
        if state == "erased" {
            erased = true;
            break;
        }
        if state == "aborted" || state == "quarantined" || state == "destroyed" {
            panic!("unexpected disposition: {state}, full job: {job}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(erased, "job did not reach Erased in time");

    // Cert generation runs via the broadcast subscriber — give it a moment.
    let mut signed: Option<serde_json::Value> = None;
    for _ in 0..50 {
        let resp = client
            .get(format!("{base}/api/jobs/{job_id}/certificate"))
            .send()
            .await
            .unwrap();
        if resp.status().is_success() {
            signed = Some(resp.json().await.unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let signed = signed.expect("certificate should be available");
    assert_eq!(
        signed["certificate"]["operator"]["email"],
        "alice@example.com"
    );
    assert_eq!(signed["certificate"]["disposition"], "erased");
    assert_eq!(signed["certificate"]["cert_format_version"], 2);
    assert_eq!(signed["signature"]["algorithm"], "ed25519");
    // Activity chain should be carried on the cert.
    let activities = signed["certificate"]["activities"].as_array().unwrap();
    assert!(activities.iter().any(|a| a["type"] == "erasure"));
    assert!(activities.iter().any(|a| a["type"] == "verification"));

    // Verify offline.
    let parsed: wipe_cert::SignedCertificate = serde_json::from_value(signed).unwrap();
    assert_eq!(parsed.signature.public_key_id, vk_id);
    let vk_bytes_vec = B64.decode(vk_b64.as_bytes()).unwrap();
    let vk_bytes: [u8; 32] = vk_bytes_vec.try_into().expect("32-byte ed25519 key");
    let vk = VerifyingKey::from_bytes(&vk_bytes).unwrap();
    wipe_cert::verify(&parsed, &[vk]).expect("cert verifies against published public key");
}

#[tokio::test]
async fn aborted_job_does_not_get_certificate() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();

    let create_resp: serde_json::Value = client
        .post(format!("{base}/api/jobs"))
        .json(&json!({
            "device_id": "dev-hdd-0",
            "classification": "moderate",
            "intent": "reuse",
            "operator": {
                "id": "op-2",
                "display_name": "Bob",
                "email": "bob@example.com"
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let job_id = create_resp["job_id"].as_str().unwrap().to_string();

    client
        .post(format!("{base}/api/jobs/{job_id}/start"))
        .send()
        .await
        .unwrap();
    // Abort almost immediately.
    tokio::time::sleep(Duration::from_millis(50)).await;
    client
        .post(format!("{base}/api/jobs/{job_id}/abort"))
        .send()
        .await
        .unwrap();

    // Wait for terminal.
    for _ in 0..50 {
        let job: serde_json::Value = client
            .get(format!("{base}/api/jobs/{job_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let state = job["state"]["state"].as_str().unwrap_or("");
        if state == "aborted" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Cert endpoint should 404.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let resp = client
        .get(format!("{base}/api/jobs/{job_id}/certificate"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

/// The most recent `destruction` activity in a Job's JSON.
fn latest_destruction(job: &serde_json::Value) -> serde_json::Value {
    job["activities"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|a| a["type"] == "destruction")
        .cloned()
        .expect("job has a destruction activity")
}

/// The bench-setup preview POSTs an unsaved draft here for live occupancy
/// (ADR-0002: the matching rules have one home, so the frontend must not
/// re-implement them). The handler was written and documented but never
/// routed, so the request fell through to the static handler and came back
/// 405 — covered here at the HTTP layer, which is where the gap was.
#[tokio::test]
async fn unsaved_topology_resolves_as_a_dry_run() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();

    // The draft an editor would be holding: this station's config, relabelled.
    let cfg_resp = client
        .get(format!("{base}/api/bay-topology/config"))
        .send()
        .await
        .unwrap();
    let cfg_status = cfg_resp.status();
    let cfg_body = cfg_resp.text().await.unwrap();
    assert_eq!(
        cfg_status, 200,
        "GET /api/bay-topology/config returned {cfg_status}: {cfg_body}"
    );
    let mut draft: serde_json::Value = serde_json::from_str(&cfg_body).unwrap();
    draft["label"] = json!("Draft bench");

    let resp = client
        .post(format!("{base}/api/bay-topology/resolve"))
        .json(&draft)
        .send()
        .await
        .unwrap();
    // Status alone is not enough: with no dist directory configured, an
    // unrouted POST falls through to the landing page and answers 200 with
    // HTML. The content type is what distinguishes the handler from the
    // fallback.
    let status = resp.status();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = resp.text().await.unwrap();
    assert_eq!(
        status, 200,
        "POST /api/bay-topology/resolve must be routed, got {status}"
    );
    assert!(
        content_type.starts_with("application/json"),
        "expected the resolve handler, got {content_type} — the request fell \
         through to the static/landing fallback: {}",
        body.chars().take(120).collect::<String>()
    );

    let resolved: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(resolved["topology"]["label"], "Draft bench");

    // Resolution actually ran against the attached devices rather than
    // echoing the draft back.
    let occupied = resolved["occupancy"].as_array().unwrap().len();
    assert!(
        occupied >= 4,
        "expected the mock backend's drives to be placed, got {occupied}"
    );

    // Dry run: the draft is not persisted and the stored config is untouched.
    let after: serde_json::Value = client
        .get(format!("{base}/api/bay-topology/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(
        after["label"], "Draft bench",
        "resolve must not persist the draft it was given"
    );
}

/// One Job, one live destruction run: a second run would let a second
/// completion decide the same Job's destruction again.
#[tokio::test]
async fn a_job_cannot_be_on_two_manifests() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let op = http_op();
    let job_id = http_certified_pending_job(&client, &base, "dev-nvme-0").await;

    let first = client
        .post(format!("{base}/api/manifests"))
        .json(&json!({ "assembled_by": op, "job_ids": [job_id] }))
        .send()
        .await
        .unwrap();
    assert!(first.status().is_success());

    let second = client
        .post(format!("{base}/api/manifests"))
        .json(&json!({ "assembled_by": op, "job_ids": [job_id] }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 409);
    let first: serde_json::Value = first.json().await.unwrap();
    let job: serde_json::Value = client
        .get(format!("{base}/api/jobs/{job_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(job["manifest_id"], first["id"]);
    let manifests: serde_json::Value = client
        .get(format!("{base}/api/manifests"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(manifests.as_array().unwrap().len(), 1);

    let dup = client
        .post(format!("{base}/api/manifests"))
        .json(&json!({ "assembled_by": op, "job_ids": [job_id, job_id] }))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), 422);
}

/// A refused manifest stamps nothing: one PendingDestruction Job alongside a
/// Queued one is a 409, neither Job carries a manifest id, and no manifest
/// is stored.
#[tokio::test]
async fn a_refused_manifest_links_no_job() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let op = json!({ "id": "op-1", "display_name": "Alice", "email": "alice@example.com" });

    let mut ids = Vec::new();
    for device in ["dev-nvme-0", "dev-sata-0"] {
        let created: serde_json::Value = client
            .post(format!("{base}/api/jobs"))
            .json(&json!({
                "device_id": device,
                "classification": "high",
                "intent": "destroy",
                "operator": op,
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        ids.push(created["job_id"].as_str().unwrap().to_string());
    }
    let (pending, queued) = (&ids[0], &ids[1]);
    let resp = client
        .post(format!("{base}/api/jobs/{pending}/record-for-destruction"))
        .json(&attested("shred", op.clone()))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let resp = client
        .post(format!("{base}/api/manifests"))
        .json(&json!({ "assembled_by": op, "job_ids": [pending, queued] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    for id in &ids {
        let job: serde_json::Value = client
            .get(format!("{base}/api/jobs/{id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(job["manifest_id"].is_null(), "job {id} was stamped");
    }
    let job: serde_json::Value = client
        .get(format!("{base}/api/jobs/{pending}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(latest_destruction(&job)["manifest_ref"].is_null());
    let manifests: serde_json::Value = client
        .get(format!("{base}/api/manifests"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(manifests.as_array().unwrap().is_empty());
}

/// Once a Job is on a destruction run, the record the run's completion
/// covers is fixed: recording it again is refused, appends no
/// DestructionEvent and leaves the stored certificate in place.
#[tokio::test]
async fn re_recording_a_job_on_a_manifest_is_refused() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let op = json!({ "id": "op-1", "display_name": "Alice", "email": "alice@example.com" });

    let created: serde_json::Value = client
        .post(format!("{base}/api/jobs"))
        .json(&json!({
            "device_id": "dev-nvme-0",
            "classification": "high",
            "intent": "destroy",
            "operator": op,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let job_id = created["job_id"].as_str().unwrap().to_string();
    client
        .post(format!("{base}/api/jobs/{job_id}/start"))
        .send()
        .await
        .unwrap();
    // The certificate needs an erasure activity to snapshot the device.
    let mut has_erasure = false;
    for _ in 0..100 {
        let job: serde_json::Value = client
            .get(format!("{base}/api/jobs/{job_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if job["activities"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x["type"] == "erasure"))
        {
            has_erasure = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(has_erasure, "job never gained an erasure activity");

    let resp = client
        .post(format!("{base}/api/jobs/{job_id}/record-for-destruction"))
        .json(&attested("disintegrate", op.clone()))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let mut cert: Option<serde_json::Value> = None;
    for _ in 0..100 {
        let resp = client
            .get(format!("{base}/api/jobs/{job_id}/certificate"))
            .send()
            .await
            .unwrap();
        if resp.status().is_success() {
            cert = Some(resp.json().await.unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let cert = cert.expect("certificate generated at pending_destruction");

    let resp = client
        .post(format!("{base}/api/manifests"))
        .json(&json!({ "assembled_by": op, "job_ids": [job_id] }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let before: serde_json::Value = client
        .get(format!("{base}/api/jobs/{job_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let resp = client
        .post(format!("{base}/api/jobs/{job_id}/record-for-destruction"))
        .json(&attested("shred", op.clone()))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        409,
        "re-recording a linked job must be a conflict"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    let manifest_id = before["manifest_id"].as_str().unwrap();
    let message = body["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("destruction manifest") && message.contains(manifest_id),
        "refusal should name the destruction manifest, got {body}"
    );

    // Give a (wrongly emitted) state change time to re-sign before checking.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after: serde_json::Value = client
        .get(format!("{base}/api/jobs/{job_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Count destruction activities only: the erasure is still running in
    // the background and may append its own activities meanwhile.
    let destructions = |job: &serde_json::Value| {
        job["activities"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["type"] == "destruction")
            .count()
    };
    assert_eq!(destructions(&after), 1);
    assert_eq!(destructions(&before), 1);
    assert_eq!(latest_destruction(&after)["method"], "disintegrate");
    let cert_after: serde_json::Value = client
        .get(format!("{base}/api/jobs/{job_id}/certificate"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cert_after["signature"], cert["signature"]);
}

/// Wire shape of the manifest link over HTTP: the certificate's destruction
/// activity has `manifest_ref` null while the live Job carries it. Assembly
/// waits for the certificate (a run never holds an uncertified Job), so
/// over HTTP the certificate is always signed first; the generator's strip
/// is proven by `certificate_signed_after_linking_still_omits_the_manifest`.
#[tokio::test]
async fn a_manifest_assembled_before_the_certificate_is_signed_stays_out_of_it() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let op = json!({ "id": "op-1", "display_name": "Alice", "email": "alice@example.com" });

    let created: serde_json::Value = client
        .post(format!("{base}/api/jobs"))
        .json(&json!({
            "device_id": "dev-nvme-0",
            "classification": "high",
            "intent": "destroy",
            "operator": op,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let job_id = created["job_id"].as_str().unwrap().to_string();
    client
        .post(format!("{base}/api/jobs/{job_id}/start"))
        .send()
        .await
        .unwrap();
    // The certificate needs an erasure activity to snapshot the device.
    let mut has_erasure = false;
    for _ in 0..100 {
        let job: serde_json::Value = client
            .get(format!("{base}/api/jobs/{job_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if job["activities"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x["type"] == "erasure"))
        {
            has_erasure = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(has_erasure, "job never gained an erasure activity");

    let resp = client
        .post(format!("{base}/api/jobs/{job_id}/record-for-destruction"))
        .json(&attested("shred", op.clone()))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    // Assemble at once. Until the certificate is signed, assembly refuses
    // the Job (409 naming it), so the run is retried until it is accepted.
    let mut manifest_id = None;
    for _ in 0..100 {
        let (status, body) = http_post(
            &client,
            format!("{base}/api/manifests"),
            Some(json!({ "assembled_by": op, "job_ids": [job_id] })),
        )
        .await;
        if status == 200 {
            manifest_id = Some(body["id"].as_str().unwrap().to_string());
            break;
        }
        assert_eq!(status, 409, "{body}");
        let message = body["error"].as_str().unwrap();
        assert!(
            message.contains(&job_id) && message.contains("no certificate"),
            "{body}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let manifest_id = manifest_id.expect("manifest stored once the certificate exists");

    let mut cert: Option<serde_json::Value> = None;
    for _ in 0..100 {
        let resp = client
            .get(format!("{base}/api/jobs/{job_id}/certificate"))
            .send()
            .await
            .unwrap();
        if resp.status().is_success() {
            cert = Some(resp.json().await.unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let cert = cert.expect("certificate generated at pending_destruction");
    let signed_destructions: Vec<&serde_json::Value> = cert["certificate"]["activities"]
        .as_array()
        .expect("certificate has activities")
        .iter()
        .filter(|a| a["type"] == "destruction")
        .collect();
    assert_eq!(signed_destructions.len(), 1);
    assert!(
        signed_destructions[0]["manifest_ref"].is_null(),
        "signed certificate carries the manifest link: {}",
        signed_destructions[0]
    );

    let job: serde_json::Value = client
        .get(format!("{base}/api/jobs/{job_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(job["manifest_id"], manifest_id.as_str());
    assert_eq!(
        latest_destruction(&job)["manifest_ref"],
        manifest_id.as_str()
    );
}

/// Deterministic companion to the test above: over HTTP the certificate
/// generator usually wins the race, so this drives `AppState` directly and
/// links the Job before the generator task can run at all (the
/// single-threaded test runtime does not poll it until we yield).
#[tokio::test]
async fn certificate_signed_after_linking_still_omits_the_manifest() {
    use wipe_common::{
        AttestationSigner, Classification, DestructMethod, DestructionAttestation,
        DestructionEvent, DeviceId, IdentifierSource, Intent, JobSpec, OperatorRef,
        RecordedIdentifiers,
    };

    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let state = AppState::new(backend, None, Arc::new(SigningKey::generate()));
    let op = OperatorRef {
        id: "op-1".into(),
        display_name: "Alice".into(),
        email: "alice@example.com".into(),
    };
    let device = DeviceId("dev-nvme-0".into());
    let job_id = state
        .runner
        .create_job(JobSpec {
            device_id: device.clone(),
            classification: Classification::High,
            intent: Intent::Destroy,
            operator: op.clone(),
            asset_tag: None,
            site_label: None,
            ticket_ref: None,
            work_order_ref: None,
            customer_ref: None,
            contract_ref: None,
            sanitization_profile_ref: None,
        })
        .await
        .unwrap();
    state.runner.start(job_id).unwrap();
    // The certificate needs an erasure activity to snapshot the device.
    let mut has_erasure = false;
    for _ in 0..100 {
        if state.runner.get(job_id).unwrap().latest_erasure().is_some() {
            has_erasure = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(has_erasure, "job never gained an erasure activity");

    // No await between these two calls: the generator sees the linked Job.
    state
        .runner
        .record_for_destruction(
            job_id,
            DestructionEvent {
                id: uuid::Uuid::new_v4(),
                device_id: device,
                at: time::OffsetDateTime::now_utc(),
                method: DestructMethod::Shred,
                operator: op.clone(),
                supervisor: None,
                manifest_ref: None,
                photo_refs: Vec::new(),
                notes: None,
                station_id: None,
                attestation: Some(
                    DestructionAttestation::new(
                        DestructMethod::Shred,
                        time::OffsetDateTime::now_utc(),
                        "SHR-2",
                        "Bay 3",
                        RecordedIdentifiers {
                            asset_tag: None,
                            serial: Some("S6ABNX0W123456".into()),
                            source: IdentifierSource::Device,
                        },
                        AttestationSigner::self_asserted(op.clone(), "Alice", "Bench lead")
                            .unwrap(),
                    )
                    .unwrap(),
                ),
            },
        )
        .unwrap();
    let manifest_id = uuid::Uuid::new_v4();
    state
        .runner
        .link_to_manifest(manifest_id, &[job_id])
        .unwrap();

    let mut cert = None;
    for _ in 0..100 {
        if let Some(c) = state.certs.read().get(&job_id).cloned() {
            cert = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let cert = serde_json::to_value(cert.expect("certificate generated")).unwrap();
    let destruction = cert["certificate"]["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["type"] == "destruction")
        .cloned()
        .expect("signed certificate has the destruction activity");
    assert!(
        destruction["manifest_ref"].is_null(),
        "signed certificate carries the manifest link: {destruction}"
    );
    assert_eq!(
        state.runner.get(job_id).unwrap().manifest_id,
        Some(manifest_id)
    );
}

// --- runner refusals map to 404 / 409 (CAP-3) --------------------------------

fn http_op() -> serde_json::Value {
    json!({ "id": "op-1", "display_name": "Alice", "email": "alice@example.com" })
}

/// An attested record-for-destruction body (ADR-0011 §1): every required
/// field filled, "I attest" ticked, identifiers read from the device.
fn attested(method: &str, operator: serde_json::Value) -> serde_json::Value {
    json!({
        "method": method,
        "equipment_id": "SHR-2",
        "location": "Bay 3",
        "operator": operator,
        "full_name": "Alice Erasure",
        "title": "Bench lead",
        "attest": true,
        "notes": null,
    })
}

async fn http_create_job(client: &reqwest::Client, base: &str, device: &str) -> String {
    let created: serde_json::Value = client
        .post(format!("{base}/api/jobs"))
        .json(&json!({
            "device_id": device,
            "classification": "high",
            "intent": "destroy",
            "operator": http_op(),
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    created["job_id"].as_str().unwrap().to_string()
}

/// POST `body` (or nothing) to `url`; return the status and the JSON body.
async fn http_post(
    client: &reqwest::Client,
    url: String,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let req = client.post(url);
    let req = match body {
        Some(b) => req.json(&b),
        None => req,
    };
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(serde_json::Value::Null))
}

async fn http_get_job(client: &reqwest::Client, base: &str, job_id: &str) -> serde_json::Value {
    client
        .get(format!("{base}/api/jobs/{job_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn http_wait_for_state(
    client: &reqwest::Client,
    base: &str,
    job_id: &str,
    state: &str,
) -> serde_json::Value {
    for _ in 0..200 {
        let job = http_get_job(client, base, job_id).await;
        if job["state"]["state"] == state {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("job {job_id} never reached {state}");
}

fn count_activities(job: &serde_json::Value, kind: &str) -> usize {
    job["activities"]
        .as_array()
        .map(|a| a.iter().filter(|x| x["type"] == kind).count())
        .unwrap_or(0)
}

#[tokio::test]
async fn unknown_job_id_is_404_on_start_abort_and_record() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let unknown = uuid::Uuid::new_v4();

    for (path, body) in [
        ("start", None),
        ("abort", None),
        ("record-for-destruction", Some(attested("shred", http_op()))),
    ] {
        let (status, body) =
            http_post(&client, format!("{base}/api/jobs/{unknown}/{path}"), body).await;
        assert_eq!(status, 404, "{path}: {body}");
        let message = body["error"].as_str().unwrap_or_default();
        assert!(
            message.contains(&unknown.to_string()),
            "{path}: error should name the job, got {body}"
        );
    }
}

#[tokio::test]
async fn malformed_job_id_is_still_400() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let (status, _) = http_post(&client, format!("{base}/api/jobs/not-a-uuid/start"), None).await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn starting_a_job_twice_is_200_then_409_with_one_erasure() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, &base, "dev-nvme-0").await;

    let (first, _) = http_post(&client, format!("{base}/api/jobs/{job_id}/start"), None).await;
    let (second, body) = http_post(&client, format!("{base}/api/jobs/{job_id}/start"), None).await;
    assert_eq!(first, 200);
    assert_eq!(second, 409, "{body}");
    assert!(body["error"]
        .as_str()
        .unwrap_or_default()
        .contains("is in state"));

    let job = http_wait_for_state(&client, &base, &job_id, "erased").await;
    assert_eq!(count_activities(&job, "erasure"), 1);
}

#[tokio::test]
async fn starting_a_job_recorded_for_destruction_is_409_and_it_stays_pending_destruction() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, &base, "dev-nvme-0").await;
    let (status, _) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("shred", http_op())),
    )
    .await;
    assert_eq!(status, 200);

    let (status, body) = http_post(&client, format!("{base}/api/jobs/{job_id}/start"), None).await;
    assert_eq!(status, 409, "{body}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "pending_destruction");
    assert_eq!(count_activities(&job, "erasure"), 0);
}

#[tokio::test]
async fn recording_an_aborted_job_is_409_and_appends_nothing() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, &base, "dev-nvme-0").await;
    let (status, _) = http_post(&client, format!("{base}/api/jobs/{job_id}/abort"), None).await;
    assert_eq!(status, 200);

    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("shred", http_op())),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "aborted");
    assert_eq!(count_activities(&job, "destruction"), 0);
}

#[tokio::test]
async fn aborting_an_erased_job_is_409_and_it_stays_erased() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, &base, "dev-nvme-0").await;
    let (status, _) = http_post(&client, format!("{base}/api/jobs/{job_id}/start"), None).await;
    assert_eq!(status, 200);
    http_wait_for_state(&client, &base, &job_id, "erased").await;

    let (status, body) = http_post(&client, format!("{base}/api/jobs/{job_id}/abort"), None).await;
    assert_eq!(status, 409, "{body}");
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "erased");
}

#[tokio::test]
async fn aborting_an_aborted_job_is_200() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, &base, "dev-nvme-0").await;

    let (first, _) = http_post(&client, format!("{base}/api/jobs/{job_id}/abort"), None).await;
    let (second, body) = http_post(&client, format!("{base}/api/jobs/{job_id}/abort"), None).await;
    assert_eq!(first, 200);
    assert_eq!(second, 200, "{body}");
    assert_eq!(body["ok"], true);
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "aborted");
}

/// ADR-0011 §5: a technique that is not NIST Destroy is still recorded as
/// what the bench actually did.
#[tokio::test]
async fn recording_with_a_non_nist_technique_records_it() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, &base, "dev-nvme-0").await;
    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("crush", http_op())),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    let job = http_get_job(&client, &base, &job_id).await;
    let destruction = job["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["type"] == "destruction")
        .expect("a destruction activity");
    assert_eq!(destruction["method"], "crush");
    assert!(destruction["attestation"]["statement"]
        .as_str()
        .unwrap()
        .contains("by Crush using SHR-2"));
}

// --- Record for destruction (destruction-attestation story 2) -----------------

async fn spawn_server_on(backend: Arc<MockBackend>) -> String {
    let state = AppState::new(backend, None, Arc::new(SigningKey::generate()));
    let app = router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn http_create_tagged_job(
    client: &reqwest::Client,
    base: &str,
    device: &str,
    asset_tag: &str,
) -> String {
    let (status, created) = http_post(
        client,
        format!("{base}/api/jobs"),
        Some(json!({
            "device_id": device,
            "classification": "moderate",
            "intent": "destroy",
            "operator": http_op(),
            "asset_tag": asset_tag,
        })),
    )
    .await;
    assert_eq!(status, 200, "{created}");
    created["job_id"].as_str().unwrap().to_string()
}

async fn http_list_jobs(client: &reqwest::Client, base: &str) -> Vec<serde_json::Value> {
    client
        .get(format!("{base}/api/jobs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn group_body(drives: serde_json::Value) -> serde_json::Value {
    let mut body = attested("shred", http_op());
    body["classification"] = json!("moderate");
    body["drives"] = drives;
    body
}

#[tokio::test]
async fn recording_a_queued_job_reads_the_identifiers_from_the_device() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_tagged_job(&client, &base, "dev-nvme-0", "A-17").await;

    let (status, preview) = http_post(
        &client,
        format!("{base}/api/destruction/statement"),
        Some(json!({
            "method": "shred", "equipment_id": "SHR-2", "location": "Bay 3",
            "full_name": "Alice Erasure", "title": "Bench lead",
        })),
    )
    .await;
    assert_eq!(status, 200, "{preview}");

    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("shred", http_op())),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "pending_destruction");
    let att = &latest_destruction(&job)["attestation"];
    assert_eq!(
        att["identifiers"],
        json!({"asset_tag": "A-17", "serial": "S6ABNX0W123456", "source": "device"})
    );
    assert_eq!(
        att["signer"]["identity_assurance"]["level"],
        "self_asserted"
    );
    assert_eq!(att["signer"]["full_name"], "Alice Erasure");
    assert_eq!(att["equipment_id"], "SHR-2");
    assert_eq!(att["location"], "Bay 3");
    assert_eq!(att["statement"], preview["statement"]);
}

#[tokio::test]
async fn recording_an_in_progress_job_with_scanned_identifiers_marks_them_scanned() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    // The mock's SATA drive fails its erasure, so the Job stays InProgress.
    let job_id = http_create_job(&client, &base, "dev-sata-0").await;
    let (status, _) = http_post(&client, format!("{base}/api/jobs/{job_id}/start"), None).await;
    assert_eq!(status, 200);
    let mut body = attested("shred", http_op());
    body["identifiers"] = json!({ "serial": "S1" });
    let (status, resp) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(body),
    )
    .await;
    assert_eq!(status, 200, "{resp}");
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "pending_destruction");
    assert_eq!(
        latest_destruction(&job)["attestation"]["identifiers"],
        json!({"asset_tag": null, "serial": "S1", "source": "scanned"})
    );
}

#[tokio::test]
async fn recording_a_job_whose_drive_cannot_be_read_asks_for_a_scan() {
    use wipe_engine::DeviceSimulator;
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let base = spawn_server_on(backend.clone()).await;
    let client = reqwest::Client::new();
    let job_id = http_create_tagged_job(&client, &base, "dev-nvme-0", "A-17").await;
    backend
        .detach(&wipe_common::DeviceId("dev-nvme-0".into()))
        .unwrap();

    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("shred", http_op())),
    )
    .await;
    assert_eq!(status, 422, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("scan"),
        "the refusal should tell the operator to scan, got {body}"
    );
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "queued");
    assert_eq!(count_activities(&job, "destruction"), 0);
}

#[tokio::test]
async fn recording_without_attesting_or_with_a_blank_field_records_nothing() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, &base, "dev-nvme-0").await;
    let url = format!("{base}/api/jobs/{job_id}/record-for-destruction");

    let mut cases: Vec<(serde_json::Value, &str)> = Vec::new();
    let mut not_attested = attested("shred", http_op());
    not_attested["attest"] = json!(false);
    cases.push((not_attested, "I attest"));
    let mut missing = attested("shred", http_op());
    missing.as_object_mut().unwrap().remove("attest");
    cases.push((missing, "I attest"));
    for field in ["equipment_id", "location", "full_name", "title"] {
        let mut blank = attested("shred", http_op());
        blank[field] = json!("  ");
        cases.push((blank, field));
    }
    for (body, needle) in cases {
        let (status, resp) = http_post(&client, url.clone(), Some(body)).await;
        assert_eq!(status, 422, "{needle}: {resp}");
        assert!(
            resp["error"].as_str().unwrap().contains(needle),
            "{needle}: {resp}"
        );
    }
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "queued");
    assert_eq!(count_activities(&job, "destruction"), 0);
}

#[tokio::test]
async fn recording_an_erased_job_is_409() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, &base, "dev-nvme-0").await;
    let (status, _) = http_post(&client, format!("{base}/api/jobs/{job_id}/start"), None).await;
    assert_eq!(status, 200);
    http_wait_for_state(&client, &base, &job_id, "erased").await;
    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("shred", http_op())),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "erased");
    assert_eq!(count_activities(&job, "destruction"), 0);
}

#[tokio::test]
async fn the_removed_escalation_route_is_404_and_changes_nothing() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, &base, "dev-nvme-0").await;
    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/escalate-to-destroy"),
        Some(json!({ "method": "shred", "operator": http_op() })),
    )
    .await;
    assert_eq!(status, 404, "{body}");
    assert!(body["error"].is_string(), "{body}");
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "queued");
    assert_eq!(count_activities(&job, "destruction"), 0);
}

#[tokio::test]
async fn recording_a_scanned_group_creates_one_pending_job_per_drive() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let (status, body) = http_post(
        &client,
        format!("{base}/api/destruction/records"),
        Some(group_body(json!([
            { "asset_tag": "A-1", "serial": "S1" },
            { "serial": "S2" },
            { "asset_tag": "A-3", "serial": null },
        ]))),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let ids: Vec<String> = body["job_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 3);

    let jobs = http_list_jobs(&client, &base).await;
    assert_eq!(jobs.len(), 3);
    let expected = [
        ("scanned:S1", json!("A-1"), json!("S1")),
        ("scanned:S2", json!(null), json!("S2")),
        ("scanned:A-3", json!("A-3"), json!(null)),
    ];
    let mut statements = Vec::new();
    for (id, (device_id, tag, serial)) in ids.iter().zip(expected) {
        let job = http_get_job(&client, &base, id).await;
        assert_eq!(job["state"]["state"], "pending_destruction");
        assert_eq!(job["spec"]["intent"], "destroy");
        assert_eq!(job["spec"]["classification"], "moderate");
        assert_eq!(job["spec"]["device_id"], device_id);
        let att = &latest_destruction(&job)["attestation"];
        assert_eq!(
            att["identifiers"],
            json!({"asset_tag": tag, "serial": serial, "source": "scanned"})
        );
        statements.push((att["statement"].clone(), att["at"].clone()));
    }
    assert!(
        statements.windows(2).all(|w| w[0] == w[1]),
        "a group shares one statement and one time"
    );
}

#[tokio::test]
async fn a_scanned_group_that_cannot_all_be_recorded_creates_nothing() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let url = format!("{base}/api/destruction/records");
    for (drives, needle) in [
        (json!([]), "at least one"),
        (
            json!([{ "serial": "S1" }, { "asset_tag": "A-2", "serial": "S1" }]),
            "S1",
        ),
        (
            json!([{ "asset_tag": "A-1" }, { "asset_tag": "a-1" }]),
            "listed twice",
        ),
        (
            json!([{ "serial": "S1" }, { "asset_tag": " ", "serial": "" }]),
            "drive 2",
        ),
    ] {
        let (status, body) = http_post(&client, url.clone(), Some(group_body(drives))).await;
        assert_eq!(status, 422, "{needle}: {body}");
        assert!(body["error"].as_str().unwrap().contains(needle), "{body}");
    }
    let mut not_attested = group_body(json!([{ "serial": "S1" }]));
    not_attested["attest"] = json!(false);
    let (status, _) = http_post(&client, url.clone(), Some(not_attested)).await;
    assert_eq!(status, 422);
    let mut blank = group_body(json!([{ "serial": "S1" }]));
    blank["location"] = json!("");
    let (status, body) = http_post(&client, url.clone(), Some(blank)).await;
    assert_eq!(status, 422);
    assert!(body["error"].as_str().unwrap().contains("location"));
    assert!(http_list_jobs(&client, &base).await.is_empty());
}

#[tokio::test]
async fn a_scanned_group_matching_an_open_job_is_409_naming_it() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let url = format!("{base}/api/destruction/records");
    // An open Job on an attached drive, with an asset tag.
    let open = http_create_tagged_job(&client, &base, "dev-nvme-0", "A-17").await;
    // An open Job recorded from scanned identifiers.
    let scanned = http_create_job(&client, &base, "dev-sata-0").await;
    let mut body = attested("shred", http_op());
    body["identifiers"] = json!({ "serial": "SCAN-9" });
    let (status, resp) = http_post(
        &client,
        format!("{base}/api/jobs/{scanned}/record-for-destruction"),
        Some(body),
    )
    .await;
    assert_eq!(status, 200, "{resp}");

    for (drive, job) in [
        (json!({ "asset_tag": "A-17" }), &open),
        (json!({ "serial": "S6ABNX0W123456" }), &open),
        (json!({ "serial": "SCAN-9" }), &scanned),
    ] {
        let (status, body) = http_post(
            &client,
            url.clone(),
            Some(group_body(json!([{ "serial": "NEW-1" }, drive]))),
        )
        .await;
        assert_eq!(status, 409, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains(job.as_str()),
            "the refusal should name job {job}, got {body}"
        );
    }
    assert_eq!(http_list_jobs(&client, &base).await.len(), 2);
}

#[tokio::test]
async fn the_statement_preview_refuses_blank_fields() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let (status, body) = http_post(
        &client,
        format!("{base}/api/destruction/statement"),
        Some(json!({
            "method": "pulverize", "equipment_id": "PUL-1", "location": "Bay 3",
            "full_name": "Ana Ruiz", "title": "",
        })),
    )
    .await;
    assert_eq!(status, 422, "{body}");
    assert!(body["error"].as_str().unwrap().contains("title"));
}

/// Wait until the Job's latest erasure has failed (the mock's SATA drive).
async fn http_wait_for_failed_erasure(
    client: &reqwest::Client,
    base: &str,
    job_id: &str,
) -> serde_json::Value {
    for _ in 0..200 {
        let job = http_get_job(client, base, job_id).await;
        let failed = job["activities"].as_array().is_some_and(|a| {
            a.iter()
                .any(|x| x["type"] == "erasure" && x["state"]["state"] == "failed")
        });
        if failed {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("job {job_id} never had a failed erasure");
}

#[tokio::test]
async fn a_device_record_reads_the_serial_from_the_erasure_snapshot_once_detached() {
    use wipe_engine::DeviceSimulator;
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let base = spawn_server_on(backend.clone()).await;
    let client = reqwest::Client::new();
    let job_id = http_create_tagged_job(&client, &base, "dev-sata-0", "A-9").await;
    let (status, _) = http_post(&client, format!("{base}/api/jobs/{job_id}/start"), None).await;
    assert_eq!(status, 200);
    http_wait_for_failed_erasure(&client, &base, &job_id).await;
    backend
        .detach(&wipe_common::DeviceId("dev-sata-0".into()))
        .unwrap();

    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("shred", http_op())),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(
        latest_destruction(&job)["attestation"]["identifiers"],
        json!({"asset_tag": "A-9", "serial": "203440FAILSIM", "source": "device"})
    );
}

#[tokio::test]
async fn the_api_catch_all_keeps_method_not_allowed() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, &base, "dev-nvme-0").await;
    let resp = client
        .get(format!("{base}/api/jobs/{job_id}/record-for-destruction"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 405);
}

#[tokio::test]
async fn a_scanned_groups_statement_is_the_previewed_one() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let (status, preview) = http_post(
        &client,
        format!("{base}/api/destruction/statement"),
        Some(json!({
            "method": "shred", "equipment_id": "SHR-2", "location": "Bay 3",
            "full_name": "Alice Erasure", "title": "Bench lead",
        })),
    )
    .await;
    assert_eq!(status, 200, "{preview}");
    let (status, body) = http_post(
        &client,
        format!("{base}/api/destruction/records"),
        Some(group_body(json!([{ "serial": "S1" }, { "serial": "S2" }]))),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    for id in body["job_ids"].as_array().unwrap() {
        let job = http_get_job(&client, &base, id.as_str().unwrap()).await;
        assert_eq!(
            latest_destruction(&job)["attestation"]["statement"],
            preview["statement"]
        );
    }
}

// --- Destruction runs: completion and concurrence (destruction-attestation story 3)

/// A server started with `policy`, plus its state so a test can reach in
/// (e.g. to remove a certificate the way an older run could lack one).
async fn spawn_server_with(policy: wipe_common::ConcurrencePolicy) -> (String, AppState) {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let state = AppState::new(backend, None, Arc::new(SigningKey::generate()))
        .with_concurrence_policy(policy);
    let app = router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

async fn http_get(client: &reqwest::Client, url: String) -> (u16, serde_json::Value) {
    let resp = client.get(url).send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(serde_json::Value::Null))
}

/// A Job with an erasure activity, recorded for destruction, whose
/// certificate has been signed: the only kind a run accepts until story 4.
async fn http_certified_pending_job(client: &reqwest::Client, base: &str, device: &str) -> String {
    let job_id = http_create_job(client, base, device).await;
    let (status, _) = http_post(client, format!("{base}/api/jobs/{job_id}/start"), None).await;
    assert_eq!(status, 200);
    let mut has_erasure = false;
    for _ in 0..200 {
        if count_activities(&http_get_job(client, base, &job_id).await, "erasure") > 0 {
            has_erasure = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(has_erasure, "job never gained an erasure activity");
    let (status, body) = http_post(
        client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("shred", http_op())),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    for _ in 0..200 {
        let (status, _) = http_get(client, format!("{base}/api/jobs/{job_id}/certificate")).await;
        if status == 200 {
            return job_id;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no certificate for job {job_id} at pending_destruction");
}

async fn http_assemble(
    client: &reqwest::Client,
    base: &str,
    job_ids: &[String],
) -> serde_json::Value {
    let (status, run) = http_post(
        client,
        format!("{base}/api/manifests"),
        Some(json!({ "assembled_by": http_op(), "job_ids": job_ids, "note": "SHR-2 run" })),
    )
    .await;
    assert_eq!(status, 200, "{run}");
    run
}

fn second_op() -> serde_json::Value {
    json!({ "id": "op-2", "display_name": "Ben", "email": "ben@example.com" })
}

/// A completion body (Design Notes): verified, attested, signed by op-1.
fn completion_body() -> serde_json::Value {
    json!({
        "remnants_inspected": true,
        "equipment_cycle_completed": true,
        "equipment_id": "SHR-2",
        "criterion": "particles ≤ 2 mm",
        "video_ref": "CAM-3 14:02",
        "operator": http_op(),
        "full_name": "Alice Erasure",
        "title": "Bench lead",
        "attest": true,
    })
}

fn concurrence_body(operator: serde_json::Value, full_name: &str) -> serde_json::Value {
    json!({ "operator": operator, "full_name": full_name, "title": "Quality lead", "attest": true })
}

async fn job_states(client: &reqwest::Client, base: &str, ids: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for id in ids {
        let job = http_get_job(client, base, id).await;
        out.push(job["state"]["state"].as_str().unwrap().to_owned());
    }
    out
}

async fn http_get_run(client: &reqwest::Client, base: &str, id: &str) -> serde_json::Value {
    let (status, run) = http_get(client, format!("{base}/api/manifests/{id}")).await;
    assert_eq!(status, 200, "{run}");
    run
}

#[tokio::test]
async fn completing_a_run_with_no_concurrence_destroys_every_job_and_keeps_its_certificate() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let (_, policy) = http_get(&client, format!("{base}/api/destruction/policy")).await;
    assert_eq!(policy, json!({ "concurrence_policy": "none" }));
    let (_, pk) = http_get(&client, format!("{base}/api/public_key")).await;
    let vk_bytes: [u8; 32] = B64
        .decode(pk["public_key_b64"].as_str().unwrap().as_bytes())
        .unwrap()
        .try_into()
        .unwrap();

    let mut ids = Vec::new();
    for device in ["dev-nvme-0", "dev-nvme-1", "dev-hdd-0"] {
        ids.push(http_certified_pending_job(&client, &base, device).await);
    }
    let mut before = Vec::new();
    for id in &ids {
        before.push(
            http_get(&client, format!("{base}/api/jobs/{id}/certificate"))
                .await
                .1,
        );
    }
    let run = http_assemble(&client, &base, &ids).await;
    assert_eq!(run["state"], "pending");
    assert_eq!(run["required_concurrence"], json!(null));
    assert!(run.get("supervisor").is_none() && run.get("signed_at").is_none());
    let run_id = run["id"].as_str().unwrap();

    let (status, preview) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/completion/statement"),
        Some(
            json!({ "equipment_id": "SHR-2", "criterion": "particles ≤ 2 mm",
                     "video_ref": "CAM-3 14:02" }),
        ),
    )
    .await;
    assert_eq!(status, 200, "{preview}");
    let mut body = completion_body();
    body["completed_at"] = preview["completed_at"].clone();
    let (status, done) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/completion"),
        Some(body),
    )
    .await;
    assert_eq!(status, 200, "{done}");

    let run = http_get_run(&client, &base, run_id).await;
    assert_eq!(run["state"], "completed");
    assert_eq!(run["completion"]["statement"], preview["statement"]);
    assert_eq!(run["completion"]["completed_at"], preview["completed_at"]);
    assert!(preview["statement"]
        .as_str()
        .unwrap()
        .contains(preview["completed_at"].as_str().unwrap()));
    assert_eq!(run["completion"]["verification"]["equipment_id"], "SHR-2");
    assert_eq!(run["completion"]["video_ref"], "CAM-3 14:02");
    assert_eq!(run["completion"]["signer"]["full_name"], "Alice Erasure");
    assert_eq!(run["concurrence"], json!(null));
    assert_eq!(job_states(&client, &base, &ids).await, ["destroyed"; 3]);

    tokio::time::sleep(Duration::from_millis(100)).await;
    for (id, before) in ids.iter().zip(before) {
        let (_, after) = http_get(&client, format!("{base}/api/jobs/{id}/certificate")).await;
        assert_eq!(after, before, "completion must not touch the certificate");
        assert_eq!(after["co_signatures"].as_array().map_or(0, Vec::len), 0);
        let parsed: wipe_cert::SignedCertificate = serde_json::from_value(after).unwrap();
        wipe_cert::verify(&parsed, &[VerifyingKey::from_bytes(&vk_bytes).unwrap()])
            .expect("certificate still verifies offline");
    }

    // A Destroyed Job cannot be aborted.
    let (status, body) =
        http_post(&client, format!("{base}/api/jobs/{}/abort", ids[0]), None).await;
    assert_eq!(status, 409, "{body}");
}

#[tokio::test]
async fn with_a_witness_policy_the_run_awaits_a_witness_before_any_job_is_destroyed() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::Witness).await;
    let client = reqwest::Client::new();
    let (_, policy) = http_get(&client, format!("{base}/api/destruction/policy")).await;
    assert_eq!(policy, json!({ "concurrence_policy": "witness" }));
    let ids = vec![
        http_certified_pending_job(&client, &base, "dev-nvme-0").await,
        http_certified_pending_job(&client, &base, "dev-nvme-1").await,
    ];
    let run = http_assemble(&client, &base, &ids).await;
    assert_eq!(run["required_concurrence"], "witness");
    let run_id = run["id"].as_str().unwrap();

    let (status, run) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(run["state"], "awaiting_concurrence");
    assert_eq!(
        job_states(&client, &base, &ids).await,
        ["pending_destruction"; 2]
    );

    let (status, preview) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/concurrence/statement"),
        Some(json!({ "full_name": "Ben Witness", "title": "Quality lead" })),
    )
    .await;
    assert_eq!(status, 200, "{preview}");
    let (status, run) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/concurrence"),
        Some(concurrence_body(second_op(), "Ben Witness")),
    )
    .await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(run["state"], "completed");
    assert_eq!(run["concurrence"]["kind"], "witness");
    assert_eq!(run["concurrence"]["statement"], preview["statement"]);
    assert!(preview["statement"]
        .as_str()
        .unwrap()
        .contains("witnessed the destruction"));
    assert_eq!(run["concurrence"]["manifest_id"], run_id);
    assert_eq!(job_states(&client, &base, &ids).await, ["destroyed"; 2]);
}

#[tokio::test]
async fn with_a_review_policy_the_concurrence_is_recorded_as_a_review() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::Review).await;
    let client = reqwest::Client::new();
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run = http_assemble(&client, &base, &ids).await;
    assert_eq!(run["required_concurrence"], "review");
    let run_id = run["id"].as_str().unwrap();
    let url = format!("{base}/api/manifests/{run_id}");
    let (status, _) = http_post(
        &client,
        format!("{url}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200);
    // The client cannot pick the kind: an extra `kind` is ignored.
    let mut body = concurrence_body(second_op(), "Ben Reviewer");
    body["kind"] = json!("witness");
    let (status, run) = http_post(&client, format!("{url}/concurrence"), Some(body)).await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(run["concurrence"]["kind"], "review");
    let statement = run["concurrence"]["statement"].as_str().unwrap();
    assert!(statement.contains("reviewed the destruction records and verification evidence"));
    assert!(!statement.contains("witnessed"));
    assert_eq!(job_states(&client, &base, &ids).await, ["destroyed"]);
}

#[tokio::test]
async fn the_completion_signer_cannot_concur_and_nothing_changes() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::Review).await;
    let client = reqwest::Client::new();
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = format!("{base}/api/manifests/{run_id}");
    let (status, _) = http_post(
        &client,
        format!("{url}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200);
    let before = http_get_run(&client, &base, &run_id).await;

    for body in [
        concurrence_body(http_op(), "Someone Else"),
        concurrence_body(second_op(), "  alice   ERASURE "),
    ] {
        let (status, resp) = http_post(&client, format!("{url}/concurrence"), Some(body)).await;
        assert_eq!(status, 422, "{resp}");
        assert!(
            resp["error"].as_str().unwrap().contains("someone other"),
            "{resp}"
        );
    }
    assert_eq!(http_get_run(&client, &base, &run_id).await, before);
    assert_eq!(before["state"], "awaiting_concurrence");
    assert_eq!(
        job_states(&client, &base, &ids).await,
        ["pending_destruction"]
    );
}

#[tokio::test]
async fn a_concurrence_not_needed_too_early_or_twice_is_409() {
    // Not needed: the run was assembled under policy `none`.
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = format!("{base}/api/manifests/{run_id}");
    let concur = concurrence_body(second_op(), "Ben Reviewer");
    let (status, body) =
        http_post(&client, format!("{url}/concurrence"), Some(concur.clone())).await;
    assert_eq!(status, 409, "{body}");
    let (status, body) = http_post(
        &client,
        format!("{url}/concurrence/statement"),
        Some(json!({ "full_name": "Ben Reviewer", "title": "Quality lead" })),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let (status, _) = http_post(
        &client,
        format!("{url}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) =
        http_post(&client, format!("{url}/concurrence"), Some(concur.clone())).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(
        http_get_run(&client, &base, &run_id).await["concurrence"],
        json!(null)
    );

    // Too early, then twice: policy `review`.
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::Review).await;
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = format!("{base}/api/manifests/{run_id}");
    let (status, body) =
        http_post(&client, format!("{url}/concurrence"), Some(concur.clone())).await;
    assert_eq!(status, 409, "{body}");
    let run = http_get_run(&client, &base, &run_id).await;
    assert_eq!(run["state"], "pending");
    assert_eq!(run["concurrence"], json!(null));
    assert_eq!(
        job_states(&client, &base, &ids).await,
        ["pending_destruction"]
    );

    let (status, _) = http_post(
        &client,
        format!("{url}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = http_post(&client, format!("{url}/concurrence"), Some(concur)).await;
    assert_eq!(status, 200);
    let done = http_get_run(&client, &base, &run_id).await;
    let (status, body) = http_post(
        &client,
        format!("{url}/concurrence"),
        Some(concurrence_body(
            json!({ "id": "op-3", "display_name": "Cy", "email": "cy@example.com" }),
            "Cy Dee",
        )),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(http_get_run(&client, &base, &run_id).await, done);
}

#[tokio::test]
async fn completing_a_run_twice_or_once_it_awaits_concurrence_is_409() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = format!("{base}/api/manifests/{run_id}/completion");
    let (first, _) = http_post(&client, url.clone(), Some(completion_body())).await;
    assert_eq!(first, 200);
    let done = http_get_run(&client, &base, &run_id).await;
    let (second, body) = http_post(&client, url, Some(completion_body())).await;
    assert_eq!(second, 409, "{body}");
    assert_eq!(http_get_run(&client, &base, &run_id).await, done);

    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::Witness).await;
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = format!("{base}/api/manifests/{run_id}/completion");
    let (first, _) = http_post(&client, url.clone(), Some(completion_body())).await;
    assert_eq!(first, 200);
    let awaiting = http_get_run(&client, &base, &run_id).await;
    let (second, body) = http_post(&client, url, Some(completion_body())).await;
    assert_eq!(second, 409, "{body}");
    assert_eq!(http_get_run(&client, &base, &run_id).await, awaiting);
    assert_eq!(
        job_states(&client, &base, &ids).await,
        ["pending_destruction"]
    );
}

#[tokio::test]
async fn an_unverified_or_unattested_completion_is_422_and_changes_nothing() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = http_get_run(&client, &base, &run_id).await;
    let url = format!("{base}/api/manifests/{run_id}/completion");

    let mut cases: Vec<(serde_json::Value, &str)> = Vec::new();
    for (field, value, needle) in [
        ("remnants_inspected", json!(false), "remnants"),
        ("equipment_cycle_completed", json!(false), "cycle"),
        ("criterion", json!(" "), "criterion"),
        ("equipment_id", json!(""), "equipment_id"),
        ("full_name", json!("  "), "full_name"),
        ("title", json!(""), "title"),
        ("attest", json!(false), "I attest"),
    ] {
        let mut body = completion_body();
        body[field] = value;
        cases.push((body, needle));
    }
    let mut missing = completion_body();
    missing.as_object_mut().unwrap().remove("attest");
    cases.push((missing, "I attest"));
    for (body, needle) in cases {
        let (status, resp) = http_post(&client, url.clone(), Some(body)).await;
        assert_eq!(status, 422, "{needle}: {resp}");
        assert!(
            resp["error"].as_str().unwrap().contains(needle),
            "{needle}: {resp}"
        );
    }
    assert_eq!(http_get_run(&client, &base, &run_id).await, before);
    assert_eq!(
        job_states(&client, &base, &ids).await,
        ["pending_destruction"]
    );
}

#[tokio::test]
async fn the_completion_time_defaults_to_now_and_is_range_checked() {
    use time::format_description::well_known::Rfc3339;
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run = http_assemble(&client, &base, &ids).await;
    let run_id = run["id"].as_str().unwrap().to_owned();
    let created_at =
        time::OffsetDateTime::parse(run["created_at"].as_str().unwrap(), &Rfc3339).unwrap();
    let url = format!("{base}/api/manifests/{run_id}");
    let fmt = |t: time::OffsetDateTime| t.format(&Rfc3339).unwrap();

    let future = fmt(time::OffsetDateTime::now_utc() + time::Duration::minutes(5));
    let early = fmt(created_at - time::Duration::hours(1));
    for at in [&future, &early] {
        let mut body = completion_body();
        body["completed_at"] = json!(at);
        let (status, resp) = http_post(&client, format!("{url}/completion"), Some(body)).await;
        assert_eq!(status, 422, "{at}: {resp}");
        assert!(
            resp["error"].as_str().unwrap().contains("out of range"),
            "{resp}"
        );
        let (status, resp) = http_post(
            &client,
            format!("{url}/completion/statement"),
            Some(json!({ "completed_at": at, "equipment_id": "SHR-2", "criterion": "c" })),
        )
        .await;
        assert_eq!(status, 422, "{at}: {resp}");
    }
    assert_eq!(
        http_get_run(&client, &base, &run_id).await["state"],
        "pending"
    );
    assert_eq!(
        job_states(&client, &base, &ids).await,
        ["pending_destruction"]
    );

    let before = time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .unwrap();
    let (status, run) = http_post(
        &client,
        format!("{url}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200, "{run}");
    let stored = run["completion"]["completed_at"].as_str().unwrap();
    let at = time::OffsetDateTime::parse(stored, &Rfc3339).unwrap();
    assert!(
        at >= before && at <= time::OffsetDateTime::now_utc(),
        "{stored}"
    );
    assert!(run["completion"]["statement"]
        .as_str()
        .unwrap()
        .contains(stored));
}

#[tokio::test]
async fn an_unknown_run_is_404_on_completion_concurrence_and_previews() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::Review).await;
    let client = reqwest::Client::new();
    let url = format!("{base}/api/manifests/{}", uuid::Uuid::new_v4());
    for (path, body) in [
        ("completion", completion_body()),
        ("concurrence", concurrence_body(second_op(), "Ben Reviewer")),
        (
            "completion/statement",
            json!({ "equipment_id": "SHR-2", "criterion": "c" }),
        ),
        (
            "concurrence/statement",
            json!({ "full_name": "Ben", "title": "Lead" }),
        ),
    ] {
        let (status, resp) = http_post(&client, format!("{url}/{path}"), Some(body)).await;
        assert_eq!(status, 404, "{path}: {resp}");
    }
}

#[tokio::test]
async fn the_statement_previews_refuse_blank_fields() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::Review).await;
    let client = reqwest::Client::new();
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = format!("{base}/api/manifests/{run_id}");
    for (path, body, needle) in [
        (
            "completion/statement",
            json!({ "equipment_id": "SHR-2", "criterion": " " }),
            "criterion",
        ),
        (
            "completion/statement",
            json!({ "criterion": "c" }),
            "equipment_id",
        ),
    ] {
        let (status, resp) = http_post(&client, format!("{url}/{path}"), Some(body)).await;
        assert_eq!(status, 422, "{path}: {resp}");
        assert!(resp["error"].as_str().unwrap().contains(needle), "{resp}");
    }
    // The concurrence preview is offered only once the run awaits it.
    let (status, _) = http_post(
        &client,
        format!("{url}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200);
    let (status, resp) = http_post(
        &client,
        format!("{url}/concurrence/statement"),
        Some(json!({ "full_name": "Ben", "title": "" })),
    )
    .await;
    assert_eq!(status, 422, "{resp}");
    assert!(resp["error"].as_str().unwrap().contains("title"), "{resp}");
}

#[tokio::test]
async fn assembling_a_job_with_no_certificate_is_409_naming_it() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let certified = http_certified_pending_job(&client, &base, "dev-nvme-1").await;
    // Recorded while Queued: no erasure activity, so no certificate.
    let bare = http_create_job(&client, &base, "dev-nvme-0").await;
    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{bare}/record-for-destruction"),
        Some(attested("disintegrate", http_op())),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (status, body) = http_post(
        &client,
        format!("{base}/api/manifests"),
        Some(json!({ "assembled_by": http_op(), "job_ids": [certified, bare] })),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let message = body["error"].as_str().unwrap();
    assert!(
        message.contains(&bare) && message.contains("no certificate"),
        "{body}"
    );
    assert!(!message.contains(&certified), "{body}");
    for id in [&certified, &bare] {
        assert!(http_get_job(&client, &base, id).await["manifest_id"].is_null());
    }
    let (_, runs) = http_get(&client, format!("{base}/api/manifests")).await;
    assert!(runs.as_array().unwrap().is_empty());
}

/// A run assembled before the assembly guard (or one whose certificate was
/// removed) must still never destroy an uncertified Job: completion and
/// concurrence refuse it, naming the Job, and change nothing.
#[tokio::test]
async fn a_run_member_without_a_certificate_blocks_completion_and_concurrence() {
    let (base, state) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let ids = vec![
        http_certified_pending_job(&client, &base, "dev-nvme-0").await,
        http_certified_pending_job(&client, &base, "dev-nvme-1").await,
    ];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let lost: uuid::Uuid = ids[1].parse().unwrap();
    state.certs.write().remove(&lost);
    let before = http_get_run(&client, &base, &run_id).await;
    let (status, body) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert!(body["error"].as_str().unwrap().contains(&ids[1]), "{body}");
    assert_eq!(http_get_run(&client, &base, &run_id).await, before);
    assert_eq!(
        job_states(&client, &base, &ids).await,
        ["pending_destruction"; 2]
    );

    let (base, state) = spawn_server_with(wipe_common::ConcurrencePolicy::Witness).await;
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = format!("{base}/api/manifests/{run_id}");
    let (status, _) = http_post(
        &client,
        format!("{url}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200);
    state
        .certs
        .write()
        .remove(&ids[0].parse::<uuid::Uuid>().unwrap());
    let before = http_get_run(&client, &base, &run_id).await;
    let (status, body) = http_post(
        &client,
        format!("{url}/concurrence"),
        Some(concurrence_body(second_op(), "Ben Witness")),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert!(body["error"].as_str().unwrap().contains(&ids[0]), "{body}");
    assert_eq!(http_get_run(&client, &base, &run_id).await, before);
    assert_eq!(
        job_states(&client, &base, &ids).await,
        ["pending_destruction"]
    );
}

#[tokio::test]
async fn the_removed_cosign_route_is_a_json_404() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, body) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/cosign"),
        Some(json!({ "supervisor": http_op() })),
    )
    .await;
    assert_eq!(status, 404, "{body}");
    assert!(body["error"].is_string(), "{body}");
    assert_eq!(
        http_get_run(&client, &base, &run_id).await["state"],
        "pending"
    );
    assert_eq!(
        job_states(&client, &base, &ids).await,
        ["pending_destruction"]
    );
}

/// A preview is refused (409) whenever the action it previews would be: a
/// completion preview once the run is no longer `pending`, a concurrence
/// preview before completion or after the run is `completed`.
#[tokio::test]
async fn previews_are_409_when_the_run_is_not_in_the_state_their_action_accepts() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::Review).await;
    let client = reqwest::Client::new();
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = format!("{base}/api/manifests/{run_id}");
    let completion_preview = json!({ "equipment_id": "SHR-2", "criterion": "c" });
    let concurrence_preview = json!({ "full_name": "Ben Reviewer", "title": "Quality lead" });

    // Pending: no concurrence preview yet.
    let (status, body) = http_post(
        &client,
        format!("{url}/concurrence/statement"),
        Some(concurrence_preview.clone()),
    )
    .await;
    assert_eq!(status, 409, "{body}");

    let (status, _) = http_post(
        &client,
        format!("{url}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200);
    // Awaiting concurrence: completion preview refused, concurrence preview allowed.
    let (status, body) = http_post(
        &client,
        format!("{url}/completion/statement"),
        Some(completion_preview.clone()),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let (status, body) = http_post(
        &client,
        format!("{url}/concurrence/statement"),
        Some(concurrence_preview.clone()),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    let (status, _) = http_post(
        &client,
        format!("{url}/concurrence"),
        Some(concurrence_body(second_op(), "Ben Reviewer")),
    )
    .await;
    assert_eq!(status, 200);
    // Completed: both previews refused.
    for (path, body) in [
        ("completion/statement", completion_preview),
        ("concurrence/statement", concurrence_preview),
    ] {
        let (status, resp) = http_post(&client, format!("{url}/{path}"), Some(body)).await;
        assert_eq!(status, 409, "{path}: {resp}");
    }
}

/// A completion time sent with a non-UTC offset is stored in UTC, so the
/// stored text appears verbatim in the statement.
#[tokio::test]
async fn a_completion_time_with_an_offset_is_stored_as_the_statement_prints_it() {
    use time::format_description::well_known::Rfc3339;
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let at = time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .unwrap()
        .to_offset(time::UtcOffset::from_hms(2, 0, 0).unwrap());
    let sent = at.format(&Rfc3339).unwrap();
    assert!(sent.ends_with("+02:00"), "{sent}");
    let mut body = completion_body();
    body["completed_at"] = json!(sent);
    let (status, run) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/completion"),
        Some(body),
    )
    .await;
    assert_eq!(status, 200, "{run}");
    let stored = run["completion"]["completed_at"].as_str().unwrap();
    assert!(stored.ends_with('Z'), "{stored}");
    assert_eq!(time::OffsetDateTime::parse(stored, &Rfc3339).unwrap(), at);
    assert!(
        run["completion"]["statement"]
            .as_str()
            .unwrap()
            .contains(stored),
        "{run}"
    );
}

/// A member Job that left pending destruction (here, aborted) blocks the
/// run's completion, naming it; nothing changes.
#[tokio::test]
async fn a_run_member_no_longer_pending_destruction_blocks_completion() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let ids = vec![
        http_certified_pending_job(&client, &base, "dev-nvme-0").await,
        http_certified_pending_job(&client, &base, "dev-nvme-1").await,
    ];
    let run_id = http_assemble(&client, &base, &ids).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, body) =
        http_post(&client, format!("{base}/api/jobs/{}/abort", ids[1]), None).await;
    assert_eq!(status, 200, "{body}");
    let before = http_get_run(&client, &base, &run_id).await;

    let (status, body) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert!(body["error"].as_str().unwrap().contains(&ids[1]), "{body}");
    assert_eq!(http_get_run(&client, &base, &run_id).await, before);
    assert_eq!(
        job_states(&client, &base, &ids).await,
        ["pending_destruction", "aborted"]
    );
}

/// The Tauri shell relies on `AppState`'s default policy being `none`.
#[tokio::test]
async fn the_default_concurrence_policy_is_none() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let (status, policy) = http_get(&client, format!("{base}/api/destruction/policy")).await;
    assert_eq!(status, 200);
    assert_eq!(policy, json!({ "concurrence_policy": "none" }));
    let ids = vec![http_certified_pending_job(&client, &base, "dev-nvme-0").await];
    let run = http_assemble(&client, &base, &ids).await;
    assert_eq!(run["required_concurrence"], json!(null));
}
