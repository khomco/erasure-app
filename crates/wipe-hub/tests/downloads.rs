//! Entitled downloads with audit (software-downloads story 2, CAP-2, CAP-3):
//! one test per row of the story's I/O matrix, through HTTP, checking the
//! response, the bytes and headers of a fetched file, the
//! `release_downloads` rows and the audit rows. Needs `docker compose up
//! -d`; fails, never skips, when it is down.

mod support;

use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use reqwest::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS,
};
use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::hub::{
    assert_error, assert_ok, bootstrap_admin, spawn_hub, spawn_hub_config, Hub, Who,
};
use time::OffsetDateTime;
use uuid::Uuid;
use wipe_hub::downloads::{MSG_LINK_INVALID, MSG_NOT_ENTITLED, MSG_NO_DOWNLOAD};
use wipe_hub::release_admin::{MSG_NO_RELEASE, MSG_STORAGE_UNAVAILABLE};

const ADMIN_ONLY: &str = "only vendor admins can use the admin area";
const NOT_SIGNED_IN: &str = "not signed in";
const OCTET: &str = "application/octet-stream";
const LINUX: &str = "linux-x86_64";

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A station "build": distinct bytes per `seed`.
fn build(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
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

async fn org_of(hub: &Hub, who: &Who) -> Uuid {
    sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1::uuid")
        .bind(&who.id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
}

/// `who` buys `offering` and pays on the Fake checkout page: the new
/// entitlement's id (as in `tests/stations.rs`).
async fn buy(hub: &Hub, who: &Who, offering: &str) -> String {
    let v = ok_json(
        hub.post(
            "/api/checkout",
            json!({ "offering_id": offering }),
            Some(&who.cookie),
        )
        .await,
        StatusCode::CREATED,
    )
    .await;
    let order = v["order"]["id"].as_str().unwrap().to_string();
    let url = url::Url::parse(v["checkout_url"].as_str().unwrap()).unwrap();
    let session = url
        .query_pairs()
        .find(|(k, _)| k == "session")
        .unwrap()
        .1
        .into_owned();
    ok_json(
        hub.post_empty(
            &format!("/api/payments/fake/sessions/{session}/pay"),
            Some(&who.cookie),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM entitlements WHERE order_id = $1::uuid")
        .bind(&order)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    id.to_string()
}

struct Fixture {
    hub: Hub,
    admin: Who,
    owner: Who,
    member: Who,
    org_id: Uuid,
    /// The organization's one entitlement (annual, current).
    entitlement: String,
}

async fn fixture_on(hub: Hub) -> Fixture {
    let admin = bootstrap_admin(&hub).await;
    let owner = hub.verified_owner("Acme Recyclers").await;
    let member = hub.add(&owner, "member").await;
    let org_id = org_of(&hub, &owner).await;
    let offering = hub.published_offering("Station — annual, 1 seat").await;
    let entitlement = buy(&hub, &owner, &offering).await;
    Fixture {
        hub,
        admin,
        owner,
        member,
        org_id,
        entitlement,
    }
}

async fn fixture() -> Fixture {
    fixture_on(spawn_hub().await).await
}

impl Fixture {
    async fn admin_post(&self, path: &str, body: Value) -> Value {
        let resp = self.hub.post(path, body, Some(&self.admin.cookie)).await;
        let code = resp.status();
        let v: Value = resp.json().await.unwrap();
        assert!(code.is_success(), "{path}: {code} {v}");
        v
    }

    /// A release with one file per `(platform, bytes)`, in draft: the release.
    async fn draft(&self, version: &str, channel: &str, files: &[(&str, &[u8])]) -> Value {
        let r = self
            .admin_post(
                "/api/admin/releases",
                json!({"version": version, "notes": format!("Notes for {version}."), "channel": channel}),
            )
            .await["release"]
            .clone();
        let id = id_of(&r);
        for (platform, bytes) in files {
            let resp = self
                .hub
                .post_req(
                    &format!(
                        "/api/admin/releases/{id}/artifacts?platform={platform}&filename=wipestation-{version}"
                    ),
                    Some(&self.admin.cookie),
                )
                .header(CONTENT_TYPE, OCTET)
                .body(bytes.to_vec())
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
        }
        self.release(&id).await
    }

    async fn release(&self, id: &str) -> Value {
        ok_json(
            self.hub
                .get(
                    &format!("/api/admin/releases/{id}"),
                    Some(&self.admin.cookie),
                )
                .await,
            StatusCode::OK,
        )
        .await["release"]
            .clone()
    }

    async fn publish(&self, id: &str) -> Value {
        self.admin_post(&format!("/api/admin/releases/{id}/publish"), json!({}))
            .await["release"]
            .clone()
    }

    async fn withdraw(&self, id: &str) -> Value {
        self.admin_post(&format!("/api/admin/releases/{id}/withdraw"), json!({}))
            .await["release"]
            .clone()
    }

    /// A published release with one Linux file of `bytes`: (release, artifact).
    async fn published(&self, version: &str, bytes: &[u8]) -> (Value, Value) {
        let r = self.draft(version, "stable", &[(LINUX, bytes)]).await;
        let r = self.publish(&id_of(&r)).await;
        let a = r["artifacts"][0].clone();
        (r, a)
    }

    async fn list(&self, who: Option<&Who>) -> Response {
        self.hub
            .get("/api/downloads", who.map(|w| w.cookie.as_str()))
            .await
    }

    async fn link_resp(&self, who: Option<&Who>, artifact: &str) -> Response {
        self.hub
            .post_empty(
                &format!("/api/downloads/{artifact}/link"),
                who.map(|w| w.cookie.as_str()),
            )
            .await
    }

    /// A link for `who` to `artifact`, expecting 200: the body.
    async fn link(&self, who: &Who, artifact: &str) -> Value {
        ok_json(self.link_resp(Some(who), artifact).await, StatusCode::OK).await
    }

    /// GET a link with no cookie.
    async fn fetch(&self, url: &str) -> Response {
        self.hub.get(url, None).await
    }

    async fn downloads_rows(&self) -> i64 {
        self.hub.count("release_downloads").await
    }

    /// (release_downloads rows, audit rows): what a refused request leaves alone.
    async fn written(&self) -> (i64, i64) {
        (
            self.downloads_rows().await,
            self.hub.count("audit_log").await,
        )
    }

    async fn refund(&self) {
        sqlx::query(
            "UPDATE entitlements SET status = 'refunded', refunded_at = now(), renews = false \
             WHERE id = $1::uuid",
        )
        .bind(&self.entitlement)
        .execute(&self.hub.db.pool)
        .await
        .unwrap();
    }

    async fn admin_downloads(&self, release: &str, who: Option<&Who>) -> Response {
        self.hub
            .get(
                &format!("/api/admin/releases/{release}/downloads"),
                who.map(|w| w.cookie.as_str()),
            )
            .await
    }
}

/// The query field `key` of a path-relative URL.
fn query_field(url: &str, key: &str) -> String {
    let u = url::Url::parse(&format!("http://x{url}")).unwrap();
    u.query_pairs()
        .find(|(k, _)| k == key)
        .unwrap_or_else(|| panic!("{key} in {url}"))
        .1
        .into_owned()
}

/// `url` with query field `key` set to `value`.
fn with_field(url: &str, key: &str, value: &str) -> String {
    let mut u = url::Url::parse(&format!("http://x{url}")).unwrap();
    let pairs: Vec<(String, String)> = u
        .query_pairs()
        .map(|(k, v)| {
            let v = if k == key {
                value.to_string()
            } else {
                v.into_owned()
            };
            (k.into_owned(), v)
        })
        .collect();
    u.query_pairs_mut().clear().extend_pairs(pairs);
    format!("{}?{}", u.path(), u.query().unwrap())
}

// ---------------------------------------------------------------------------
// List

#[tokio::test]
async fn entitled_owner_and_member_list_published_releases_only_newest_first() {
    let f = fixture().await;
    let older = build(1, 100);
    let (first, _) = f.published("0.1.0", &older).await;
    let win = build(2, 50);
    let linux = build(3, 70);
    let second = f
        .draft(
            "0.2.0-rc.1",
            "preview",
            &[("windows-x86_64", &win), (LINUX, &linux)],
        )
        .await;
    let second = f.publish(&id_of(&second)).await;
    let draft = f.draft("0.3.0", "stable", &[(LINUX, &build(4, 10))]).await;
    let (gone, _) = f.published("0.0.9", &build(5, 10)).await;
    f.withdraw(&id_of(&gone)).await;
    let before = f.written().await;

    for who in [&f.owner, &f.member] {
        let body = ok_json(f.list(Some(who)).await, StatusCode::OK).await;
        let releases = body["releases"].as_array().unwrap();
        let ids: Vec<String> = releases.iter().map(id_of).collect();
        // Newest publication first; no draft, no withdrawn release.
        assert_eq!(ids, [id_of(&second), id_of(&first)]);
        assert!(!body.to_string().contains(&id_of(&draft)));
        assert!(!body.to_string().contains(&id_of(&gone)));
        let r = &releases[0];
        assert_eq!(r["version"], "0.2.0-rc.1");
        assert_eq!(r["channel"], "preview");
        assert_eq!(r["notes"], "Notes for 0.2.0-rc.1.");
        assert_eq!(r["published_at"], second["published_at"]);
        let keys: Vec<&String> = r.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "artifacts",
                "channel",
                "id",
                "notes",
                "published_at",
                "version"
            ]
        );
        // Files by platform, each with exactly these fields.
        let files = r["artifacts"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[0],
            json!({
                "id": second["artifacts"][0]["id"],
                "platform": LINUX,
                "filename": "wipestation-0.2.0-rc.1",
                "size_bytes": 70,
                "sha256": sha256_hex(&linux),
            })
        );
        assert_eq!(files[1]["platform"], "windows-x86_64");
        assert_eq!(files[1]["sha256"], sha256_hex(&win));
        assert_eq!(releases[1]["channel"], "stable");
        assert_eq!(releases[1]["artifacts"][0]["sha256"], sha256_hex(&older));
    }
    // Listing is not audited.
    assert_eq!(f.written().await, before);
}

#[tokio::test]
async fn an_organization_without_a_current_entitlement_cannot_list() {
    let f = fixture().await;
    let (_, a) = f.published("1.0.0", &build(1, 10)).await;

    // No entitlement at all: a pending order only.
    let other = f.hub.verified_owner("Pending Org").await;
    let offering = f.hub.published_offering("Other annual").await;
    ok_json(
        f.hub
            .post(
                "/api/checkout",
                json!({ "offering_id": offering }),
                Some(&other.cookie),
            )
            .await,
        StatusCode::CREATED,
    )
    .await;
    // Term ended: a second organization whose entitlement moved into the
    // past (the trigger refuses this to the hub, so it is lifted here).
    let ended = f.hub.verified_owner("Ended Org").await;
    let ended_ent = buy(&f.hub, &ended, &offering).await;
    let pool = &f.hub.db.pool;
    sqlx::query("ALTER TABLE entitlements DISABLE TRIGGER entitlements_lifecycle")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE entitlements SET term_start = term_start - interval '2 years', \
                term_end = term_end - interval '2 years' WHERE id = $1::uuid",
    )
    .bind(&ended_ent)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE entitlements ENABLE TRIGGER entitlements_lifecycle")
        .execute(pool)
        .await
        .unwrap();

    let before = f.written().await;
    for who in [&other, &ended, &f.admin] {
        assert_error(
            f.list(Some(who)).await,
            StatusCode::FORBIDDEN,
            MSG_NOT_ENTITLED,
        )
        .await;
        // Nor can they get a link, the vendor admin included.
        assert_error(
            f.link_resp(Some(who), &id_of(&a)).await,
            StatusCode::FORBIDDEN,
            MSG_NOT_ENTITLED,
        )
        .await;
    }
    // Only refunded: the fixture organization's one entitlement.
    f.refund().await;
    for who in [&f.owner, &f.member] {
        assert_error(
            f.list(Some(who)).await,
            StatusCode::FORBIDDEN,
            MSG_NOT_ENTITLED,
        )
        .await;
    }
    assert_eq!(f.written().await, before);
}

#[tokio::test]
async fn a_cancelled_entitlement_stays_entitled_until_its_term_ends() {
    let f = fixture().await;
    f.published("1.0.0", &build(1, 10)).await;
    sqlx::query("UPDATE entitlements SET status = 'cancelled', cancelled_at = now(), renews = false WHERE id = $1::uuid")
        .bind(&f.entitlement)
        .execute(&f.hub.db.pool)
        .await
        .unwrap();
    let body = ok_json(f.list(Some(&f.member)).await, StatusCode::OK).await;
    assert_eq!(body["releases"].as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Links

#[tokio::test]
async fn an_entitled_user_gets_a_five_minute_link() {
    let f = fixture().await;
    let (_, a) = f.published("1.0.0", &build(1, 10)).await;
    let before = f.written().await;
    let start = OffsetDateTime::now_utc().unix_timestamp();
    let body = f.link(&f.member, &id_of(&a)).await;
    let end = OffsetDateTime::now_utc().unix_timestamp();
    let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["expires_at", "url"]);
    let url = body["url"].as_str().unwrap();
    assert!(
        url.starts_with(&format!("/api/downloads/{}/file?", id_of(&a))),
        "{url}"
    );
    assert_eq!(query_field(url, "user"), f.member.id);
    let expires: i64 = query_field(url, "expires").parse().unwrap();
    assert!(
        (start + 300..=end + 300).contains(&expires),
        "{expires} {start}"
    );
    let at = OffsetDateTime::parse(
        body["expires_at"].as_str().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert_eq!(at.unix_timestamp(), expires);
    assert!(body["expires_at"].as_str().unwrap().ends_with('Z'));
    // Issuing a link is not audited and records no download.
    assert_eq!(f.written().await, before);
}

#[tokio::test]
async fn links_to_draft_withdrawn_unknown_or_malformed_artifacts_are_404() {
    let f = fixture().await;
    let draft = f.draft("1.0.0", "stable", &[(LINUX, &build(1, 10))]).await;
    let (gone, gone_a) = f.published("1.1.0", &build(2, 10)).await;
    f.withdraw(&id_of(&gone)).await;
    let before = f.written().await;
    for artifact in [
        id_of(&draft["artifacts"][0]),
        id_of(&gone_a),
        Uuid::new_v4().to_string(),
        "not-a-uuid".to_string(),
    ] {
        assert_error(
            f.link_resp(Some(&f.owner), &artifact).await,
            StatusCode::NOT_FOUND,
            MSG_NO_DOWNLOAD,
        )
        .await;
    }
    // Entitlement is checked before the artifact id is looked at.
    f.refund().await;
    for artifact in ["not-a-uuid".to_string(), id_of(&gone_a)] {
        assert_error(
            f.link_resp(Some(&f.owner), &artifact).await,
            StatusCode::FORBIDDEN,
            MSG_NOT_ENTITLED,
        )
        .await;
    }
    assert_eq!(f.written().await, before);
}

#[tokio::test]
async fn signed_out_list_and_link_are_401() {
    let f = fixture().await;
    let (_, a) = f.published("1.0.0", &build(1, 10)).await;
    assert_error(f.list(None).await, StatusCode::UNAUTHORIZED, NOT_SIGNED_IN).await;
    assert_error(
        f.link_resp(None, &id_of(&a)).await,
        StatusCode::UNAUTHORIZED,
        NOT_SIGNED_IN,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Fetch

#[tokio::test]
async fn a_link_fetches_the_exact_file_and_records_the_download() {
    let f = fixture().await;
    let bytes = build(7, 300_000);
    let (r, a) = f.published("1.0.0", &bytes).await;
    let url = f.link(&f.member, &id_of(&a)).await["url"]
        .as_str()
        .unwrap()
        .to_string();
    let audit_before = f.hub.count("audit_log").await;

    let resp = f.fetch(&url).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let h = resp.headers().clone();
    assert_eq!(h[CONTENT_TYPE], OCTET);
    assert_eq!(
        h[CONTENT_DISPOSITION],
        "attachment; filename=\"wipestation-1.0.0\""
    );
    assert_eq!(h[CONTENT_LENGTH], bytes.len().to_string().as_str());
    assert_eq!(h[CACHE_CONTROL], "no-store");
    assert_eq!(h[X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(h[REFERRER_POLICY], "no-referrer");
    let body = resp.bytes().await.unwrap();
    assert_eq!(body.as_ref(), bytes.as_slice());
    assert_eq!(sha256_hex(&body), a["sha256"].as_str().unwrap());

    // One download row and one audit row.
    let rows: Vec<(Uuid, Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT release_id, artifact_id, organization_id, user_id FROM release_downloads",
    )
    .fetch_all(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [(
            Uuid::parse_str(&id_of(&r)).unwrap(),
            Uuid::parse_str(&id_of(&a)).unwrap(),
            f.org_id,
            Uuid::parse_str(&f.member.id).unwrap(),
        )]
    );
    // No email or other personal data is copied into the append-only table.
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns \
         WHERE table_name = 'release_downloads' ORDER BY ordinal_position",
    )
    .fetch_all(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(
        columns,
        [
            "id",
            "release_id",
            "artifact_id",
            "organization_id",
            "user_id",
            "downloaded_at"
        ]
    );
    assert_eq!(f.hub.count("audit_log").await, audit_before + 1);
    assert_eq!(
        f.hub.audited("release.downloaded").await,
        [(
            format!("user:{}", f.member.id),
            format!("release:{}", id_of(&r)),
            json!({
                "artifact_id": id_of(&a),
                "platform": LINUX,
                "filename": "wipestation-1.0.0",
                "sha256": sha256_hex(&bytes),
                "organization_id": f.org_id,
            }),
        )]
    );

    // The same link again before it expires: another download each.
    let again = f.fetch(&url).await;
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(again.bytes().await.unwrap().as_ref(), bytes.as_slice());
    assert_eq!(f.downloads_rows().await, 2);
    assert_eq!(f.hub.audited("release.downloaded").await.len(), 2);
}

#[tokio::test]
async fn a_missing_tampered_or_expired_link_is_403_and_writes_nothing() {
    let f = fixture().await;
    let (_, a) = f.published("1.0.0", &build(1, 100)).await;
    let (_, other) = f.published("1.1.0", &build(2, 100)).await;
    let url = f.link(&f.member, &id_of(&a)).await["url"]
        .as_str()
        .unwrap()
        .to_string();
    let sig = query_field(&url, "sig");
    let expires: i64 = query_field(&url, "expires").parse().unwrap();
    let mut flipped = sig.clone().into_bytes();
    flipped[3] = if flipped[3] == b'A' { b'B' } else { b'A' };
    let flipped = String::from_utf8(flipped).unwrap();
    let path = format!("/api/downloads/{}/file", id_of(&a));
    let before = f.written().await;

    for bad in [
        path.clone(),
        format!("{path}?"),
        with_field(&url, "sig", &flipped),
        with_field(&url, "sig", "not base64!"),
        with_field(&url, "user", &f.owner.id),
        with_field(&url, "user", "nobody"),
        with_field(&url, "expires", &(expires + 3600).to_string()),
        with_field(&url, "expires", "soon"),
        url.replace(&id_of(&a), &id_of(&other)),
        url.replace(&id_of(&a), "not-a-uuid"),
        format!("{url}&sig={sig}"),
    ] {
        assert_error(f.fetch(&bad).await, StatusCode::FORBIDDEN, MSG_LINK_INVALID).await;
    }
    assert_eq!(f.written().await, before);

    // A link with no lifetime is already expired.
    let zero =
        fixture_on(spawn_hub_config(|_| {}, |c| c.download_link_ttl = Duration::ZERO).await).await;
    let (_, a) = zero.published("1.0.0", &build(1, 10)).await;
    let url = zero.link(&zero.owner, &id_of(&a)).await["url"]
        .as_str()
        .unwrap()
        .to_string();
    let before = zero.written().await;
    assert_error(
        zero.fetch(&url).await,
        StatusCode::FORBIDDEN,
        MSG_LINK_INVALID,
    )
    .await;
    assert_eq!(zero.written().await, before);
}

#[tokio::test]
async fn a_link_stops_working_once_the_entitlement_lapses_or_the_user_is_removed() {
    let f = fixture().await;
    let (_, a) = f.published("1.0.0", &build(1, 10)).await;
    let member_url = f.link(&f.member, &id_of(&a)).await["url"]
        .as_str()
        .unwrap()
        .to_string();
    let owner_url = f.link(&f.owner, &id_of(&a)).await["url"]
        .as_str()
        .unwrap()
        .to_string();

    // The member is removed from the team: their links are void.
    let resp = f.hub.remove(&f.owner, &f.member.id).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let before = f.written().await;
    assert_error(
        f.fetch(&member_url).await,
        StatusCode::FORBIDDEN,
        MSG_LINK_INVALID,
    )
    .await;
    assert_eq!(f.written().await, before);

    // The entitlement is refunded: the owner's link is refused too.
    f.refund().await;
    assert_error(
        f.fetch(&owner_url).await,
        StatusCode::FORBIDDEN,
        MSG_NOT_ENTITLED,
    )
    .await;
    assert_eq!(f.written().await, before);
}

#[tokio::test]
async fn a_link_to_a_release_withdrawn_since_is_404() {
    let f = fixture().await;
    let (r, a) = f.published("1.0.0", &build(1, 10)).await;
    let url = f.link(&f.owner, &id_of(&a)).await["url"]
        .as_str()
        .unwrap()
        .to_string();
    f.withdraw(&id_of(&r)).await;
    let before = f.written().await;
    assert_error(f.fetch(&url).await, StatusCode::NOT_FOUND, MSG_NO_DOWNLOAD).await;
    assert_eq!(f.written().await, before);
}

#[tokio::test]
async fn a_withdrawal_racing_a_fetch_wins_and_nothing_is_written() {
    let f = fixture().await;
    let (r, a) = f.published("1.0.0", &build(1, 10)).await;
    let url = f.link(&f.owner, &id_of(&a)).await["url"]
        .as_str()
        .unwrap()
        .to_string();
    let before = f.written().await;

    // A withdrawal in progress: the release row is locked and changed, not
    // yet committed.
    let mut tx = f.hub.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM releases WHERE id = $1::uuid FOR UPDATE")
        .bind(id_of(&r))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE releases SET status = 'withdrawn', withdrawn_at = now() WHERE id = $1::uuid",
    )
    .bind(id_of(&r))
    .execute(&mut *tx)
    .await
    .unwrap();

    // The fetch starts and must wait on the release row's lock.
    let fetch = {
        let http = f.hub.http.clone();
        let full = f.hub.url(&url);
        tokio::spawn(async move { http.get(full).send().await.unwrap() })
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
        if waiting > 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the fetch never waited on the release lock"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tx.commit().await.unwrap();

    // The share lock re-reads the release once the withdrawal commits.
    assert_error(fetch.await.unwrap(), StatusCode::NOT_FOUND, MSG_NO_DOWNLOAD).await;
    assert_eq!(f.written().await, before);
}

#[tokio::test]
async fn a_head_probe_of_a_valid_link_is_405_and_writes_nothing() {
    let f = fixture().await;
    let (_, a) = f.published("1.0.0", &build(1, 10)).await;
    let url = f.link(&f.owner, &id_of(&a)).await["url"]
        .as_str()
        .unwrap()
        .to_string();
    let before = f.written().await;
    let resp = f.hub.http.head(f.hub.url(&url)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(f.written().await, before);
    // Every other method is refused too.
    let resp = f.hub.post_empty(&url, None).await;
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(f.written().await, before);
    // The link itself is still good.
    assert_eq!(f.fetch(&url).await.status(), StatusCode::OK);
    assert_eq!(f.downloads_rows().await, before.0 + 1);
}

#[tokio::test]
async fn a_stored_file_gone_from_the_artifact_directory_is_503() {
    let f = fixture().await;
    let (_, a) = f.published("1.0.0", &build(1, 10)).await;
    let url = f.link(&f.owner, &id_of(&a)).await["url"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::remove_file(f.hub.artifacts.path().join(id_of(&a))).unwrap();
    let before = f.written().await;
    assert_error(
        f.fetch(&url).await,
        StatusCode::SERVICE_UNAVAILABLE,
        MSG_STORAGE_UNAVAILABLE,
    )
    .await;
    assert_eq!(f.written().await, before);
}

#[tokio::test]
async fn a_link_is_a_bearer_credential_recorded_as_its_own_user() {
    let f = fixture().await;
    let bytes = build(9, 64);
    let (_, a) = f.published("1.0.0", &bytes).await;
    let url = f.link(&f.member, &id_of(&a)).await["url"]
        .as_str()
        .unwrap()
        .to_string();
    // Organization B's owner (itself entitled) fetches A's member's link,
    // with B's session cookie: recorded as A's member, in A's organization.
    let b = f.hub.verified_owner("Bravo Holdings").await;
    let offering = f.hub.published_offering("Bravo annual").await;
    buy(&f.hub, &b, &offering).await;
    let resp = f.hub.get(&url, Some(&b.cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), bytes.as_slice());
    let rows: Vec<(Uuid, Uuid)> =
        sqlx::query_as("SELECT user_id, organization_id FROM release_downloads")
            .fetch_all(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(rows, [(Uuid::parse_str(&f.member.id).unwrap(), f.org_id)]);
    let audited = f.hub.audited("release.downloaded").await;
    assert_eq!(audited[0].0, format!("user:{}", f.member.id));
}

// ---------------------------------------------------------------------------
// Admin view

#[tokio::test]
async fn admins_see_each_releases_downloads_newest_first_audited() {
    let f = fixture().await;
    let (r, a) = f.published("1.0.0", &build(1, 10)).await;
    let (other, other_a) = f.published("1.1.0", &build(2, 10)).await;
    for (who, artifact) in [(&f.owner, &a), (&f.member, &a), (&f.owner, &other_a)] {
        let url = f.link(who, &id_of(artifact)).await["url"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(f.fetch(&url).await.status(), StatusCode::OK);
    }
    let org_name = f.hub.org_name(&f.owner).await;

    let viewed_before = f.hub.audited("admin.release_downloads_viewed").await.len();
    let body = ok_json(
        f.admin_downloads(&id_of(&r), Some(&f.admin)).await,
        StatusCode::OK,
    )
    .await;
    let rows = body["downloads"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{body}");
    // Newest first: the member's download came second.
    assert_eq!(rows[0]["user_email"], f.member.email.as_str());
    assert_eq!(rows[1]["user_email"], f.owner.email.as_str());
    let row = &rows[0];
    let keys: Vec<&String> = row.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "artifact_id",
            "downloaded_at",
            "filename",
            "id",
            "organization_id",
            "organization_name",
            "platform",
            "user_email",
            "user_id",
        ]
    );
    assert_eq!(row["artifact_id"], a["id"]);
    assert_eq!(row["platform"], LINUX);
    assert_eq!(row["filename"], "wipestation-1.0.0");
    assert_eq!(row["user_id"], f.member.id.as_str());
    assert_eq!(row["organization_id"], f.org_id.to_string());
    assert_eq!(row["organization_name"], org_name.as_str());
    assert!(row["downloaded_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(
        f.hub.audited("admin.release_downloads_viewed").await[viewed_before..],
        [(
            format!("user:{}", f.admin.id),
            format!("release:{}", id_of(&r)),
            json!({}),
        )]
    );

    // Any status: still listed after withdrawal, and a removed member's
    // download stays recorded by id, with no email once they are gone.
    f.withdraw(&id_of(&r)).await;
    assert_eq!(
        f.hub.remove(&f.owner, &f.member.id).await.status(),
        StatusCode::NO_CONTENT
    );
    let body = ok_json(
        f.admin_downloads(&id_of(&r), Some(&f.admin)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body["downloads"].as_array().unwrap().len(), 2);
    assert_eq!(body["downloads"][0]["user_id"], f.member.id.as_str());
    assert_eq!(body["downloads"][0]["user_email"], Value::Null);
    assert_eq!(
        body["downloads"][0]["organization_id"],
        f.org_id.to_string()
    );
    assert_eq!(body["downloads"][1]["user_email"], f.owner.email.as_str());
    let body = ok_json(
        f.admin_downloads(&id_of(&other), Some(&f.admin)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body["downloads"].as_array().unwrap().len(), 1);

    // Unknown or non-UUID: 404, nothing audited.
    let before = f.hub.count("audit_log").await;
    for id in [Uuid::new_v4().to_string(), "nope".to_string()] {
        assert_error(
            f.admin_downloads(&id, Some(&f.admin)).await,
            StatusCode::NOT_FOUND,
            MSG_NO_RELEASE,
        )
        .await;
    }
    // A customer is refused; no session is 401.
    assert_error(
        f.admin_downloads(&id_of(&r), Some(&f.owner)).await,
        StatusCode::FORBIDDEN,
        ADMIN_ONLY,
    )
    .await;
    assert_error(
        f.admin_downloads(&id_of(&r), None).await,
        StatusCode::UNAUTHORIZED,
        NOT_SIGNED_IN,
    )
    .await;
    assert_eq!(f.hub.count("audit_log").await, before);
}

// ---------------------------------------------------------------------------
// Acceptance: a CLI-published build, downloaded by an entitled member

/// `wipe-hub release publish` against the test database and artifact
/// directory (as in `tests/releases.rs`).
fn release_publish(db_url: &str, artifact_dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wipe-hub"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("WIPE_HUB_") {
            cmd.env_remove(&key);
        }
    }
    cmd.env("RUST_LOG", "warn")
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

#[tokio::test]
async fn a_cli_published_build_downloads_with_a_matching_checksum() {
    let f = fixture().await;
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("wipestation");
    let bytes = build(42, 500_000);
    std::fs::write(&path, &bytes).unwrap();
    let out = release_publish(
        &f.hub.db.url,
        f.hub.artifacts.path(),
        &[
            "--version",
            "0.2.0",
            "--notes",
            "First build",
            "--file",
            &format!("{LINUX}={}", path.display()),
        ],
    );
    assert_ok(&out);

    let list = ok_json(f.list(Some(&f.member)).await, StatusCode::OK).await;
    let release = &list["releases"][0];
    let file = &release["artifacts"][0];
    let url = f.link(&f.member, &id_of(file)).await["url"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = f.fetch(&url).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.bytes().await.unwrap();
    let on_disk = sha256_hex(&std::fs::read(&path).unwrap());
    assert_eq!(sha256_hex(&body), file["sha256"].as_str().unwrap());
    assert_eq!(sha256_hex(&body), on_disk);

    let admin = ok_json(
        f.admin_downloads(&id_of(release), Some(&f.admin)).await,
        StatusCode::OK,
    )
    .await;
    let row = &admin["downloads"][0];
    assert_eq!(row["user_email"], f.member.email.as_str());
    assert_eq!(row["organization_id"], f.org_id.to_string());
    assert_eq!(row["artifact_id"], file["id"]);
    assert_eq!(row["filename"], "wipestation");
}
