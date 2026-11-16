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
    assert_eq!(signed["certificate"]["cert_format_version"], 3);
    assert!(signed["certificate"].get("destruction").is_none());
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
    let job_id = http_pending_job(&client, &base, "dev-nvme-0").await;

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
/// covers is fixed: recording it again is refused and appends no
/// DestructionEvent.
#[tokio::test]
async fn re_recording_a_job_on_a_manifest_is_refused() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let job_id = http_pending_job(&client, &base, "dev-nvme-0").await;
    let run = http_assemble(&client, &base, std::slice::from_ref(&job_id)).await;
    let manifest_id = run["id"].as_str().unwrap();

    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("disintegrate", http_op())),
    )
    .await;
    assert_eq!(status, 409, "re-recording a linked job must be a conflict");
    let message = body["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("destruction manifest") && message.contains(manifest_id),
        "refusal should name the destruction manifest, got {body}"
    );
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(count_activities(&job, "destruction"), 1);
    assert_eq!(latest_destruction(&job)["method"], "shred");
    assert_eq!(latest_destruction(&job)["manifest_ref"], manifest_id);
}

/// Recording for destruction, and assembling a run, sign nothing
/// (ADR-0011 §2): the certificate waits for `Destroyed`.
#[tokio::test]
async fn recording_for_destruction_signs_no_certificate() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let erased_first = http_pending_job(&client, &base, "dev-nvme-0").await;
    let (status, body) = http_post(
        &client,
        format!("{base}/api/destruction/records"),
        Some(group_body(json!([{ "serial": "DEAD-1" }]))),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let scanned = body["job_ids"][0].as_str().unwrap().to_owned();
    let ids = [erased_first, scanned];
    // Give a (wrongly triggered) generator time to sign.
    tokio::time::sleep(Duration::from_millis(150)).await;
    for id in &ids {
        let (status, _) = http_get(&client, format!("{base}/api/jobs/{id}/certificate")).await;
        assert_eq!(status, 404, "job {id} was certified at pending_destruction");
    }
    http_assemble(&client, &base, &ids).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    for id in &ids {
        let (status, _) = http_get(&client, format!("{base}/api/jobs/{id}/certificate")).await;
        assert_eq!(status, 404, "job {id} was certified at assembly");
    }
}

/// The destroyed certificate signs the run link: its Destruction activity's
/// `manifest_ref` and its `destruction.manifest_id` both name the run.
#[tokio::test]
async fn a_destroyed_certificate_signs_the_run_link() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let job_id = http_pending_job(&client, &base, "dev-nvme-0").await;
    let run = http_assemble(&client, &base, std::slice::from_ref(&job_id)).await;
    let run_id = run["id"].as_str().unwrap();
    let (status, done) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200, "{done}");

    let signed = http_wait_for_certificate(&client, &base, &job_id).await;
    let cert = &signed["certificate"];
    assert_eq!(cert["cert_format_version"], 3);
    assert_eq!(cert["disposition"], "destroyed");
    assert_eq!(latest_destruction(cert)["manifest_ref"], run_id);
    assert_eq!(cert["destruction"]["manifest_id"], run_id);
    assert_eq!(
        cert["destruction"]["destruction_event_id"],
        latest_destruction(cert)["id"]
    );
    assert_eq!(cert["destruction"]["concurrence"], json!(null));
    assert_eq!(cert["destruction"]["completion"], done["completion"]);
    assert_eq!(
        cert["sanitization"]["method"],
        json!({ "kind": "destroy", "method": "shred" })
    );
    assert_eq!(cert["sanitization"]["category"], "destroy");
    assert_eq!(
        cert["media_status"],
        json!({ "operational": false, "damaged": true, "notes": null })
    );
    assert!(
        cert["device"].is_object(),
        "an erased-first drive keeps its device"
    );
    let parsed: wipe_cert::SignedCertificate = serde_json::from_value(signed).unwrap();
    wipe_cert::verify(&parsed, &[http_public_key(&client, &base).await]).unwrap();
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
/// (e.g. to store a run the assembly route would never build).
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

/// A Job with an erasure activity, recorded for destruction (by Shred).
async fn http_pending_job(client: &reqwest::Client, base: &str, device: &str) -> String {
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
    job_id
}

