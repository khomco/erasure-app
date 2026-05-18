//! End-to-end test: spin up the Axum server with the mock backend, drive
//! it via HTTP, and verify a signed certificate is produced and can be
//! validated offline using the public key the server publishes.

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

    // Create a job.
    let create_resp: serde_json::Value = client
        .post(format!("{base}/api/jobs"))
        .json(&json!({
            "device_id": "dev-nvme-0",
            "classification": "high",
            "intent": "reuse",
            "verify": true,
            "verify_samples": 4,
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

    // Poll until completed.
    let mut completed = false;
    for _ in 0..200 {
        let job: serde_json::Value = client
            .get(format!("{base}/api/jobs/{job_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let state = job["state"]["state"].as_str().unwrap_or("");
        if state == "completed" {
            completed = true;
            break;
        }
        if state == "failed" || state == "aborted" {
            panic!("job ended in unexpected state: {state}, full job: {job}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(completed, "job did not complete in time");

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
    assert_eq!(signed["signature"]["algorithm"], "ed25519");

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
            "verify": true,
            "verify_samples": 1,
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
        if state == "aborted" || state == "failed" {
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
