//! Releases, the artifact store and `wipe-hub release publish`
//! (software-downloads story 1, CAP-1, CAP-4, CAP-5): one test per row of
//! the story's I/O matrix, through HTTP and the built binary, checking the
//! response, the stored rows, the files in the hub's artifact directory and
//! the audit rows. Needs `docker compose up -d`; fails, never skips, when it
//! is down.

mod support;

use std::path::Path;
use std::process::{Command, Output};

use reqwest::header::CONTENT_TYPE;
use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::hub::{
    assert_error, bootstrap_admin, closed_port, spawn_hub, spawn_hub_config, stderr, stdout, Hub,
    Who,
};
use uuid::Uuid;
use wipe_hub::artifacts::{ArtifactStore, LocalDirStore};
use wipe_hub::release_admin::{
    CLI_ACTOR, MSG_ALREADY_WITHDRAWN, MSG_ARTIFACT_TOO_LARGE, MSG_BAD_FILENAME, MSG_BAD_PLATFORM,
    MSG_EMPTY_ARTIFACT, MSG_NO_ARTIFACT, MSG_NO_ARTIFACTS, MSG_NO_RELEASE, MSG_PLATFORM_TAKEN,
    MSG_PUBLISH_NOT_DRAFT, MSG_RELEASE_NOT_DRAFT, MSG_STORAGE_UNAVAILABLE, MSG_VERSION_TAKEN,
};

const ADMIN_ONLY: &str = "only vendor admins can use the admin area";
const NOT_SIGNED_IN: &str = "not signed in";
const BAD_CSRF: &str = "missing or invalid CSRF token";
const OCTET: &str = "application/octet-stream";
const LINUX: &str = "linux-x86_64";

struct Fixture {
    hub: Hub,
    admin: Who,
}