/// The Job's certificate, once the generator has signed it.
async fn http_wait_for_certificate(
    client: &reqwest::Client,
    base: &str,
    job_id: &str,
) -> serde_json::Value {
    for _ in 0..200 {
        let (status, body) =
            http_get(client, format!("{base}/api/jobs/{job_id}/certificate")).await;
        if status == 200 {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no certificate for job {job_id}");
}

async fn http_public_key(client: &reqwest::Client, base: &str) -> VerifyingKey {
    let (_, pk) = http_get(client, format!("{base}/api/public_key")).await;
    let bytes: [u8; 32] = B64
        .decode(pk["public_key_b64"].as_str().unwrap().as_bytes())
        .unwrap()
        .try_into()
        .unwrap();
    VerifyingKey::from_bytes(&bytes).unwrap()
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
async fn completing_a_run_with_no_concurrence_destroys_every_job_and_certifies_each_once() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let (_, policy) = http_get(&client, format!("{base}/api/destruction/policy")).await;
    assert_eq!(policy, json!({ "concurrence_policy": "none" }));
    let vk = http_public_key(&client, &base).await;

    let mut ids = Vec::new();
    for device in ["dev-nvme-0", "dev-nvme-1", "dev-hdd-0"] {
        ids.push(http_pending_job(&client, &base, device).await);
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

    for id in &ids {
        let signed = http_wait_for_certificate(&client, &base, id).await;
        let cert = &signed["certificate"];
        assert_eq!(cert["job_id"], id.as_str());
        assert_eq!(cert["cert_format_version"], 3);
        assert_eq!(cert["evaluation"], true);
        assert_eq!(cert["disposition"], "destroyed");
        assert_eq!(cert["destruction"]["completion"], run["completion"]);
        assert_eq!(cert["destruction"]["manifest_id"], run_id);
        assert_eq!(signed["co_signatures"].as_array().map_or(0, Vec::len), 0);
        let parsed: wipe_cert::SignedCertificate = serde_json::from_value(signed.clone()).unwrap();
        wipe_cert::verify(&parsed, std::slice::from_ref(&vk))
            .expect("certificate verifies offline with the completion inside");
        // Signed once: a later read returns the same signature.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (_, again) = http_get(&client, format!("{base}/api/jobs/{id}/certificate")).await;
        assert_eq!(again["signature"], signed["signature"]);
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
        http_pending_job(&client, &base, "dev-nvme-0").await,
        http_pending_job(&client, &base, "dev-nvme-1").await,
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
    tokio::time::sleep(Duration::from_millis(100)).await;
    for id in &ids {
        let (status, _) = http_get(&client, format!("{base}/api/jobs/{id}/certificate")).await;
        assert_eq!(status, 404, "no certificate before the concurrence");
    }

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
    let vk = http_public_key(&client, &base).await;
    for id in &ids {
        let signed = http_wait_for_certificate(&client, &base, id).await;
        let rec = &signed["certificate"]["destruction"];
        assert_eq!(rec["required_concurrence"], "witness");
        assert_eq!(rec["concurrence"]["kind"], "witness");
        assert_eq!(rec["concurrence"], run["concurrence"]);
        let parsed: wipe_cert::SignedCertificate = serde_json::from_value(signed).unwrap();
        wipe_cert::verify(&parsed, std::slice::from_ref(&vk)).unwrap();
    }
}

#[tokio::test]
async fn with_a_review_policy_the_concurrence_is_recorded_as_a_review() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::Review).await;
    let client = reqwest::Client::new();
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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

/// CAP-4's success signal: drives that were never erased (three scanned dead
/// drives, and a queued Job recorded from its device) assemble onto a run,
/// complete, and each gets one certificate that verifies, carrying the
/// recorded identifiers instead of a device snapshot.
/// The lease counts on the destroy path: erased drives destroyed by a run
/// count once each as evaluation erasures (the default station is
/// unlicensed); drives destroyed with no erasure attempt count nothing.
#[tokio::test]
async fn a_run_of_erased_drives_counts_one_evaluation_erasure_per_drive() {
    let (base, state) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let mut ids = Vec::new();
    for device in ["dev-nvme-0", "dev-nvme-1", "dev-hdd-0"] {
        ids.push(http_pending_job(&client, &base, device).await);
    }
    let run = http_assemble(&client, &base, &ids).await;
    let run_id = run["id"].as_str().unwrap();
    let (status, done) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200, "{done}");
    for id in &ids {
        let signed = http_wait_for_certificate(&client, &base, id).await;
        assert_eq!(signed["certificate"]["disposition"], "destroyed");
    }
    // Give a (wrongly repeated) count time to land.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let lease = state.license().lease_state();
    assert_eq!(lease.evaluation_erasures, ids.len() as u64);
    assert_eq!(lease.erasures_used, 0);
}

#[tokio::test]
async fn a_run_of_never_erased_drives_counts_nothing() {
    let (base, state) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let (status, body) = http_post(
        &client,
        format!("{base}/api/destruction/records"),
        Some(group_body(
            json!([{ "serial": "DEAD-1" }, { "serial": "DEAD-2" }]),
        )),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let ids: Vec<String> = body["job_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let run = http_assemble(&client, &base, &ids).await;
    let run_id = run["id"].as_str().unwrap();
    let (status, done) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200, "{done}");
    for id in &ids {
        http_wait_for_certificate(&client, &base, id).await;
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    let lease = state.license().lease_state();
    assert_eq!(lease.evaluation_erasures, 0);
    assert_eq!(lease.erasures_used, 0);
}

#[tokio::test]
async fn never_erased_jobs_assemble_complete_and_get_verifying_certificates() {
    let (base, _) = spawn_server_with(wipe_common::ConcurrencePolicy::None).await;
    let client = reqwest::Client::new();
    let (status, body) = http_post(
        &client,
        format!("{base}/api/destruction/records"),
        Some(group_body(json!([
            { "asset_tag": "A-1", "serial": "DEAD-1" },
            { "serial": "DEAD-2" },
            { "asset_tag": "A-3" },
        ]))),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let mut ids: Vec<String> = body["job_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let queued = http_create_job(&client, &base, "dev-nvme-1").await;
    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{queued}/record-for-destruction"),
        Some(attested("crush", http_op())),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    ids.push(queued);

    let run = http_assemble(&client, &base, &ids).await;
    let run_id = run["id"].as_str().unwrap();
    let (status, done) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/completion"),
        Some(completion_body()),
    )
    .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(job_states(&client, &base, &ids).await, ["destroyed"; 4]);

    let vk = http_public_key(&client, &base).await;
    for (i, id) in ids.iter().enumerate() {
        let signed = http_wait_for_certificate(&client, &base, id).await;
        let cert = &signed["certificate"];
        assert!(cert.get("device").is_none(), "{cert}");
        assert_eq!(cert["evaluation"], true);
        assert!(cert.get("capabilities_snapshot").is_none());
        assert_eq!(cert["validation"]["media_class"], "unknown");
        let source = &latest_destruction(cert)["attestation"]["identifiers"]["source"];
        let rec = &cert["destruction"];
        if i < 3 {
            assert_eq!(source, "scanned");
            assert_eq!(rec["standard_label"], "NIST SP 800-88 Destroy");
            assert_eq!(rec["caution"], wipe_common::SHRED_PULVERIZE_CAUTION);
        } else {
            assert_eq!(source, "device");
            assert_eq!(rec["standard_label"], "physical damage — not NIST Destroy");
            assert!(cert["sanitization"].get("category").is_none());
        }
        assert_eq!(
            cert["started_at"],
            http_get_job(&client, &base, id).await["created_at"]
        );
        let parsed: wipe_cert::SignedCertificate = serde_json::from_value(signed.clone()).unwrap();
        wipe_cert::verify(&parsed, std::slice::from_ref(&vk)).unwrap();
    }
}

/// A run whose member cannot be certified (here, a run stored without
/// linking its Job, which the assembly route never does) must never destroy
/// it: completion and concurrence refuse with 409 naming the Job, and
/// change nothing.
#[tokio::test]
async fn an_uncertifiable_member_blocks_completion_and_concurrence() {
    for (policy, required) in [
        (wipe_common::ConcurrencePolicy::None, None),
        (
            wipe_common::ConcurrencePolicy::Witness,
            Some(wipe_common::ConcurrenceKind::Witness),
        ),
    ] {
        let (base, state) = spawn_server_with(policy).await;
        let client = reqwest::Client::new();
        let linked = http_pending_job(&client, &base, "dev-nvme-0").await;
        let unlinked = http_pending_job(&client, &base, "dev-nvme-1").await;
        let ids = vec![linked.clone(), unlinked.clone()];
        let run_id = http_assemble(&client, &base, std::slice::from_ref(&linked)).await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        // Add the unlinked Job to the stored run behind the route's back.
        {
            let mut runs = state.manifests.write();
            let run = runs.get_mut(&run_id.parse().unwrap()).unwrap();
            run.job_ids.push(unlinked.parse().unwrap());
            assert_eq!(run.required_concurrence, required);
        }
        let url = format!("{base}/api/manifests/{run_id}");
        let before = http_get_run(&client, &base, &run_id).await;
        let (status, body) = if required.is_none() {
            http_post(
                &client,
                format!("{url}/completion"),
                Some(completion_body()),
            )
            .await
        } else {
            let (status, _) = http_post(
                &client,
                format!("{url}/completion"),
                Some(completion_body()),
            )
            .await;
            assert_eq!(status, 200, "completion alone destroys nothing");
            let before = http_get_run(&client, &base, &run_id).await;
            let refused = http_post(
                &client,
                format!("{url}/concurrence"),
                Some(concurrence_body(second_op(), "Ben Witness")),
            )
            .await;
            assert_eq!(http_get_run(&client, &base, &run_id).await, before);
            refused
        };
        assert_eq!(status, 409, "{body}");
        let message = body["error"].as_str().unwrap();
        assert!(message.contains(&unlinked), "{body}");
        assert!(!message.contains(&linked), "{body}");
        if required.is_none() {
            assert_eq!(http_get_run(&client, &base, &run_id).await, before);
        }
        assert_eq!(
            job_states(&client, &base, &ids).await,
            ["pending_destruction"; 2]
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        for id in &ids {
            let (status, _) = http_get(&client, format!("{base}/api/jobs/{id}/certificate")).await;
            assert_eq!(status, 404);
        }
    }
}

#[tokio::test]
async fn the_removed_cosign_route_is_a_json_404() {
    let base = spawn_server().await;
    let client = reqwest::Client::new();
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
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
        http_pending_job(&client, &base, "dev-nvme-0").await,
        http_pending_job(&client, &base, "dev-nvme-1").await,
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
    let ids = vec![http_pending_job(&client, &base, "dev-nvme-0").await];
    let run = http_assemble(&client, &base, &ids).await;
    assert_eq!(run["required_concurrence"], json!(null));
}

// --- GET /api/license (licence-visibility story 1) ---------------------------

async fn spawn_server_with_license(license: Arc<wipe_server::LicenseContext>) -> String {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let state =
        AppState::new(backend, None, Arc::new(SigningKey::generate())).with_license(license);
    let app = router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn license_chain(not_after: time::OffsetDateTime) -> wipe_license::AttestationChain {
    let now = time::OffsetDateTime::now_utc();
    let ent = wipe_license::Entitlements {
        customer_id: "cust-42".into(),
        customer_name: "Acme Recycling".into(),
        quota: wipe_license::Quota::Unlimited,
        scope: wipe_license::Scope::Site {
            site_id: "s".into(),
        },
        not_before: now - time::Duration::days(60),
        not_after,
        features: vec![],
        allowed_methods: wipe_license::AllowedMethods::All,
        machine_binding: None,
    };
    let cert = wipe_license::VendorRoot::generate()
        .issue("lic-1", "ed25519:instance-key", ent, now)
        .unwrap();
    wipe_license::AttestationChain::new(cert)
}

fn licensed_ctx(not_after: time::OffsetDateTime) -> Arc<wipe_server::LicenseContext> {
    Arc::new(wipe_server::LicenseContext::licensed(
        "station-1",
        license_chain(not_after),
        wipe_license::LeaseState::new(time::OffsetDateTime::now_utc()),
    ))
}

/// A one-drive start on the simulated backend, which is never refused.
fn sim_start() -> wipe_server::StartRequest {
    wipe_server::StartRequest {
        method: None,
        drives: 1,
        simulated: true,
    }
}

async fn get_license(base: &str) -> serde_json::Value {
    let (status, body) = http_get(&reqwest::Client::new(), format!("{base}/api/license")).await;
    assert_eq!(status, 200, "{body}");
    body
}

fn license_tmpdir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wipestation-license-route-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn license_route_on_an_unlicensed_station() {
    let base = spawn_server().await;
    let view = get_license(&base).await;
    let mut keys: Vec<&str> = view
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "customer",
            "fingerprint",
            "instance_public_key_id",
            "new_jobs_refused",
            "not_after",
            "operator_message",
            "simulated_backend",
            "status"
        ]
    );
    assert_eq!(view["status"], json!({ "status": "unlicensed" }));
    assert_eq!(
        view["operator_message"],
        wipe_license::LeaseStatus::Unlicensed.operator_message()
    );
    for key in [
        "customer",
        "not_after",
        "instance_public_key_id",
        "new_jobs_refused",
    ] {
        assert_eq!(view[key], json!(null), "{key}");
    }
    // The mock backend is simulated: nothing is refused (ADR-0013 §2).
    assert_eq!(view["simulated_backend"], true);
    assert_eq!(
        view["fingerprint"],
        wipe_license::machine_fingerprint("standalone")
    );
}

#[tokio::test]
async fn license_route_on_a_valid_licence() {
    let not_after = time::OffsetDateTime::now_utc() + time::Duration::days(30);
    let ctx = licensed_ctx(not_after);
    let base = spawn_server_with_license(ctx).await;
    let view = get_license(&base).await;
    assert_eq!(view["status"]["status"], "valid");
    assert!(
        view["status"]["days_remaining"].as_i64().unwrap() >= 29,
        "{view}"
    );
    assert!(view["status"].get("remaining_erasures").is_none(), "{view}");
    assert_eq!(
        view["customer"],
        json!({ "id": "cust-42", "name": "Acme Recycling" })
    );
    let parsed = time::OffsetDateTime::parse(
        view["not_after"].as_str().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert_eq!(parsed, not_after);
    assert_eq!(view["instance_public_key_id"], "ed25519:instance-key");
    assert!(view.get("lapse_policy").is_none(), "{view}");
    assert_eq!(
        view["fingerprint"],
        wipe_license::machine_fingerprint("station-1")
    );
    assert!(view["operator_message"]
        .as_str()
        .unwrap()
        .starts_with("Licensed - unlimited"));
}

#[tokio::test]
async fn license_route_on_an_expired_licence() {
    let not_after = time::OffsetDateTime::now_utc() - time::Duration::days(3);
    let ctx = licensed_ctx(not_after);
    let base = spawn_server_with_license(ctx).await;
    let view = get_license(&base).await;
    assert_eq!(view["status"]["status"], "expired");
    assert!(view["status"]["not_after"].is_string(), "{view}");
    assert!(view["not_after"].is_string(), "{view}");
    assert!(
        view["operator_message"]
            .as_str()
            .unwrap()
            .contains(&not_after.date().to_string()),
        "{view}"
    );
}

#[tokio::test]
async fn license_route_reports_unreadable_lease_state() {
    let dir = license_tmpdir("corrupt");
    let path = dir.join("lease-state.json");
    std::fs::write(&path, b"not json").unwrap();
    let ctx = Arc::new(wipe_server::LicenseContext::open(
        "station-1",
        Some(license_chain(
            time::OffsetDateTime::now_utc() + time::Duration::days(30),
        )),
        Arc::new(wipe_server::LocalFileLeaseStore::new(path.clone())),
        time::OffsetDateTime::now_utc(),
    ));
    let base = spawn_server_with_license(ctx).await;
    let view = get_license(&base).await;
    assert_eq!(view["status"]["status"], "lease_state_unavailable");
    assert!(
        view["status"]["detail"]
            .as_str()
            .unwrap()
            .contains(&path.display().to_string()),
        "{view}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"not json");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn license_route_never_writes_lease_state_or_moves_the_watermark() {
    let dir = license_tmpdir("no-write");
    let path = dir.join("lease-state.json");
    let start = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    let ctx = Arc::new(wipe_server::LicenseContext::open(
        "station-1",
        Some(license_chain(
            time::OffsetDateTime::now_utc() + time::Duration::days(30),
        )),
        Arc::new(wipe_server::LocalFileLeaseStore::new(path.clone())),
        start,
    ));
    let base = spawn_server_with_license(ctx.clone()).await;

    // No file yet: reads must not create one.
    get_license(&base).await;
    get_license(&base).await;
    assert!(!path.exists(), "GET /api/license created the lease file");
    assert_eq!(ctx.lease_state().time_watermark, start);

    // A file a start decision wrote: reads must leave it byte-identical.
    let decision = ctx.decide_start(&sim_start(), start).unwrap();
    assert!(!decision.evaluation);
    let saved = std::fs::read(&path).unwrap();
    let before = ctx.lease_state();
    for _ in 0..3 {
        assert_eq!(get_license(&base).await["status"]["status"], "valid");
    }
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    assert_eq!(ctx.lease_state(), before);
    assert_eq!(ctx.lease_state().time_watermark, start);
    assert_eq!(ctx.lease_state().erasures_used, 0);

    // And the next start decision is the one it would have been anyway.
    assert!(
        !ctx.decide_start(&sim_start(), time::OffsetDateTime::now_utc())
            .unwrap()
            .evaluation
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A lease store whose saves always fail.
struct FailingSaveStore;

impl wipe_server::LeaseStore for FailingSaveStore {
    fn tier(&self) -> wipe_server::StoreTier {
        wipe_server::StoreTier::LocalFile
    }
    fn location(&self) -> String {
        "failing-save".into()
    }
    fn detail(&self) -> String {
        "failing-save".into()
    }
    fn load(&self) -> Result<Option<wipe_license::LeaseState>, wipe_server::store::StoreError> {
        Ok(None)
    }
    fn save(
        &self,
        _state: &wipe_license::LeaseState,
    ) -> Result<(), wipe_server::store::StoreError> {
        Err(wipe_server::store::StoreError::Unavailable)
    }
}

#[tokio::test]
async fn license_route_reports_a_failed_lease_save() {
    let ctx = Arc::new(wipe_server::LicenseContext::open(
        "station-1",
        Some(license_chain(
            time::OffsetDateTime::now_utc() + time::Duration::days(30),
        )),
        Arc::new(FailingSaveStore),
        time::OffsetDateTime::now_utc(),
    ));
    // A lease fault never uncovers an unlimited licence (decision 5).
    assert!(
        !ctx.decide_start(&sim_start(), time::OffsetDateTime::now_utc())
            .unwrap()
            .evaluation
    );
    let base = spawn_server_with_license(ctx).await;
    let view = get_license(&base).await;
    assert_eq!(
        view["status"]["status"], "lease_state_unavailable",
        "{view}"
    );
    assert!(
        view["status"]["detail"]
            .as_str()
            .unwrap()
            .contains("failing-save"),
        "{view}"
    );
}

// --- POST /api/license (licence-visibility story 4) --------------------------

/// A station started with `roots` (`--vendor-root-b64`) and, optionally, a
/// licence context, signing with `key`.
async fn spawn_server_with_roots(
    key: Arc<SigningKey>,
    roots: Vec<VerifyingKey>,
    license: Option<Arc<wipe_server::LicenseContext>>,
) -> (String, AppState) {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let mut state = AppState::new(backend, None, key).with_vendor_roots(roots);
    if let Some(license) = license {
        state = state.with_license(license);
    }
    let app = router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

/// A licence `license_id` from `root` for the instance key `key_id`.
fn issue_licence(
    root: &wipe_license::VendorRoot,
    license_id: &str,
    key_id: &str,
    not_after: time::OffsetDateTime,
) -> wipe_license::LicenseCertificate {
    let now = time::OffsetDateTime::now_utc();
    let ent = wipe_license::Entitlements {
        customer_id: format!("cust-{license_id}"),
        customer_name: "Acme Recycling".into(),
        quota: wipe_license::Quota::Unlimited,
        scope: wipe_license::Scope::Site {
            site_id: "s".into(),
        },
        not_before: now - time::Duration::days(60),
        not_after,
        features: vec![],
        allowed_methods: wipe_license::AllowedMethods::All,
        machine_binding: None,
    };
    root.issue(license_id, key_id, ent, now).unwrap()
}

fn in_30_days() -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc() + time::Duration::days(30)
}

/// POST `body` to `/api/license`.
async fn post_license(base: &str, body: impl Into<reqwest::Body>) -> (u16, serde_json::Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/license"))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

/// Erase `device` and return its certificate.
async fn erase_and_certify(base: &str, device: &str) -> serde_json::Value {
    let client = reqwest::Client::new();
    let job_id = http_create_job(&client, base, device).await;
    let (status, body) = http_post(&client, format!("{base}/api/jobs/{job_id}/start"), None).await;
    assert_eq!(status, 200, "{body}");
    http_wait_for_state(&client, base, &job_id, "erased").await;
    http_wait_for_certificate(&client, base, &job_id).await
}

/// A station with a valid licence `lic-a` from `root`, with some lease
/// history, so a rejected or accepted install can be compared against it.
fn licensed_with_a(
    root: &wipe_license::VendorRoot,
    key: &SigningKey,
) -> Arc<wipe_server::LicenseContext> {
    let licence = issue_licence(root, "lic-a", &key.public_key_id(), in_30_days());
    let mut lease =
        wipe_license::LeaseState::new(time::OffsetDateTime::now_utc() - time::Duration::hours(2));
    lease.license_id = Some("lic-a".into());
    lease.erasures_used = 4;
    lease.evaluation_erasures = 2;
    Arc::new(wipe_server::LicenseContext::licensed(
        "station-1",
        wipe_license::AttestationChain::new(licence),
        lease,
    ))
}

#[tokio::test]
async fn installing_a_licence_on_an_unlicensed_station_licenses_the_next_certificate() {
    let root = wipe_license::VendorRoot::generate();
    let key = Arc::new(SigningKey::generate());
    let (base, _state) =
        spawn_server_with_roots(key.clone(), vec![root.verifying_key()], None).await;
    assert_eq!(get_license(&base).await["status"]["status"], "unlicensed");

    let licence = issue_licence(&root, "lic-1", &key.public_key_id(), in_30_days());
    let (status, view) = post_license(&base, serde_json::to_string(&licence).unwrap()).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["status"]["status"], "valid", "{view}");
    assert_eq!(
        view["customer"],
        json!({ "id": "cust-lic-1", "name": "Acme Recycling" })
    );
    assert_eq!(view["instance_public_key_id"], key.public_key_id());
    assert_eq!(get_license(&base).await, view);

    let signed = erase_and_certify(&base, "dev-nvme-0").await;
    assert_eq!(signed["certificate"]["evaluation"], false, "{signed}");
    assert_eq!(
        signed["attestation"]["license"]["body"]["license_id"],
        "lic-1"
    );
    let parsed: wipe_cert::SignedCertificate = serde_json::from_value(signed).unwrap();
    let chain: wipe_license::AttestationChain =
        serde_json::from_value(parsed.attestation.clone().unwrap()).unwrap();
    assert!(wipe_license::verify_chain(
        Some(&chain),
        &parsed.signature.public_key_id,
        time::OffsetDateTime::now_utc(),
        &[root.verifying_key()],
    )
    .is_licensed());
}

/// A licence file from the hub with a credit grant beside the licence
/// (controlled-licensing story 3) installs exactly as the bare licence does;
/// the station ignores the grant until it pools credits (story 4), and the
/// certificate's attestation chain carries the licence alone.
#[tokio::test]
async fn a_licence_file_with_a_credit_grant_installs_like_the_bare_licence() {
    let root = wipe_license::VendorRoot::generate();
    let key = Arc::new(SigningKey::generate());
    let (base, _state) =
        spawn_server_with_roots(key.clone(), vec![root.verifying_key()], None).await;

    let licence = issue_licence(&root, "lic-1", &key.public_key_id(), in_30_days());
    let grant = root
        .issue_credit_grant(
            "grant-1",
            "org-1",
            key.public_key_id(),
            50,
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    let file = wipe_license::LicenseFile {
        license: licence.clone(),
        credit_grant: Some(grant),
    };
    let (status, view) = post_license(&base, serde_json::to_string_pretty(&file).unwrap()).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["status"]["status"], "valid", "{view}");
    assert_eq!(view["instance_public_key_id"], key.public_key_id());

    let signed = erase_and_certify(&base, "dev-nvme-0").await;
    assert_eq!(signed["certificate"]["evaluation"], false, "{signed}");
    assert_eq!(
        signed["attestation"]["license"],
        serde_json::to_value(&licence).unwrap()
    );
    assert!(!signed.to_string().contains("credit_grant"), "{signed}");
}

#[tokio::test]
async fn replacing_a_licence_restarts_only_the_licensed_count() {
    let root = wipe_license::VendorRoot::generate();
    let key = Arc::new(SigningKey::generate());
    let ctx = licensed_with_a(&root, &key);
    let before = ctx.lease_state();
    let (base, state) =
        spawn_server_with_roots(key.clone(), vec![root.verifying_key()], Some(ctx)).await;

    let b = issue_licence(&root, "lic-b", &key.public_key_id(), in_30_days());
    let (status, view) = post_license(&base, serde_json::to_string(&b).unwrap()).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["customer"]["id"], "cust-lic-b");
    assert_eq!(get_license(&base).await["customer"]["id"], "cust-lic-b");
    let lease = state.license().lease_state();
    assert_eq!(lease.license_id.as_deref(), Some("lic-b"));
    assert_eq!(lease.erasures_used, 0);
    assert_eq!(lease.evaluation_erasures, before.evaluation_erasures);
    assert_eq!(lease.time_watermark, before.time_watermark);
}

#[tokio::test]
async fn reinstalling_the_same_licence_keeps_the_licensed_count() {
    let root = wipe_license::VendorRoot::generate();
    let key = Arc::new(SigningKey::generate());
    let ctx = licensed_with_a(&root, &key);
    let (base, state) =
        spawn_server_with_roots(key.clone(), vec![root.verifying_key()], Some(ctx)).await;

    let a = issue_licence(&root, "lic-a", &key.public_key_id(), in_30_days());
    let (status, view) = post_license(&base, serde_json::to_string(&a).unwrap()).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(state.license().lease_state().erasures_used, 4);
}

#[tokio::test]
async fn an_expired_but_authentic_licence_is_installed_and_reported_expired() {
    let root = wipe_license::VendorRoot::generate();
    let key = Arc::new(SigningKey::generate());
    let (base, _state) =
        spawn_server_with_roots(key.clone(), vec![root.verifying_key()], None).await;
    let past = time::OffsetDateTime::now_utc() - time::Duration::days(3);
    let licence = issue_licence(&root, "lic-old", &key.public_key_id(), past);
    let (status, view) = post_license(&base, serde_json::to_string(&licence).unwrap()).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["status"]["status"], "expired", "{view}");
    assert_eq!(get_license(&base).await, view);
}

#[tokio::test]
async fn a_body_that_is_not_a_licence_is_422_and_changes_nothing() {
    let root = wipe_license::VendorRoot::generate();
    let key = Arc::new(SigningKey::generate());
    let ctx = licensed_with_a(&root, &key);
    let (base, state) =
        spawn_server_with_roots(key.clone(), vec![root.verifying_key()], Some(ctx)).await;
    let before = get_license(&base).await;
    let lease_before = state.license().lease_state();
    let bodies: [&[u8]; 4] = [b"\"hello\"", b"{}", b"hello", b"{\"a\": \"\xff\xfe\"}"];
    for body in bodies {
        let (status, err) = post_license(&base, body.to_vec()).await;
        assert_eq!(status, 422, "{body:?}: {err}");
        assert!(
            err["error"]
                .as_str()
                .unwrap()
                .starts_with("not a licence file: "),
            "{body:?}: {err}"
        );
        assert_eq!(err.as_object().unwrap().len(), 1, "{err}");
    }
    assert_eq!(get_license(&base).await, before);
    assert_eq!(state.license().lease_state(), lease_before);
}

#[tokio::test]
async fn an_untrusted_or_tampered_licence_is_422_and_the_next_certificate_is_unchanged() {
    let root = wipe_license::VendorRoot::generate();
    let key = Arc::new(SigningKey::generate());
    let ctx = licensed_with_a(&root, &key);
    let (base, state) =
        spawn_server_with_roots(key.clone(), vec![root.verifying_key()], Some(ctx)).await;
    let before = get_license(&base).await;
    let lease_before = state.license().lease_state();

    let other_root = wipe_license::VendorRoot::generate();
    let foreign = issue_licence(&other_root, "lic-x", &key.public_key_id(), in_30_days());
    let mut tampered = issue_licence(&root, "lic-y", &key.public_key_id(), in_30_days());
    tampered.body.entitlements.customer_name = "Someone Else".into();
    for licence in [foreign, tampered] {
        let (status, err) = post_license(&base, serde_json::to_string(&licence).unwrap()).await;
        assert_eq!(status, 422, "{err}");
        assert!(
            err["error"]
                .as_str()
                .unwrap()
                .starts_with("licence did not verify"),
            "{err}"
        );
    }
    assert_eq!(get_license(&base).await, before);
    assert_eq!(state.license().lease_state(), lease_before);

    let signed = erase_and_certify(&base, "dev-nvme-0").await;
    assert_eq!(signed["certificate"]["evaluation"], false, "{signed}");
    assert_eq!(
        signed["attestation"]["license"]["body"]["license_id"],
        "lic-a"
    );
}

#[tokio::test]
async fn a_licence_for_another_station_is_422_naming_both_keys() {
    let root = wipe_license::VendorRoot::generate();
    let key = Arc::new(SigningKey::generate());
    let (base, _state) =
        spawn_server_with_roots(key.clone(), vec![root.verifying_key()], None).await;
    let other_key = SigningKey::generate().public_key_id();
    let licence = issue_licence(&root, "lic-1", &other_key, in_30_days());
    let (status, err) = post_license(&base, serde_json::to_string(&licence).unwrap()).await;
    assert_eq!(status, 422, "{err}");
    let expected = wipe_license::InstallProblem::WrongInstanceKey {
        expected: key.public_key_id(),
        found: other_key,
    }
    .to_string();
    assert_eq!(err, json!({ "error": expected }));
    assert_eq!(get_license(&base).await["status"]["status"], "unlicensed");

    let signed = erase_and_certify(&base, "dev-nvme-0").await;
    assert_eq!(signed["certificate"]["evaluation"], true, "{signed}");
    assert!(signed.get("attestation").is_none(), "{signed}");
}

#[tokio::test]
async fn a_station_with_no_vendor_root_refuses_an_install_with_409() {
    let root = wipe_license::VendorRoot::generate();
    let key = Arc::new(SigningKey::generate());
    let (base, _state) = spawn_server_with_roots(key.clone(), vec![], None).await;
    let licence = issue_licence(&root, "lic-1", &key.public_key_id(), in_30_days());
    let (status, err) = post_license(&base, serde_json::to_string(&licence).unwrap()).await;
    assert_eq!(status, 409, "{err}");
    let msg = err["error"].as_str().unwrap();
    assert!(msg.contains("no vendor root is configured"), "{err}");
    assert!(msg.contains("--vendor-root-b64"), "{err}");
    assert_eq!(get_license(&base).await["status"]["status"], "unlicensed");

    // `AppState::new`, as the desktop shell builds it, trusts no root either.
    let plain = spawn_server().await;
    let (status, _) = post_license(&plain, serde_json::to_string(&licence).unwrap()).await;
    assert_eq!(status, 409);
}

#[tokio::test]
async fn installing_over_an_unreadable_lease_file_reports_it_and_never_writes_it() {
    let dir = license_tmpdir("install-corrupt");
    let path = dir.join("lease-state.json");
    std::fs::write(&path, b"not json").unwrap();
    let ctx = Arc::new(wipe_server::LicenseContext::open(
        "station-1",
        None,
        Arc::new(wipe_server::LocalFileLeaseStore::new(path.clone())),
        time::OffsetDateTime::now_utc(),
    ));
    let root = wipe_license::VendorRoot::generate();
    let key = Arc::new(SigningKey::generate());
    let (base, state) =
        spawn_server_with_roots(key.clone(), vec![root.verifying_key()], Some(ctx)).await;
    let licence = issue_licence(&root, "lic-1", &key.public_key_id(), in_30_days());
    let (status, view) = post_license(&base, serde_json::to_string(&licence).unwrap()).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(
        view["status"]["status"], "lease_state_unavailable",
        "{view}"
    );
    let decision = state
        .license()
        .decide_start(&sim_start(), time::OffsetDateTime::now_utc())
        .unwrap();
    assert!(!decision.evaluation, "an unlimited licence stays licensed");
    state.license().spend(&decision);
    assert_eq!(std::fs::read(&path).unwrap(), b"not json");
    let _ = std::fs::remove_dir_all(&dir);
}

// --- The start gate (licence-visibility story 5, ADR-0013 §1) -----------------

/// A backend that touches "real" hardware: the mock underneath, reporting
/// `is_simulated() == false`, so the station's licence check can refuse.
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

/// Erasures slow enough to observe a running Job's reservation.
fn gate_timing() -> MockTiming {
    MockTiming {
        nvme_sanitize_secs: 1.5,
        ata_secure_erase_secs: 1.5,
        block_overwrite_secs: 1.5,
        deterministic: true,
    }
}

/// A station on a real backend (or, with `simulated`, the mock itself),
/// with `license` if given, trusting `roots` and signing with `key`. Returns
/// the base URL, the state and the mock underneath (to detach drives).
async fn spawn_gated(
    simulated: bool,
    license: Option<Arc<wipe_server::LicenseContext>>,
    roots: Vec<VerifyingKey>,
    key: Arc<SigningKey>,
) -> (String, AppState, Arc<MockBackend>) {
    spawn_gated_timed(simulated, license, roots, key, gate_timing()).await
}

async fn spawn_gated_timed(
    simulated: bool,
    license: Option<Arc<wipe_server::LicenseContext>>,
    roots: Vec<VerifyingKey>,
    key: Arc<SigningKey>,
    timing: MockTiming,
) -> (String, AppState, Arc<MockBackend>) {
    let mock = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        timing,
    ));
    let backend: Arc<dyn wipe_engine::DeviceBackend> = if simulated {
        mock.clone()
    } else {
        Arc::new(RealBackend(mock.clone()))
    };
    let mut state = AppState::new(backend, None, key).with_vendor_roots(roots);
    if let Some(license) = license {
        state = state.with_license(license);
    }
    let app = router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state, mock)
}

async fn spawn_real(
    license: Option<Arc<wipe_server::LicenseContext>>,
) -> (String, AppState, Arc<MockBackend>) {
    spawn_gated(false, license, vec![], Arc::new(SigningKey::generate())).await
}

/// Valid entitlements (unlimited, every method, no binding) to adjust.
fn gate_ent() -> wipe_license::Entitlements {
    let now = time::OffsetDateTime::now_utc();
    wipe_license::Entitlements {
        customer_id: "cust-gate".into(),
        customer_name: "Acme Recycling".into(),
        quota: wipe_license::Quota::Unlimited,
        scope: wipe_license::Scope::Site {
            site_id: "s".into(),
        },
        not_before: now - time::Duration::days(60),
        not_after: now + time::Duration::days(30),
        features: vec![],
        allowed_methods: wipe_license::AllowedMethods::All,
        machine_binding: None,
    }
}

/// A licence context for `ent` with a fresh lease in RAM.
fn gate_ctx(ent: wipe_license::Entitlements) -> Arc<wipe_server::LicenseContext> {
    gate_ctx_with_lease(
        ent,
        wipe_license::LeaseState::new(time::OffsetDateTime::now_utc()),
    )
}

fn gate_ctx_with_lease(
    ent: wipe_license::Entitlements,
    lease: wipe_license::LeaseState,
) -> Arc<wipe_server::LicenseContext> {
    Arc::new(wipe_server::LicenseContext::licensed(
        "station-1",
        gate_chain(ent),
        lease,
    ))
}

/// Licence `lic-gate` for `ent`.
fn gate_chain(ent: wipe_license::Entitlements) -> wipe_license::AttestationChain {
    let cert = wipe_license::VendorRoot::generate()
        .issue(
            "lic-gate",
            "ed25519:instance-key",
            ent,
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    wipe_license::AttestationChain::new(cert)
}

fn count(erasures: u64) -> wipe_license::Entitlements {
    wipe_license::Entitlements {
        quota: wipe_license::Quota::Count { erasures },
        ..gate_ent()
    }
}

/// A queued Job that erases `device` for reuse at `classification`.
async fn http_create_reuse_job(
    client: &reqwest::Client,
    base: &str,
    device: &str,
    classification: &str,
) -> String {
    let (status, created) = http_post(
        client,
        format!("{base}/api/jobs"),
        Some(json!({
            "device_id": device,
            "classification": classification,
            "intent": "reuse",
            "operator": http_op(),
        })),
    )
    .await;
    assert_eq!(status, 200, "{created}");
    created["job_id"].as_str().unwrap().to_string()
}

async fn http_start(
    client: &reqwest::Client,
    base: &str,
    job_id: &str,
) -> (u16, serde_json::Value) {
    http_post(client, format!("{base}/api/jobs/{job_id}/start"), None).await
}

/// Start `job_id` expecting a refusal: 409 whose text contains every one of
/// `wanted`; the Job stays queued, and nothing is broadcast.
async fn assert_refused(
    client: &reqwest::Client,
    base: &str,
    state: &AppState,
    job_id: &str,
    wanted: &[&str],
) -> String {
    let mut rx = state.runner.subscribe();
    let (status, body) = http_start(client, base, job_id).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body.as_object().unwrap().len(), 1, "{body}");
    let text = body["error"].as_str().unwrap().to_string();
    for w in wanted {
        assert!(text.contains(w), "{text:?} should contain {w:?}");
    }
    let job = http_get_job(client, base, job_id).await;
    assert_eq!(job["state"]["state"], "queued", "{job}");
    assert!(job.get("licence").is_none(), "{job}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        matches!(
            rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "a refused start broadcast something"
    );
    text
}

/// Credits left as `GET /api/license` reports them: `remaining_erasures`
/// while valid, 0 once every credit is spent or reserved.
async fn remaining(base: &str) -> serde_json::Value {
    let status = get_license(base).await["status"].clone();
    if status["status"] == "quota_exhausted" {
        return json!(0);
    }
    status["remaining_erasures"].clone()
}

/// Poll `GET /api/license` until `remaining_erasures` is `want`.
async fn wait_for_remaining(base: &str, want: u64) {
    for _ in 0..200 {
        if remaining(base).await == json!(want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "remaining_erasures never reached {want}: {}",
        get_license(base).await
    );
}

#[tokio::test]
async fn a_licensed_start_reserves_a_credit_and_its_certificate_spends_it() {
    let ctx = gate_ctx(count(5));
    let (base, state, _) = spawn_real(Some(ctx.clone())).await;
    let client = reqwest::Client::new();
    let job_id = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    let (status, body) = http_start(&client, &base, &job_id).await;
    assert_eq!(status, 200, "{body}");

    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["licence"]["evaluation"], false, "{job}");
    assert_eq!(job["licence"]["license_id"], "lic-gate");
    assert_eq!(job["licence"]["credits_reserved"], 1);
    assert_eq!(
        job["licence"]["attestation"]["license"]["body"]["license_id"],
        "lic-gate"
    );
    assert_eq!(remaining(&base).await, json!(4));
    assert_eq!(get_license(&base).await["new_jobs_refused"], json!(null));

    let signed = http_wait_for_certificate(&client, &base, &job_id).await;
    assert_eq!(signed["certificate"]["evaluation"], false, "{signed}");
    assert_eq!(
        signed["attestation"]["license"]["body"]["license_id"],
        "lic-gate"
    );
    // The decision itself is station bookkeeping, never signed.
    assert!(signed["certificate"].get("licence").is_none(), "{signed}");
    for _ in 0..100 {
        if ctx.lease_state().erasures_used == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let lease = state.license().lease_state();
    assert_eq!((lease.erasures_used, lease.credits_reserved), (1, 0));
    assert_eq!(remaining(&base).await, json!(4));
}

#[tokio::test]
async fn an_unlicensed_real_station_refuses_starts_and_scanned_records() {
    let (base, state, _) = spawn_real(None).await;
    let client = reqwest::Client::new();
    let job_id = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    let text = assert_refused(
        &client,
        &base,
        &state,
        &job_id,
        &["no licence", "Install a licence"],
    )
    .await;

    let (status, body) = http_post(
        &client,
        format!("{base}/api/destruction/records"),
        Some(group_body(json!([{ "serial": "DEAD-1" }]))),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"], text);
    assert_eq!(
        http_list_jobs(&client, &base).await.len(),
        1,
        "nothing created"
    );

    // Recording the queued Job for destruction is a start too.
    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("shred", http_op())),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["state"]["state"], "queued");
    assert_eq!(count_activities(&job, "destruction"), 0);

    let view = get_license(&base).await;
    assert_eq!(view["new_jobs_refused"], text);
    assert_eq!(view["simulated_backend"], false);
}

#[tokio::test]
async fn each_licence_problem_refuses_with_its_own_reason() {
    let now = time::OffsetDateTime::now_utc();
    let expired_on = now - time::Duration::days(3);
    let valid_from = now + time::Duration::days(3);
    let cases: Vec<(Arc<wipe_server::LicenseContext>, Vec<String>)> = vec![
        (
            gate_ctx(wipe_license::Entitlements {
                not_after: expired_on,
                ..gate_ent()
            }),
            vec![
                format!("expired on {}", expired_on.date()),
                "Install the renewed licence".into(),
            ],
        ),
        (
            gate_ctx(wipe_license::Entitlements {
                not_before: valid_from,
                ..gate_ent()
            }),
            vec![format!("not valid until {}", valid_from.date())],
        ),
        (
            gate_ctx(wipe_license::Entitlements {
                machine_binding: Some("mf1:another-bench".into()),
                ..gate_ent()
            }),
            vec![
                "bound to machine mf1:another-bench".into(),
                wipe_license::machine_fingerprint("station-1"),
            ],
        ),
        (
            gate_ctx_with_lease(
                gate_ent(),
                wipe_license::LeaseState::new(now + time::Duration::days(2)),
            ),
            vec!["moved backwards".into(), "Correct the system clock".into()],
        ),
    ];
    let mut texts = Vec::new();
    for (ctx, wanted) in cases {
        let (base, state, _) = spawn_real(Some(ctx)).await;
        let client = reqwest::Client::new();
        let job_id = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
        let wanted: Vec<&str> = wanted.iter().map(String::as_str).collect();
        let text = assert_refused(&client, &base, &state, &job_id, &wanted).await;
        assert_eq!(get_license(&base).await["new_jobs_refused"], text);
        texts.push(text);
    }
    texts.dedup();
    assert_eq!(texts.len(), 4, "{texts:#?}");
}

#[tokio::test]
async fn the_last_credit_goes_to_one_start_and_the_next_is_refused() {
    let (base, state, _) = spawn_real(Some(gate_ctx(count(1)))).await;
    let client = reqwest::Client::new();
    let first = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    let second = http_create_reuse_job(&client, &base, "dev-nvme-1", "high").await;
    assert_eq!(http_start(&client, &base, &first).await.0, 200);
    assert_refused(
        &client,
        &base,
        &state,
        &second,
        &["no credits left", "Add credits"],
    )
    .await;
}

#[tokio::test]
async fn a_method_the_licence_does_not_allow_is_refused_naming_it() {
    let only_overwrite = wipe_license::Entitlements {
        allowed_methods: wipe_license::AllowedMethods::Only {
            classes: vec![wipe_license::MethodClass::BlockOverwrite],
        },
        ..gate_ent()
    };
    let (base, state, mock) = spawn_real(Some(gate_ctx(only_overwrite))).await;
    let client = reqwest::Client::new();
    let nvme = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    assert_refused(&client, &base, &state, &nvme, &["NVMe Sanitize"]).await;

    // A drive that is no longer detected: its method cannot be checked.
    use wipe_engine::DeviceSimulator;
    let gone = http_create_reuse_job(&client, &base, "dev-nvme-1", "high").await;
    mock.detach(&wipe_common::DeviceId("dev-nvme-1".into()))
        .unwrap();
    assert_refused(&client, &base, &state, &gone, &["could not be determined"]).await;
}

#[tokio::test]
async fn under_every_method_an_undetected_drive_still_starts() {
    use wipe_engine::DeviceSimulator;
    let (base, _, mock) = spawn_real(Some(gate_ctx(gate_ent()))).await;
    let client = reqwest::Client::new();
    let job_id = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    mock.detach(&wipe_common::DeviceId("dev-nvme-0".into()))
        .unwrap();
    let (status, body) = http_start(&client, &base, &job_id).await;
    assert_eq!(status, 200, "{body}");
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["licence"]["evaluation"], false);
}

#[tokio::test]
async fn a_counted_reservation_that_cannot_be_saved_is_refused() {
    let ctx = Arc::new(wipe_server::LicenseContext::open(
        "station-1",
        Some(gate_chain(count(5))),
        Arc::new(FailingSaveStore),
        time::OffsetDateTime::now_utc(),
    ));
    let (base, state, _) = spawn_real(Some(ctx.clone())).await;
    let client = reqwest::Client::new();
    let job_id = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    assert_refused(&client, &base, &state, &job_id, &["failing-save"]).await;
    assert_eq!(ctx.lease_state().credits_reserved, 0, "reservation undone");
}

/// The lease-fault rows: a valid unlimited licence whose lease cannot be
/// saved starts licensed Jobs, real or simulated, and certifies them
/// licensed; the banner reports the fault.
#[tokio::test]
async fn a_lease_fault_on_an_unlimited_licence_starts_licensed_jobs() {
    for simulated in [false, true] {
        let ctx = Arc::new(wipe_server::LicenseContext::open(
            "station-1",
            Some(gate_chain(gate_ent())),
            Arc::new(FailingSaveStore),
            time::OffsetDateTime::now_utc(),
        ));
        let (base, _, _) = spawn_gated(
            simulated,
            Some(ctx),
            vec![],
            Arc::new(SigningKey::generate()),
        )
        .await;
        let client = reqwest::Client::new();
        let job_id = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
        let (status, body) = http_start(&client, &base, &job_id).await;
        assert_eq!(status, 200, "simulated {simulated}: {body}");
        let signed = http_wait_for_certificate(&client, &base, &job_id).await;
        assert_eq!(signed["certificate"]["evaluation"], false, "{signed}");
        assert!(signed["attestation"].is_object(), "{signed}");
        let view = get_license(&base).await;
        assert_eq!(
            view["status"]["status"], "lease_state_unavailable",
            "{view}"
        );
        assert_eq!(view["new_jobs_refused"], json!(null), "{view}");
    }
}

#[tokio::test]
async fn the_simulated_backend_never_refuses_and_certifies_uncovered_starts_evaluation() {
    let expired = gate_ctx(wipe_license::Entitlements {
        not_after: time::OffsetDateTime::now_utc() - time::Duration::days(1),
        ..gate_ent()
    });
    let mut used_up = wipe_license::LeaseState::new(time::OffsetDateTime::now_utc());
    used_up.license_id = Some("lic-gate".into());
    used_up.erasures_used = 1;
    let exhausted = gate_ctx_with_lease(count(1), used_up);
    for license in [None, Some(expired), Some(exhausted)] {
        let (base, _, _) =
            spawn_gated(true, license, vec![], Arc::new(SigningKey::generate())).await;
        let client = reqwest::Client::new();
        let job_id = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
        let (status, body) = http_start(&client, &base, &job_id).await;
        assert_eq!(status, 200, "{body}");
        let job = http_get_job(&client, &base, &job_id).await;
        assert_eq!(job["licence"]["evaluation"], true, "{job}");
        let signed = http_wait_for_certificate(&client, &base, &job_id).await;
        assert_eq!(signed["certificate"]["evaluation"], true, "{signed}");
        assert!(signed.get("attestation").is_none(), "{signed}");
        let view = get_license(&base).await;
        assert_eq!(view["new_jobs_refused"], json!(null));
        assert_eq!(view["simulated_backend"], true);
    }
}

#[tokio::test]
async fn a_job_started_before_expiry_and_signed_after_it_is_licensed() {
    let slow = MockTiming {
        nvme_sanitize_secs: 6.0,
        ..gate_timing()
    };
    let (base, state, _) =
        spawn_gated_timed(false, None, vec![], Arc::new(SigningKey::generate()), slow).await;
    let client = reqwest::Client::new();
    let job_id = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    // The licence runs out a few seconds after the start, mid-erasure.
    let not_after = time::OffsetDateTime::now_utc() + time::Duration::seconds(3);
    *state.license.write() = gate_ctx(wipe_license::Entitlements {
        not_after,
        ..gate_ent()
    });
    let (status, body) = http_start(&client, &base, &job_id).await;
    assert_eq!(status, 200, "{body}");
    let mut signed = None;
    for _ in 0..600 {
        let (status, body) =
            http_get(&client, format!("{base}/api/jobs/{job_id}/certificate")).await;
        if status == 200 {
            signed = Some(body);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let signed = signed.expect("no certificate");
    assert!(
        time::OffsetDateTime::now_utc() > not_after,
        "signed too early"
    );
    assert_eq!(get_license(&base).await["status"]["status"], "expired");
    assert_eq!(signed["certificate"]["evaluation"], false, "{signed}");
    assert_eq!(
        signed["attestation"]["license"]["body"]["license_id"],
        "lic-gate"
    );
}

#[tokio::test]
async fn an_aborted_job_returns_its_credit() {
    let (base, _, _) = spawn_real(Some(gate_ctx(count(5)))).await;
    let client = reqwest::Client::new();

    // Aborted while erasing.
    let running = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    assert_eq!(http_start(&client, &base, &running).await.0, 200);
    assert_eq!(remaining(&base).await, json!(4));
    let (status, _) = http_post(&client, format!("{base}/api/jobs/{running}/abort"), None).await;
    assert_eq!(status, 200);
    wait_for_remaining(&base, 5).await;

    // Aborted while pending destruction (recorded from the queue).
    let pending = http_create_reuse_job(&client, &base, "dev-nvme-1", "high").await;
    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{pending}/record-for-destruction"),
        Some(attested("shred", http_op())),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let job = http_get_job(&client, &base, &pending).await;
    assert_eq!(job["licence"]["credits_reserved"], 1, "{job}");
    assert_eq!(remaining(&base).await, json!(4));
    let (status, _) = http_post(&client, format!("{base}/api/jobs/{pending}/abort"), None).await;
    assert_eq!(status, 200);
    wait_for_remaining(&base, 5).await;
}

#[tokio::test]
async fn a_started_job_recorded_for_destruction_keeps_its_decision() {
    let (base, _, _) = spawn_real(Some(gate_ctx(count(1)))).await;
    let client = reqwest::Client::new();
    let job_id = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    assert_eq!(http_start(&client, &base, &job_id).await.0, 200);
    let decided = http_get_job(&client, &base, &job_id).await["licence"].clone();
    // No credit is left, but no second check or credit is needed.
    let (status, body) = http_post(
        &client,
        format!("{base}/api/jobs/{job_id}/record-for-destruction"),
        Some(attested("shred", http_op())),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let job = http_get_job(&client, &base, &job_id).await;
    assert_eq!(job["licence"], decided);
    assert_eq!(remaining(&base).await, json!(0));
}

#[tokio::test]
async fn a_scanned_group_needs_one_credit_per_drive() {
    let (base, state, _) = spawn_real(Some(gate_ctx(count(2)))).await;
    let client = reqwest::Client::new();
    let mut rx = state.runner.subscribe();
    let three = json!([{ "serial": "D-1" }, { "serial": "D-2" }, { "serial": "D-3" }]);
    let (status, body) = http_post(
        &client,
        format!("{base}/api/destruction/records"),
        Some(group_body(three)),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let text = body["error"].as_str().unwrap();
    assert!(text.contains("needs 3 credits"), "{text}");
    assert!(text.contains("2 left"), "{text}");
    assert!(http_list_jobs(&client, &base).await.is_empty());
    assert!(rx.try_recv().is_err());
    assert_eq!(remaining(&base).await, json!(2));

    let two = json!([{ "serial": "D-1" }, { "serial": "D-2" }]);
    let (status, body) = http_post(
        &client,
        format!("{base}/api/destruction/records"),
        Some(group_body(two)),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    for job in http_list_jobs(&client, &base).await {
        assert_eq!(job["licence"]["credits_reserved"], 1, "{job}");
        assert_eq!(job["licence"]["evaluation"], false, "{job}");
    }
    assert_eq!(remaining(&base).await, json!(0));
}

#[tokio::test]
async fn installing_a_licence_lifts_a_refusal_without_a_restart() {
    let root = wipe_license::VendorRoot::generate();
    let key = Arc::new(SigningKey::generate());
    let (base, state, _) = spawn_gated(false, None, vec![root.verifying_key()], key.clone()).await;
    let client = reqwest::Client::new();
    let job_id = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    assert_refused(&client, &base, &state, &job_id, &["no licence"]).await;

    let licence = issue_licence(&root, "lic-1", &key.public_key_id(), in_30_days());
    let (status, view) = post_license(&base, serde_json::to_string(&licence).unwrap()).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["new_jobs_refused"], json!(null), "{view}");
    assert_eq!(view["simulated_backend"], false, "{view}");

    let (status, body) = http_start(&client, &base, &job_id).await;
    assert_eq!(status, 200, "{body}");
    let signed = http_wait_for_certificate(&client, &base, &job_id).await;
    assert_eq!(signed["certificate"]["evaluation"], false, "{signed}");
    assert_eq!(
        signed["attestation"]["license"]["body"]["license_id"],
        "lic-1"
    );
}

#[tokio::test]
async fn one_aborted_and_one_certified_job_leave_one_of_two_credits() {
    let (base, _, _) = spawn_real(Some(gate_ctx(count(2)))).await;
    let client = reqwest::Client::new();
    let aborted = http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    let certified = http_create_reuse_job(&client, &base, "dev-nvme-1", "high").await;
    assert_eq!(http_start(&client, &base, &aborted).await.0, 200);
    assert_eq!(http_start(&client, &base, &certified).await.0, 200);
    let (status, _) = http_post(&client, format!("{base}/api/jobs/{aborted}/abort"), None).await;
    assert_eq!(status, 200);
    http_wait_for_certificate(&client, &base, &certified).await;
    wait_for_remaining(&base, 1).await;
}

/// Decision 4: a licensed destroyed drive that was never erased spends its
/// credit when its certificate is signed.
#[tokio::test]
async fn scanned_drives_destroyed_under_a_licence_spend_their_credits() {
    let ctx = gate_ctx(count(2));
    let (base, _, _) = spawn_real(Some(ctx.clone())).await;
    let client = reqwest::Client::new();
    let (status, body) = http_post(
        &client,
        format!("{base}/api/destruction/records"),
        Some(group_body(
            json!([{ "serial": "D-1" }, { "serial": "D-2" }]),
        )),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let ids: Vec<String> = body["job_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 2);

    let run = http_assemble(&client, &base, &ids).await;
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
    let mut completion = completion_body();
    completion["completed_at"] = preview["completed_at"].clone();
    let (status, done) = http_post(
        &client,
        format!("{base}/api/manifests/{run_id}/completion"),
        Some(completion),
    )
    .await;
    assert_eq!(status, 200, "{done}");

    for id in &ids {
        let signed = http_wait_for_certificate(&client, &base, id).await;
        assert_eq!(signed["certificate"]["evaluation"], false, "{signed}");
        assert!(signed["attestation"].is_object(), "{signed}");
    }
    for _ in 0..100 {
        if ctx.lease_state().erasures_used == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let lease = ctx.lease_state();
    assert_eq!((lease.erasures_used, lease.credits_reserved), (2, 0));
}

/// A scanned group the runner rejects after the licence reserved its
/// credits (a drive matching an open Job) returns the reservation.
#[tokio::test]
async fn a_scanned_group_the_runner_rejects_returns_its_reservation() {
    let ctx = gate_ctx(count(3));
    let (base, _, _) = spawn_real(Some(ctx.clone())).await;
    let client = reqwest::Client::new();
    // A queued Job on an attached drive: its serial marks it open.
    http_create_reuse_job(&client, &base, "dev-nvme-0", "high").await;
    let (status, body) = http_post(
        &client,
        format!("{base}/api/destruction/records"),
        Some(group_body(
            json!([{ "serial": "NEW-1" }, { "serial": "S6ABNX0W123456" }]),
        )),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("matches open job"),
        "{body}"
    );
    assert_eq!(remaining(&base).await, json!(3));
    assert_eq!(ctx.lease_state().credits_reserved, 0);
}
