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

#[tokio::test]
async fn destroy_path_via_manifest_cosign_produces_two_signatures() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();

    // Create + start a Job we'll record for destruction.
    let create_resp: serde_json::Value = client
        .post(format!("{base}/api/jobs"))
        .json(&json!({
            "device_id": "dev-nvme-0",
            "classification": "high",
            "intent": "destroy",
            "operator": {
                "id": "op-1",
                "display_name": "Alice",
                "email": "alice@example.com"
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

    // Wait briefly for the Job to be in_progress, then immediately record it for destruction.
    // We don't need the erasure to complete — the model allows recording for destruction
    // at any non-terminal point.
    let mut started = false;
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
        let has_erasure = job["activities"]
            .as_array()
            .map(|a| a.iter().any(|x| x["type"] == "erasure"))
            .unwrap_or(false);
        if state == "in_progress" && has_erasure {
            started = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        started,
        "job did not reach in_progress with an erasure activity"
    );

    // Record for destruction.
    let resp = client
        .post(format!("{base}/api/jobs/{job_id}/record-for-destruction"))
        .json(&{
            let mut body = attested("disintegrate", http_op());
            body["notes"] = json!("drive bricked mid-wipe");
            body
        })
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "record for destruction failed: {}",
        resp.status()
    );

    // Job should now be in pending_destruction and a cert should be generated.
    let mut pending = false;
    for _ in 0..50 {
        let job: serde_json::Value = client
            .get(format!("{base}/api/jobs/{job_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if job["state"]["state"] == "pending_destruction" {
            pending = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(pending, "job did not reach pending_destruction");

    // Cert should be available with disposition Destroyed.
    let mut signed_json: Option<serde_json::Value> = None;
    for _ in 0..50 {
        let resp = client
            .get(format!("{base}/api/jobs/{job_id}/certificate"))
            .send()
            .await
            .unwrap();
        if resp.status().is_success() {
            signed_json = Some(resp.json().await.unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let signed_json =
        signed_json.expect("destroy-path cert should be generated at pending_destruction");
    assert_eq!(signed_json["certificate"]["disposition"], "destroyed");
    // `co_signatures` is omitted from JSON when empty (skip_serializing_if).
    let pre_cosigs = signed_json["co_signatures"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(pre_cosigs, 0, "should have no cosignatures before manifest");

    // Build a manifest and supervisor-cosign it.
    let manifest: serde_json::Value = client
        .post(format!("{base}/api/manifests"))
        .json(&json!({
            "assembled_by": {
                "id": "op-1",
                "display_name": "Alice",
                "email": "alice@example.com"
            },
            "job_ids": [job_id],
            "note": "shredder run 2026-05-18"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let manifest_id = manifest["id"].as_str().unwrap();

    // Assembly stamps the manifest on the Job and on its DestructionEvent,
    // without changing the Job's state or its stored certificate.
    let job: serde_json::Value = client
        .get(format!("{base}/api/jobs/{job_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(job["state"]["state"], "pending_destruction");
    assert_eq!(job["manifest_id"], manifest_id);
    assert_eq!(latest_destruction(&job)["manifest_ref"], manifest_id);
    let after_link: serde_json::Value = client
        .get(format!("{base}/api/jobs/{job_id}/certificate"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after_link["signature"], signed_json["signature"]);

    let cosign_resp = client
        .post(format!("{base}/api/manifests/{manifest_id}/cosign"))
        .json(&json!({
            "supervisor": {
                "id": "sup-1",
                "display_name": "Sandra Supervisor",
                "email": "sandra@example.com"
            }
        }))
        .send()
        .await
        .unwrap();
    assert!(cosign_resp.status().is_success());

    // Cert should now carry a supervisor co-signature; Job should be Destroyed.
    let signed_json: serde_json::Value = client
        .get(format!("{base}/api/jobs/{job_id}/certificate"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cosigs = signed_json["co_signatures"].as_array().unwrap();
    assert_eq!(cosigs.len(), 1);
    assert_eq!(cosigs[0]["role"], "supervisor");
    assert_eq!(cosigs[0]["manifest_ref"], manifest_id);

    let job: serde_json::Value = client
        .get(format!("{base}/api/jobs/{job_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(job["state"]["state"], "destroyed");
    assert_eq!(job["manifest_id"], manifest_id);
    assert_eq!(latest_destruction(&job)["manifest_ref"], manifest_id);
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

/// A Job recorded for destruction before any erasure activity exists has nothing for the
/// cert generator to certify. Co-signing must refuse to finalise it as
/// Destroyed — an uncertified destruction is the failure this product
/// exists to prevent — and the refusal must leave the Job untouched.
#[tokio::test]
async fn cosign_refuses_to_destroy_a_job_with_no_certificate() {
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

    // Record while still Queued: no erasure activity, so no cert.
    let resp = client
        .post(format!("{base}/api/jobs/{job_id}/record-for-destruction"))
        .json(&attested("disintegrate", op.clone()))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    tokio::time::sleep(Duration::from_millis(200)).await;

    let manifest: serde_json::Value = client
        .post(format!("{base}/api/manifests"))
        .json(&json!({ "assembled_by": op, "job_ids": [job_id] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let manifest_id = manifest["id"].as_str().unwrap().to_string();

    let cosign = client
        .post(format!("{base}/api/manifests/{manifest_id}/cosign"))
        .json(&json!({ "supervisor": op }))
        .send()
        .await
        .unwrap();
    assert_eq!(cosign.status(), 409);
    assert!(cosign.text().await.unwrap().contains("has no certificate"));

    let job: serde_json::Value = client
        .get(format!("{base}/api/jobs/{job_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(job["state"]["state"], "pending_destruction");

    let m: serde_json::Value = client
        .get(format!("{base}/api/manifests/{manifest_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(m["state"], "pending");
}

/// One Job, one live manifest: a second manifest would let a second
/// co-sign append another supervisor signature to an already-final cert.
#[tokio::test]
async fn a_job_cannot_be_on_two_manifests() {
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
        .post(format!("{base}/api/jobs/{job_id}/record-for-destruction"))
        .json(&attested("disintegrate", op.clone()))
        .send()
        .await
        .unwrap();

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

/// Once a Job is on a manifest, the record the supervisor will co-sign is
/// fixed: recording it again is refused, appends no DestructionEvent and
/// leaves the stored certificate in place.
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
/// activity has `manifest_ref` null while the live Job carries it. Over
/// HTTP the generator usually signs before the manifest arrives, so this
/// passes even without the strip; the deterministic proof is
/// `certificate_signed_after_linking_still_omits_the_manifest`.
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
    // Assemble at once, without waiting for the certificate.
    let manifest: serde_json::Value = client
        .post(format!("{base}/api/manifests"))
        .json(&json!({ "assembled_by": op, "job_ids": [job_id] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let manifest_id = manifest["id"]
        .as_str()
        .expect("manifest stored")
        .to_string();

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

#[tokio::test]
async fn cosigning_an_unknown_manifest_is_404() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let unknown = uuid::Uuid::new_v4();
    let (status, body) = http_post(
        &client,
        format!("{base}/api/manifests/{unknown}/cosign"),
        Some(json!({ "supervisor": http_op() })),
    )
    .await;
    assert_eq!(status, 404, "{body}");
}

#[tokio::test]
async fn cosigning_a_signed_manifest_again_is_409_and_adds_no_cosignature() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, &base, "dev-nvme-0").await;
    let (status, _) = http_post(&client, format!("{base}/api/jobs/{job_id}/start"), None).await;
    assert_eq!(status, 200);
    // The certificate needs an erasure activity to snapshot the device.
    let mut has_erasure = false;
    for _ in 0..200 {
        if count_activities(&http_get_job(&client, &base, &job_id).await, "erasure") > 0 {
            has_erasure = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(has_erasure, "job never gained an erasure activity");
    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("shred", http_op())),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let mut has_cert = false;
    for _ in 0..200 {
        let resp = client
            .get(format!("{base}/api/jobs/{job_id}/certificate"))
            .send()
            .await
            .unwrap();
        if resp.status().is_success() {
            has_cert = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(has_cert, "certificate generated at pending_destruction");

    let (status, manifest) = http_post(
        &client,
        format!("{base}/api/manifests"),
        Some(json!({ "assembled_by": http_op(), "job_ids": [job_id] })),
    )
    .await;
    assert_eq!(status, 200, "{manifest}");
    let manifest_id = manifest["id"].as_str().unwrap().to_string();
    let cosign = json!({ "supervisor": http_op() });
    let url = format!("{base}/api/manifests/{manifest_id}/cosign");
    let (first, _) = http_post(&client, url.clone(), Some(cosign.clone())).await;
    let (second, body) = http_post(&client, url, Some(cosign)).await;
    assert_eq!(first, 200);
    assert_eq!(second, 409, "{body}");

    let cert: serde_json::Value = client
        .get(format!("{base}/api/jobs/{job_id}/certificate"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cert["co_signatures"].as_array().unwrap().len(), 1);
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "destroyed");

    // A Destroyed Job cannot be aborted either.
    let (status, body) = http_post(&client, format!("{base}/api/jobs/{job_id}/abort"), None).await;
    assert_eq!(status, 409, "{body}");
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "destroyed");
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