async fn fixture() -> Fixture {
    let hub = spawn_hub().await;
    let admin = bootstrap_admin(&hub).await;
    Fixture { hub, admin }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A station "build": distinct bytes per `seed`.
fn build(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// Every name in `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

async fn read_stored(dir: &Path, key: &str) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut r = LocalDirStore::new(dir).open(key).await.unwrap();
    let mut out = Vec::new();
    r.read_to_end(&mut out).await.unwrap();
    out
}

async fn ok_json(resp: Response, want: StatusCode) -> Value {
    let code = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(code, want, "{body}");
    body
}

fn id_of(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

fn subject(id: &str) -> String {
    format!("release:{id}")
}

fn create_body(version: &str) -> Value {
    json!({"version": version, "notes": "First build.\n\n- erases\ttwice as fast", "channel": "stable"})
}

impl Fixture {
    fn dir(&self) -> &Path {
        self.hub.artifacts.path()
    }

    async fn get(&self, path: &str) -> Response {
        self.hub.get(path, Some(&self.admin.cookie)).await
    }

    async fn post(&self, path: &str, body: Value) -> Response {
        self.hub.post(path, body, Some(&self.admin.cookie)).await
    }

    async fn post_empty(&self, path: &str) -> Response {
        self.hub.post_empty(path, Some(&self.admin.cookie)).await
    }

    async fn create(&self, version: &str) -> Value {
        let body = ok_json(
            self.post("/api/admin/releases", create_body(version)).await,
            StatusCode::CREATED,
        )
        .await;
        body["release"].clone()
    }

    /// `POST …/artifacts?<query>` with `bytes` as the raw body.
    async fn upload_query(
        &self,
        release: &str,
        query: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Response {
        self.hub
            .post_req(
                &format!("/api/admin/releases/{release}/artifacts?{query}"),
                Some(&self.admin.cookie),
            )
            .header(CONTENT_TYPE, content_type)
            .body(bytes)
            .send()
            .await
            .unwrap()
    }

    async fn upload(
        &self,
        release: &str,
        platform: &str,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Response {
        self.upload_query(
            release,
            &format!("platform={platform}&filename={filename}"),
            bytes,
            OCTET,
        )
        .await
    }

    /// Upload, expecting 201; the artifact.
    async fn uploaded(&self, release: &str, platform: &str, bytes: Vec<u8>) -> Value {
        let body = ok_json(
            self.upload(release, platform, "wipestation", bytes).await,
            StatusCode::CREATED,
        )
        .await;
        body["artifact"].clone()
    }

    /// An upload sent with `Transfer-Encoding: chunked` (no
    /// `Content-Length`), over a raw connection: the status and the whole
    /// response text.
    async fn chunked_upload(&self, release: &str, chunks: &[&[u8]]) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut conn = tokio::net::TcpStream::connect(self.hub.addr).await.unwrap();
        let head = format!(
            "POST /api/admin/releases/{release}/artifacts?platform={LINUX}&filename=w HTTP/1.1\r\n\
             Host: {}\r\nCookie: {}; wipehub_csrf={}\r\nX-CSRF-Token: {}\r\n\
             Content-Type: {OCTET}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            self.hub.addr, self.admin.cookie, self.hub.csrf, self.hub.csrf
        );
        conn.write_all(head.as_bytes()).await.unwrap();
        for chunk in chunks {
            conn.write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .await
                .unwrap();
            // The hub may answer and close before taking every byte.
            if conn.write_all(chunk).await.is_err() || conn.write_all(b"\r\n").await.is_err() {
                break;
            }
        }
        let _ = conn.write_all(b"0\r\n\r\n").await;
        let mut out = Vec::new();
        let _ = conn.read_to_end(&mut out).await;
        let text = String::from_utf8_lossy(&out).into_owned();
        let status = text
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("no status line in {text:?}"));
        (status, text)
    }

    async fn publish(&self, id: &str) -> Response {
        self.post_empty(&format!("/api/admin/releases/{id}/publish"))
            .await
    }

    async fn withdraw(&self, id: &str) -> Response {
        self.post_empty(&format!("/api/admin/releases/{id}/withdraw"))
            .await
    }

    async fn remove(&self, id: &str, artifact: &str) -> Response {
        self.post_empty(&format!(
            "/api/admin/releases/{id}/artifacts/{artifact}/remove"
        ))
        .await
    }

    /// A published release with one Linux artifact: (release, artifact).
    async fn published(&self, version: &str) -> (Value, Value) {
        let r = self.create(version).await;
        let a = self.uploaded(&id_of(&r), LINUX, build(1, 100)).await;
        let r = ok_json(self.publish(&id_of(&r)).await, StatusCode::OK).await;
        (r["release"].clone(), a)
    }

    /// Every audit row for `action`, checking the actor; `(subject, detail)`.
    async fn audits(&self, action: &str) -> Vec<(String, Value)> {
        let rows = self.hub.audited(action).await;
        for row in &rows {
            assert_eq!(row.0, format!("user:{}", self.admin.id), "{action}");
        }
        rows.into_iter().map(|r| (r.1, r.2)).collect()
    }

    /// `(audit rows, releases, artifacts)`: what a refused request must leave alone.
    async fn counts(&self) -> (i64, i64, i64) {
        (
            self.hub.count("audit_log").await,
            self.hub.count("releases").await,
            self.hub.count("release_artifacts").await,
        )
    }
}

#[tokio::test]
async fn create_makes_an_audited_draft() {
    let f = fixture().await;
    let r = f.create(" 0.2.0 ").await;
    let id = id_of(&r);
    assert_eq!(r["version"], "0.2.0");
    assert_eq!(r["status"], "draft");
    assert_eq!(r["channel"], "stable");
    assert_eq!(r["notes"], "First build.\n\n- erases\ttwice as fast");
    assert_eq!(r["artifacts"], json!([]));
    assert!(
        r["published_at"].is_null() && r["withdrawn_at"].is_null(),
        "{r}"
    );
    assert!(r["created_at"].as_str().unwrap().ends_with('Z'), "{r}");
    assert_eq!(
        f.audits("release.created").await,
        [(
            subject(&id),
            json!({"version": "0.2.0", "channel": "stable"})
        )]
    );
    let created_by: Option<Uuid> =
        sqlx::query_scalar("SELECT created_by FROM releases WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(created_by.unwrap().to_string(), f.admin.id);
    let p = ok_json(
        f.post(
            "/api/admin/releases",
            json!({"version": "0.3.0-rc.1", "notes": "Preview", "channel": "preview"}),
        )
        .await,
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(p["release"]["channel"], "preview");
}

#[tokio::test]
async fn bad_create_bodies_are_400_naming_the_field() {
    let f = fixture().await;
    let before = f.counts().await;
    let with = |k: &str, v: Value| {
        let mut b = create_body("1.0.0");
        b[k] = v;
        b
    };
    for (body, field) in [
        (with("version", json!("1.0")), "version"),
        (with("version", json!("v1.0.0")), "version"),
        (with("version", json!("1.0.0+build.5")), "version"),
        (with("version", json!("")), "version"),
        (
            with("version", json!(format!("1.0.0-{}", "a".repeat(60)))),
            "version",
        ),
        (with("channel", json!("beta")), "channel"),
        (with("notes", json!("  \n ")), "notes"),
        (with("notes", json!("x".repeat(10_001))), "notes"),
        (with("notes", json!("bell\u{7}")), "notes"),
        (with("notes", json!("cr\rlf")), "notes"),
        (with("status", json!("published")), "status"),
        (with("version", json!(1)), "version"),
        (json!({"version": "1.0.0", "channel": "stable"}), "notes"),
    ] {
        let (code, msg) =
            support::hub::error_of(f.post("/api/admin/releases", body.clone()).await).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}: {msg}");
        assert!(msg.contains(field), "{body}: {msg}");
    }
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn a_version_is_never_reused_even_after_withdrawal() {
    let f = fixture().await;
    let r = f.create("0.2.0").await;
    let before = f.counts().await;
    assert_error(
        f.post("/api/admin/releases", create_body("0.2.0")).await,
        StatusCode::CONFLICT,
        MSG_VERSION_TAKEN,
    )
    .await;
    assert_eq!(f.counts().await, before);
    ok_json(f.withdraw(&id_of(&r)).await, StatusCode::OK).await;
    let before = f.counts().await;
    assert_error(
        f.post("/api/admin/releases", create_body("0.2.0")).await,
        StatusCode::CONFLICT,
        MSG_VERSION_TAKEN,
    )
    .await;
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn an_upload_is_stored_hashed_by_the_hub_and_audited() {
    let f = fixture().await;
    let r = f.create("0.2.0").await;
    let id = id_of(&r);
    // Bigger than one body chunk, so the hash covers a stream.
    let bytes = build(7, 300_000);
    let resp = f
        .upload_query(
            &id,
            &format!("platform={LINUX}&filename=wipestation-0.2.0.tar.gz"),
            bytes.clone(),
            "application/octet-stream; charset=binary",
        )
        .await;
    let a = ok_json(resp, StatusCode::CREATED).await["artifact"].clone();
    let key = id_of(&a);
    assert_eq!(a["platform"], LINUX);
    assert_eq!(a["filename"], "wipestation-0.2.0.tar.gz");
    assert_eq!(a["size_bytes"], bytes.len());
    assert_eq!(a["sha256"], sha256_hex(&bytes));
    assert!(a["created_at"].as_str().unwrap().ends_with('Z'));
    // The store holds exactly the bytes, and nothing else (no temp file).
    assert_eq!(read_stored(f.dir(), &key).await, bytes);
    assert_eq!(entries(f.dir()), [key.as_str()]);
    assert_eq!(
        f.audits("release.artifact_added").await,
        [(
            subject(&id),
            json!({
                "artifact_id": key,
                "platform": LINUX,
                "filename": "wipestation-0.2.0.tar.gz",
                "size_bytes": bytes.len(),
                "sha256": sha256_hex(&bytes),
            })
        )]
    );
    let detail = ok_json(
        f.get(&format!("/api/admin/releases/{id}")).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(detail["release"]["artifacts"], json!([a]));
}

#[tokio::test]
async fn bad_uploads_store_and_write_nothing() {
    let f = fixture().await;
    let id = id_of(&f.create("0.2.0").await);
    let before = f.counts().await;
    let body = build(1, 10);
    for (query, message) in [
        ("filename=wipestation".to_string(), MSG_BAD_PLATFORM),
        ("platform=Linux&filename=w".into(), MSG_BAD_PLATFORM),
        ("platform=linux/x86&filename=w".into(), MSG_BAD_PLATFORM),
        ("platform=-x&filename=w".into(), MSG_BAD_PLATFORM),
        (format!("platform={LINUX}"), MSG_BAD_FILENAME),
        (
            format!("platform={LINUX}&filename=..%2Fetc"),
            MSG_BAD_FILENAME,
        ),
        (
            format!("platform={LINUX}&filename=.hidden"),
            MSG_BAD_FILENAME,
        ),
        (format!("platform={LINUX}&filename=a%20b"), MSG_BAD_FILENAME),
    ] {
        assert_error(
            f.upload_query(&id, &query, body.clone(), OCTET).await,
            StatusCode::BAD_REQUEST,
            message,
        )
        .await;
    }
    let (code, msg) = support::hub::error_of(
        f.upload_query(
            &id,
            &format!("platform={LINUX}&filename=w&sha256=x"),
            body.clone(),
            OCTET,
        )
        .await,
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{msg}");
    assert!(msg.contains("platform and filename"), "{msg}");
    assert_error(
        f.upload(&id, LINUX, "wipestation", Vec::new()).await,
        StatusCode::BAD_REQUEST,
        MSG_EMPTY_ARTIFACT,
    )
    .await;
    // Not the raw file: refused before the body is read.
    for ct in ["application/json", "multipart/form-data; boundary=x", ""] {
        let (code, msg) = support::hub::error_of(
            f.upload_query(
                &id,
                &format!("platform={LINUX}&filename=w"),
                body.clone(),
                ct,
            )
            .await,
        )
        .await;
        assert_eq!(code, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{ct}: {msg}");
    }
    // Without the CSRF token.
    let resp = f
        .hub
        .http
        .post(f.hub.url(&format!(
            "/api/admin/releases/{id}/artifacts?platform={LINUX}&filename=w"
        )))
        .header(reqwest::header::COOKIE, &f.admin.cookie)
        .header(CONTENT_TYPE, OCTET)
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_error(resp, StatusCode::FORBIDDEN, BAD_CSRF).await;
    // Unknown or non-UUID release.
    for bad in [Uuid::new_v4().to_string(), "not-a-uuid".into()] {
        assert_error(
            f.upload(&bad, LINUX, "w", body.clone()).await,
            StatusCode::NOT_FOUND,
            MSG_NO_RELEASE,
        )
        .await;
    }
    assert!(entries(f.dir()).is_empty(), "{:?}", entries(f.dir()));
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn too_large_is_413_and_keeps_nothing() {
    let hub = spawn_hub_config(|_| {}, |c| c.artifact_max_bytes = 1024).await;
    let admin = bootstrap_admin(&hub).await;
    let f = Fixture { hub, admin };
    let id = id_of(&f.create("0.2.0").await);
    let before = f.counts().await;
    assert_error(
        f.upload(&id, LINUX, "wipestation", build(3, 1025)).await,
        StatusCode::PAYLOAD_TOO_LARGE,
        MSG_ARTIFACT_TOO_LARGE,
    )
    .await;
    assert!(entries(f.dir()).is_empty(), "{:?}", entries(f.dir()));
    assert_eq!(f.counts().await, before);
    // Without a declared length, the bytes themselves are counted.
    let (status, text) = f
        .chunked_upload(&id, &[&build(3, 600), &build(4, 600)])
        .await;
    assert_eq!(status, 413, "{text}");
    assert!(text.contains(MSG_ARTIFACT_TOO_LARGE), "{text}");
    assert!(entries(f.dir()).is_empty(), "{:?}", entries(f.dir()));
    assert_eq!(f.counts().await, before);
    // Exactly the limit is accepted, streamed or not.
    let (status, text) = f
        .chunked_upload(&id, &[&build(5, 600), &build(6, 424)])
        .await;
    assert_eq!(status, 201, "{text}");
    let mut whole = build(5, 600);
    whole.extend(build(6, 424));
    assert!(text.contains(&sha256_hex(&whole)), "{text}");
    let a = f.uploaded(&id, "windows-x86_64", build(3, 1024)).await;
    assert_eq!(a["size_bytes"], 1024);
}

#[tokio::test]
async fn a_storage_failure_is_503_without_details() {
    let f = fixture().await;
    let id = id_of(&f.create("0.2.0").await);
    let before = f.counts().await;
    std::fs::remove_dir_all(f.dir()).unwrap();
    assert_error(
        f.upload(&id, LINUX, "wipestation", build(1, 50)).await,
        StatusCode::SERVICE_UNAVAILABLE,
        MSG_STORAGE_UNAVAILABLE,
    )
    .await;
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn a_taken_platform_or_a_release_not_in_draft_is_409_and_keeps_no_file() {
    let f = fixture().await;
    let r = f.create("0.2.0").await;
    let id = id_of(&r);
    let first = f.uploaded(&id, LINUX, build(1, 50)).await;
    let before = f.counts().await;
    // The second file is stored, then refused in the transaction and deleted.
    assert_error(
        f.upload(&id, LINUX, "other", build(2, 50)).await,
        StatusCode::CONFLICT,
        MSG_PLATFORM_TAKEN,
    )
    .await;
    assert_eq!(entries(f.dir()), [id_of(&first)]);
    assert_eq!(f.counts().await, before);

    ok_json(f.publish(&id).await, StatusCode::OK).await;
    let withdrawn = id_of(&f.create("0.1.0").await);
    ok_json(f.withdraw(&withdrawn).await, StatusCode::OK).await;
    let before = f.counts().await;
    for release in [&id, &withdrawn] {
        assert_error(
            f.upload(release, "windows-x86_64", "w.exe", build(4, 50))
                .await,
            StatusCode::CONFLICT,
            MSG_RELEASE_NOT_DRAFT,
        )
        .await;
    }
    assert_eq!(entries(f.dir()), [id_of(&first)]);
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn removing_a_drafts_file_deletes_it_and_audits() {
    let f = fixture().await;
    let id = id_of(&f.create("0.2.0").await);
    let linux = f.uploaded(&id, LINUX, build(1, 40)).await;
    let mac = f.uploaded(&id, "macos-aarch64", build(2, 40)).await;
    let body = ok_json(f.remove(&id, &id_of(&linux)).await, StatusCode::OK).await;
    assert_eq!(body["release"]["artifacts"], json!([mac]));
    assert_eq!(entries(f.dir()), [id_of(&mac)]);
    assert_eq!(
        f.audits("release.artifact_removed").await,
        [(
            subject(&id),
            json!({
                "artifact_id": id_of(&linux),
                "platform": LINUX,
                "filename": "wipestation",
                "size_bytes": 40,
                "sha256": linux["sha256"],
            })
        )]
    );
    // The platform is free again.
    f.uploaded(&id, LINUX, build(3, 40)).await;

    let before = f.counts().await;
    let other = id_of(&f.create("0.3.0").await);
    let before = (before.0 + 1, before.1 + 1, before.2);
    for (release, artifact) in [
        (id.clone(), Uuid::new_v4().to_string()),
        (id.clone(), "not-a-uuid".into()),
        (id.clone(), id_of(&linux)),
        // An artifact of another release.
        (other.clone(), id_of(&mac)),
    ] {
        assert_error(
            f.remove(&release, &artifact).await,
            StatusCode::NOT_FOUND,
            MSG_NO_ARTIFACT,
        )
        .await;
    }
    assert_error(
        f.remove(&Uuid::new_v4().to_string(), &id_of(&mac)).await,
        StatusCode::NOT_FOUND,
        MSG_NO_RELEASE,
    )
    .await;
    assert_eq!(f.counts().await, before);

    ok_json(f.publish(&id).await, StatusCode::OK).await;
    let before = f.counts().await;
    assert_error(
        f.remove(&id, &id_of(&mac)).await,
        StatusCode::CONFLICT,
        MSG_RELEASE_NOT_DRAFT,
    )
    .await;
    assert_eq!(f.counts().await, before);
    assert!(entries(f.dir()).contains(&id_of(&mac)));
}

#[tokio::test]
async fn publish_needs_a_draft_with_a_file() {
    let f = fixture().await;
    let id = id_of(&f.create("0.2.0").await);
    let before = f.counts().await;
    assert_error(f.publish(&id).await, StatusCode::CONFLICT, MSG_NO_ARTIFACTS).await;
    assert_eq!(f.counts().await, before);
    let mac = f.uploaded(&id, "macos-aarch64", build(2, 40)).await;
    let linux = f.uploaded(&id, LINUX, build(1, 40)).await;
    let r = ok_json(f.publish(&id).await, StatusCode::OK).await["release"].clone();
    assert_eq!(r["status"], "published");
    assert!(r["published_at"].as_str().unwrap().ends_with('Z'), "{r}");
    assert!(r["withdrawn_at"].is_null());
    // By platform.
    assert_eq!(r["artifacts"], json!([linux, mac]));
    assert_eq!(
        f.audits("release.published").await,
        [(
            subject(&id),
            json!({"artifact_ids": [id_of(&linux), id_of(&mac)]})
        )]
    );
    let before = f.counts().await;
    assert_error(
        f.publish(&id).await,
        StatusCode::CONFLICT,
        MSG_PUBLISH_NOT_DRAFT,
    )
    .await;
    for bad in [Uuid::new_v4().to_string(), "nope".into()] {
        assert_error(f.publish(&bad).await, StatusCode::NOT_FOUND, MSG_NO_RELEASE).await;
        assert_error(
            f.withdraw(&bad).await,
            StatusCode::NOT_FOUND,
            MSG_NO_RELEASE,
        )
        .await;
    }
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn withdraw_is_terminal_keeps_files_and_stays_listed() {
    let f = fixture().await;
    let (published, artifact) = f.published("0.2.0").await;
    let pid = id_of(&published);
    let draft = id_of(&f.create("0.3.0").await);

    let w = ok_json(f.withdraw(&pid).await, StatusCode::OK).await["release"].clone();
    assert_eq!(w["status"], "withdrawn");
    assert_eq!(w["published_at"], published["published_at"]);
    assert!(w["withdrawn_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(w["artifacts"], json!([artifact]));
    let d = ok_json(f.withdraw(&draft).await, StatusCode::OK).await["release"].clone();
    assert_eq!(d["status"], "withdrawn");
    assert!(d["published_at"].is_null());
    assert_eq!(
        f.audits("release.withdrawn").await,
        [
            (subject(&pid), json!({"previous_status": "published"})),
            (subject(&draft), json!({"previous_status": "draft"})),
        ]
    );
    // Files are kept.
    assert_eq!(entries(f.dir()), [id_of(&artifact)]);
    let before = f.counts().await;
    for id in [&pid, &draft] {
        assert_error(
            f.withdraw(id).await,
            StatusCode::CONFLICT,
            MSG_ALREADY_WITHDRAWN,
        )
        .await;
        assert_error(
            f.publish(id).await,
            StatusCode::CONFLICT,
            MSG_PUBLISH_NOT_DRAFT,
        )
        .await;
    }
    assert_eq!(f.counts().await, before);
    let list = ok_json(f.get("/api/admin/releases").await, StatusCode::OK).await;
    let statuses: Vec<&str> = list["releases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["withdrawn", "withdrawn"]);
}

#[tokio::test]
async fn list_and_view_are_newest_first_with_artifacts_and_audited() {
    let f = fixture().await;
    let (old, artifact) = f.published("0.1.0").await;
    let mid = f.create("0.2.0").await;
    ok_json(f.withdraw(&id_of(&mid)).await, StatusCode::OK).await;
    let new = f.create("0.3.0").await;

    let list = ok_json(f.get("/api/admin/releases").await, StatusCode::OK).await;
    let releases = list["releases"].as_array().unwrap();
    let ids: Vec<String> = releases.iter().map(id_of).collect();
    assert_eq!(ids, [id_of(&new), id_of(&mid), id_of(&old)]);
    assert_eq!(releases[2]["artifacts"], json!([artifact]));
    let a = releases[2]["artifacts"][0].as_object().unwrap();
    let mut keys: Vec<&str> = a.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "created_at",
            "filename",
            "id",
            "platform",
            "sha256",
            "size_bytes"
        ]
    );
    assert_eq!(
        f.audits("admin.releases_listed").await,
        [("search:releases".to_string(), json!({ "release_ids": ids }))]
    );

    let view = ok_json(
        f.get(&format!("/api/admin/releases/{}", id_of(&old))).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(view["release"], releases[2]);
    assert_eq!(
        f.audits("admin.release_viewed").await,
        [(subject(&id_of(&old)), json!({}))]
    );

    let before = f.counts().await;
    for bad in [Uuid::new_v4().to_string(), "not-a-uuid".into()] {
        assert_error(
            f.get(&format!("/api/admin/releases/{bad}")).await,
            StatusCode::NOT_FOUND,
            MSG_NO_RELEASE,
        )
        .await;
    }
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn only_vendor_admins_reach_release_routes() {
    let f = fixture().await;
    let hub = &f.hub;
    let (published, artifact) = f.published("0.2.0").await;
    let id = id_of(&published);
    let draft = id_of(&f.create("0.3.0").await);
    let owner = hub.verified_owner("Customer Co").await;
    let member = hub.add(&owner, "member").await;
    let before = f.counts().await;
    let files = entries(f.dir());

    let gets = [
        "/api/admin/releases".to_string(),
        format!("/api/admin/releases/{id}"),
        "/api/admin/releases/not-a-uuid".into(),
    ];
    let posts = [
        ("/api/admin/releases".to_string(), create_body("9.9.9")),
        (format!("/api/admin/releases/{draft}/publish"), json!({})),
        (format!("/api/admin/releases/{id}/withdraw"), json!({})),
        (
            format!(
                "/api/admin/releases/{draft}/artifacts/{}/remove",
                id_of(&artifact)
            ),
            json!({}),
        ),
    ];
    for (cookie, code, message) in [
        (
            Some(owner.cookie.as_str()),
            StatusCode::FORBIDDEN,
            ADMIN_ONLY,
        ),
        (
            Some(member.cookie.as_str()),
            StatusCode::FORBIDDEN,
            ADMIN_ONLY,
        ),
        (None, StatusCode::UNAUTHORIZED, NOT_SIGNED_IN),
    ] {
        for path in &gets {
            assert_error(hub.get(path, cookie).await, code, message).await;
        }
        for (path, b) in &posts {
            assert_error(hub.post(path, b.clone(), cookie).await, code, message).await;
            assert_error(hub.post_empty(path, cookie).await, code, message).await;
        }
        // An upload is refused before its body is read or stored.
        let resp = hub
            .post_req(
                &format!("/api/admin/releases/{draft}/artifacts?platform={LINUX}&filename=w"),
                cookie,
            )
            .header(CONTENT_TYPE, OCTET)
            .body(build(9, 64))
            .send()
            .await
            .unwrap();
        assert_error(resp, code, message).await;
    }
    assert_eq!(f.counts().await, before);
    assert_eq!(entries(f.dir()), files);
}

// ---------------------------------------------------------------------------
// `wipe-hub release publish`

/// `wipe-hub release publish` against `db_url` and `artifact_dir`, with every
/// `WIPE_HUB_*` var removed and `USER` set, so the developer's environment
/// cannot change the outcome.
fn release_publish(db_url: &str, artifact_dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wipe-hub"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("WIPE_HUB_") {
            cmd.env_remove(&key);
        }
    }
    cmd.env("RUST_LOG", "warn")
        .env("USER", "release-bot")
        .args([
            "release",
            "publish",
            "--database-url",
            db_url,
            "--artifact-dir",
        ])
        .arg(artifact_dir)
        .args(args)
        .output()
        .unwrap()
}

fn assert_ok(out: &Output) {
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        stdout(out),
        stderr(out)
    );
}

fn assert_failed_with(out: &Output, message: &str) {
    assert!(!out.status.success(), "stdout: {}", stdout(out));
    assert!(stderr(out).contains(message), "stderr: {}", stderr(out));
}

#[tokio::test]
async fn cli_publish_is_listed_to_admins_with_the_files_checksum() {
    let f = fixture().await;
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("wipestation");
    let bytes = build(5, 250_000);
    std::fs::write(&path, &bytes).unwrap();
    let win = files.path().join("wipestation.exe");
    std::fs::write(&win, build(6, 1000)).unwrap();
    let out = release_publish(
        &f.hub.db.url,
        f.dir(),
        &[
            "--version",
            "0.2.0",
            "--notes",
            "First build",
            "--file",
            &format!("{LINUX}={}", path.display()),
            "--file",
            &format!("windows-x86_64={}", win.display()),
        ],
    );
    assert_ok(&out);

    let list = ok_json(f.get("/api/admin/releases").await, StatusCode::OK).await;
    let releases = list["releases"].as_array().unwrap();
    assert_eq!(releases.len(), 1);
    let r = &releases[0];
    assert_eq!(r["version"], "0.2.0");
    assert_eq!(r["status"], "published");
    assert_eq!(r["channel"], "stable");
    assert_eq!(r["notes"], "First build");
    let linux = &r["artifacts"][0];
    assert_eq!(linux["platform"], LINUX);
    assert_eq!(linux["filename"], "wipestation");
    assert_eq!(linux["size_bytes"], bytes.len());
    assert_eq!(linux["sha256"], sha256_hex(&bytes));
    assert_eq!(read_stored(f.dir(), &id_of(linux)).await, bytes);
    assert_eq!(r["artifacts"][1]["filename"], "wipestation.exe");
    assert_eq!(entries(f.dir()).len(), 2);

    // Printed: the release id and each artifact's size and checksum.
    let printed = stdout(&out);
    assert!(printed.contains(&id_of(r)), "{printed}");
    assert!(printed.contains(&sha256_hex(&bytes)), "{printed}");
    assert!(
        printed.contains(&format!("{} bytes", bytes.len())),
        "{printed}"
    );

    // Audited as the CLI, with the OS user.
    let id = id_of(r);
    for (action, detail) in [
        (
            "release.created",
            json!({"version": "0.2.0", "channel": "stable", "os_user": "release-bot"}),
        ),
        (
            "release.published",
            json!({"artifact_ids": [id_of(linux), id_of(&r["artifacts"][1])], "os_user": "release-bot"}),
        ),
    ] {
        assert_eq!(
            f.hub.audited(action).await,
            [(CLI_ACTOR.to_string(), subject(&id), detail)],
            "{action}"
        );
    }
    let added = f.hub.audited("release.artifact_added").await;
    assert_eq!(added.len(), 2);
    assert!(added
        .iter()
        .all(|(a, s, d)| a == CLI_ACTOR && *s == subject(&id) && d["os_user"] == "release-bot"));
    let created_by: Option<Uuid> = sqlx::query_scalar("SELECT created_by FROM releases")
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(created_by, None);
}

#[tokio::test]
async fn cli_failures_name_the_problem_and_leave_nothing() {
    let f = fixture().await;
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("wipestation");
    std::fs::write(&path, b"station build").unwrap();
    let good = format!("{LINUX}={}", path.display());
    let missing = format!("{LINUX}={}", files.path().join("absent").display());
    let db = f.hub.db.url.clone();
    let before = f.counts().await;
    for (args, message) in [
        (
            vec!["--version", "0.2", "--notes", "n", "--file", &good],
            "--version",
        ),
        (
            vec!["--version", "0.2.0", "--notes", " ", "--file", &good],
            "--notes",
        ),
        (
            vec![
                "--version",
                "0.2.0",
                "--notes",
                "n",
                "--channel",
                "beta",
                "--file",
                &good,
            ],
            "--channel",
        ),
        (
            vec!["--version", "0.2.0", "--notes", "n", "--file", &missing],
            "cannot read",
        ),
        (
            vec!["--version", "0.2.0", "--notes", "n", "--file", "nonsense"],
            "<platform>=<path>",
        ),
        (vec!["--version", "0.2.0", "--notes", "n"], "--file"),
    ] {
        let out = release_publish(&db, f.dir(), &args);
        assert_failed_with(&out, message);
    }
    // An unreachable database.
    let port = closed_port();
    let out = release_publish(
        &format!("postgres://wipehub:wipehub@127.0.0.1:{port}/wipehub"),
        f.dir(),
        &["--version", "0.2.0", "--notes", "n", "--file", &good],
    );
    assert_failed_with(&out, &format!("127.0.0.1:{port}"));
    assert_eq!(f.counts().await, before);
    assert!(entries(f.dir()).is_empty(), "{:?}", entries(f.dir()));

    // The second file cannot be opened after the first is stored: the
    // first is deleted again.
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = files.path().join("locked.exe");
        std::fs::write(&locked, b"windows build").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads it anyway: nothing to test then.
        if std::fs::File::open(&locked).is_err() {
            let spec = format!("windows-x86_64={}", locked.display());
            let out = release_publish(
                &db,
                f.dir(),
                &[
                    "--version",
                    "0.2.0",
                    "--notes",
                    "n",
                    "--file",
                    &good,
                    "--file",
                    &spec,
                ],
            );
            assert_failed_with(&out, &format!("--file {spec:?}"));
            assert!(entries(f.dir()).is_empty(), "{:?}", entries(f.dir()));
            assert_eq!(f.counts().await, before);
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    // A version already used.
    assert_ok(&release_publish(
        &db,
        f.dir(),
        &["--version", "0.2.0", "--notes", "n", "--file", &good],
    ));
    let files_after_first = entries(f.dir());
    let before = f.counts().await;
    let out = release_publish(
        &db,
        f.dir(),
        &["--version", "0.2.0", "--notes", "again", "--file", &good],
    );
    assert_failed_with(&out, MSG_VERSION_TAKEN);
    assert_eq!(f.counts().await, before);
    assert_eq!(entries(f.dir()), files_after_first);
}
