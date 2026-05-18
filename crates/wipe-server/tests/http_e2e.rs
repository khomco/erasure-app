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
    assert_eq!(signed["certificate"]["operator"]["email"], "alice@example.com");
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

    // Create + start a Job we'll escalate to destruction.
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

    // Wait briefly for the Job to be in_progress, then immediately escalate.
    // We don't need the erasure to complete — the model allows escalation
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
    assert!(started, "job did not reach in_progress with an erasure activity");

    // Escalate to destroy.
    let resp = client
        .post(format!("{base}/api/jobs/{job_id}/escalate-to-destroy"))
        .json(&json!({
            "method": "disintegrate",
            "operator": {
                "id": "op-1",
                "display_name": "Alice",
                "email": "alice@example.com"
            },
            "notes": "drive bricked mid-wipe"
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "escalate failed: {}", resp.status());

    // Job should now be in pending_co_sign and a cert should be generated.
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
        if job["state"]["state"] == "pending_co_sign" {
            pending = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(pending, "job did not reach pending_co_sign");

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
    let signed_json = signed_json.expect("destroy-path cert should be generated at pending_co_sign");
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
}
