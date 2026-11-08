//! Vendor-admin licence issue, inspection and revocation (licence-delivery
//! story 3, CAP-4): one test per row of the story's I/O matrix, through
//! HTTP, checking the response, the stored rows and the audit rows. A
//! vendor-issued file is installed the way `wipestation serve --license …
//! --vendor-root-b64 …` installs it. Needs `docker compose up -d`; fails,
//! never skips, when it is down.

mod support;

use reqwest::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{assert_error, bootstrap_admin, spawn_hub, spawn_hub_with_root, Hub, Who};
use time::OffsetDateTime;
use uuid::Uuid;
use wipe_hub::admin_routes::{MSG_ADMIN_ONLY, MSG_NO_ORGANIZATION};
use wipe_hub::licence_admin::MSG_NOT_A_LICENCE;
use wipe_hub::stations::{
    MSG_ALREADY_REVOKED, MSG_BAD_INSTANCE_KEY, MSG_ISSUANCE_UNAVAILABLE, MSG_LICENCE_REVOKED,
    MSG_NO_LICENCE, MSG_STATION_ID_REQUIRED,
};
use wipe_license::{
    check_installable, evaluate, machine_fingerprint, AllowedMethods, Entitlements, Feature,
    LeaseState, LeaseStatus, LicenseCertificate, Quota, Scope, VendorRoot,
};

struct Fixture {
    hub: Hub,
    root: VendorRoot,
    admin: Who,
    owner: Who,
    member: Who,
    org_id: Uuid,
    /// Annual, one machine seat, unlimited, `hub_sync` + `pdf_certificates`.
    machine: String,
}

fn new_key() -> String {
    wipe_cert::SigningKey::generate().public_key_id()
}

async fn ok_json(resp: Response, want: StatusCode) -> Value {
    let code = resp.status();
    let v: Value = resp.json().await.unwrap();
    assert_eq!(code, want, "{v}");
    v
}

/// `who` buys `offering` and pays on the Fake checkout page: the new
/// entitlement's id.
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

async fn fixture() -> Fixture {
    let (hub, root) = spawn_hub_with_root().await;
    let admin = bootstrap_admin(&hub).await;
    let owner = hub.verified_owner("Acme Recyclers").await;
    let member = hub.add(&owner, "member").await;
    let org_id: Uuid = sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1::uuid")
        .bind(&owner.id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    let annual = hub.published_offering("Station — annual, 1 seat").await;
    let machine = buy(&hub, &owner, &annual).await;
    Fixture {
        hub,
        root,
        admin,
        owner,
        member,
        org_id,
        machine,
    }
}

/// A good issue body: a 30-day machine licence for `bench-1`.
fn machine_body(key: &str) -> Value {
    json!({
        "instance_key_id": key,
        "scope": "machine",
        "station_id": "bench-1",
        "quota_erasures": null,
        "features": ["pdf_certificates", "hub_sync"],
        "days": 30,
        "reason": "30-day evaluation agreed with Acme",
    })
}

/// Install `file` the way `wipestation serve --license <file>
/// --vendor-root-b64 <root>` does, on a station whose signing key is `key`.
fn install(file: &str, key: &str, root: &VendorRoot) -> LicenseCertificate {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("licence.json");
    std::fs::write(&path, file).unwrap();
    let licence = wipe_license::load_license(&path).unwrap();
    use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine as _};
    let bytes: [u8; 32] = B64
        .decode(root.verifying_key().to_base64())
        .unwrap()
        .try_into()
        .unwrap();
    let published = wipe_cert::VerifyingKey::from_bytes(&bytes).unwrap();
    check_installable(&licence, key, &[published]).unwrap();
    licence
}

impl Fixture {
    async fn issue(&self, org: &str, body: Value) -> Response {
        self.hub
            .post(
                &format!("/api/admin/organizations/{org}/licences"),
                body,
                Some(&self.admin.cookie),
            )
            .await
    }

    /// The admin issues a 30-day machine licence for `key`: its record.
    async fn issue_machine(&self, key: &str) -> Value {
        ok_json(
            self.issue(&self.org_id.to_string(), machine_body(key))
                .await,
            StatusCode::CREATED,
        )
        .await["licence"]
            .clone()
    }

    async fn admin_get(&self, path: &str) -> Response {
        self.hub.get(path, Some(&self.admin.cookie)).await
    }

    async fn inspect(&self, body: Value) -> Response {
        self.hub
            .post(
                "/api/admin/licences/inspect",
                body,
                Some(&self.admin.cookie),
            )
            .await
    }

    async fn revoke(&self, licence: &str, reason: &str) -> Response {
        self.hub
            .post(
                &format!("/api/admin/licences/{licence}/revoke"),
                json!({ "reason": reason }),
                Some(&self.admin.cookie),
            )
            .await
    }

    /// The owner registers `key` on the machine entitlement as `bench-1`.
    async fn register(&self, key: &str) -> (String, String) {
        let v = ok_json(
            self.hub
                .post(
                    "/api/stations",
                    json!({
                        "entitlement_id": self.machine,
                        "instance_key_id": key,
                        "station_id": "bench-1",
                        "label": "Bench 1",
                    }),
                    Some(&self.owner.cookie),
                )
                .await,
            StatusCode::CREATED,
        )
        .await;
        (
            v["station"]["id"].as_str().unwrap().to_string(),
            v["licence"]["id"].as_str().unwrap().to_string(),
        )
    }

    async fn stations(&self, who: &Who) -> Vec<Value> {
        ok_json(
            self.hub.get("/api/stations", Some(&who.cookie)).await,
            StatusCode::OK,
        )
        .await["stations"]
            .as_array()
            .unwrap()
            .clone()
    }

    async fn stored_document(&self, licence: &str) -> String {
        sqlx::query_scalar("SELECT document FROM licences WHERE id = $1::uuid")
            .bind(licence)
            .fetch_one(&self.hub.db.pool)
            .await
            .unwrap()
    }

    async fn extend_term(&self) {
        sqlx::query(
            "UPDATE entitlements SET term_end = term_end + interval '1 year' \
             WHERE id = $1::uuid",
        )
        .bind(&self.machine)
        .execute(&self.hub.db.pool)
        .await
        .unwrap();
    }

    /// Rows a refused request must leave unchanged.
    async fn counts(&self) -> (i64, i64, i64) {
        (
            self.hub.count("stations").await,
            self.hub.count("licences").await,
            self.hub.count("audit_log").await,
        )
    }

    fn actor(&self) -> String {
        format!("user:{}", self.admin.id)
    }
}

// ---------------------------------------------------------------------------
// Issue

#[tokio::test]
async fn a_vendor_issued_machine_licence_installs_and_uses_no_seat() {
    let f = fixture().await;
    let seats_before = ok_json(
        f.hub.get("/api/entitlements", Some(&f.owner.cookie)).await,
        StatusCode::OK,
    )
    .await;
    let key = new_key();
    let before = OffsetDateTime::now_utc();
    let licence = f.issue_machine(&key).await;
    let id = licence["id"].as_str().unwrap();
    assert_eq!(licence["source"], "vendor");
    assert_eq!(licence["status"], "active");
    assert_eq!(licence["station"], Value::Null);
    assert_eq!(licence["entitlement_id"], Value::Null);
    assert_eq!(
        licence["organization"],
        json!({ "id": f.org_id, "name": "Acme Recyclers" })
    );
    assert_eq!(licence["instance_key_id"], json!(key));
    assert_eq!(
        licence["issue_reason"],
        "30-day evaluation agreed with Acme"
    );
    assert_eq!(licence["revocation_reason"], Value::Null);
    assert_eq!(licence["revoked_at"], Value::Null);

    // The admin downloads it with the customer route's headers.
    let resp = f
        .admin_get(&format!("/api/admin/licences/{id}/download"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(
        resp.headers()[CONTENT_DISPOSITION],
        format!("attachment; filename=\"licence-{id}.json\"").as_str()
    );
    assert_eq!(resp.headers()[CACHE_CONTROL], "no-store");
    let file = resp.text().await.unwrap();
    assert_eq!(file, f.stored_document(id).await);

    // It installs and is valid on `bench-1`, named for the organization.
    let cert = install(&file, &key, &f.root);
    assert_eq!(file, serde_json::to_string_pretty(&cert).unwrap());
    let body = &cert.body;
    assert_eq!(body.license_id, id);
    let ent = &body.entitlements;
    assert_eq!(ent.customer_id, f.org_id.to_string());
    assert_eq!(ent.customer_name, "Acme Recyclers");
    assert_eq!(ent.quota, Quota::Unlimited);
    assert_eq!(
        ent.scope,
        Scope::Machine {
            fingerprint: machine_fingerprint("bench-1")
        }
    );
    assert_eq!(ent.features, [Feature::HubSync, Feature::PdfCertificates]);
    assert_eq!(ent.allowed_methods, AllowedMethods::All);
    assert_eq!(ent.machine_binding, None);
    assert_eq!(ent.not_before, body.issued_at);
    assert!(body.issued_at >= before - time::Duration::seconds(1));
    assert_eq!(ent.not_after, body.issued_at + time::Duration::days(30));
    assert_eq!(body.issued_at.nanosecond() % 1_000, 0);
    let now = OffsetDateTime::now_utc();
    assert!(matches!(
        evaluate(
            Some(ent),
            &LeaseState::new(now),
            &machine_fingerprint("bench-1"),
            now
        ),
        LeaseStatus::Valid {
            remaining_erasures: None,
            ..
        }
    ));

    // No station, no seat: the customer's seats and stations are unchanged.
    assert_eq!(f.hub.count("stations").await, 0);
    assert!(f.stations(&f.owner).await.is_empty());
    let seats_after = ok_json(
        f.hub.get("/api/entitlements", Some(&f.owner.cookie)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(seats_before, seats_after);
    // The customer cannot download a vendor-issued file: it reaches them
    // through the vendor.
    assert_error(
        f.hub
            .get(
                &format!("/api/licences/{id}/download"),
                Some(&f.owner.cookie),
            )
            .await,
        StatusCode::NOT_FOUND,
        MSG_NO_LICENCE,
    )
    .await;

    // Stored as vendor, with its reason.
    let (source, reason): (String, Option<String>) =
        sqlx::query_as("SELECT source, issue_reason FROM licences WHERE id = $1::uuid")
            .bind(id)
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(source, "vendor");
    assert_eq!(
        reason.as_deref(),
        Some("30-day evaluation agreed with Acme")
    );

    // Audited.
    assert_eq!(
        f.hub.audited("licence.issued").await,
        [(
            f.actor(),
            format!("licence:{id}"),
            json!({
                "organization_id": f.org_id,
                "instance_key_id": key,
                "root_key_id": f.root.public_key_id(),
                "not_after": licence["not_after"],
                "source": "vendor",
                "scope": { "kind": "machine", "fingerprint": machine_fingerprint("bench-1") },
                "quota_erasures": null,
                "features": ["hub_sync", "pdf_certificates"],
                "reason": "30-day evaluation agreed with Acme",
                "lapse_policy": "evaluation",
            }),
        )]
    );
    assert_eq!(
        f.hub.audited("licence.downloaded").await,
        [(
            f.actor(),
            format!("licence:{id}"),
            json!({ "organization_id": f.org_id, "by": "vendor" }),
        )]
    );
}

#[tokio::test]
async fn a_site_licence_names_the_site_or_the_organization() {
    let f = fixture().await;
    let org = f.org_id.to_string();
    for (site, want) in [
        (json!("  "), org.clone()),
        (json!(" Floor 2 "), "Floor 2".into()),
    ] {
        let v = ok_json(
            f.issue(
                &org,
                json!({
                    "instance_key_id": new_key(),
                    "scope": "site",
                    "site": site,
                    "quota_erasures": 500,
                    "features": [],
                    "days": 1,
                    "reason": "goodwill",
                }),
            )
            .await,
            StatusCode::CREATED,
        )
        .await;
        let id = v["licence"]["id"].as_str().unwrap();
        let cert: LicenseCertificate = serde_json::from_str(&f.stored_document(id).await).unwrap();
        let ent: &Entitlements = &cert.body.entitlements;
        assert_eq!(ent.scope, Scope::Site { site_id: want });
        assert_eq!(ent.quota, Quota::Count { erasures: 500 });
        assert!(ent.features.is_empty());
        assert_eq!(ent.not_after, ent.not_before + time::Duration::days(1));
    }
}

#[tokio::test]
async fn a_bad_issue_body_is_400_naming_the_field_and_writes_nothing() {
    let f = fixture().await;
    let org = f.org_id.to_string();
    let key = new_key();
    let with = |k: &str, v: Value| {
        let mut b = machine_body(&key);
        b[k] = v;
        b
    };
    let without = |k: &str| {
        let mut b = machine_body(&key);
        b.as_object_mut().unwrap().remove(k);
        b
    };
    let before = f.counts().await;
    let cases: Vec<(Value, &str)> = vec![
        (
            with("instance_key_id", json!("ed25519:nope")),
            MSG_BAD_INSTANCE_KEY,
        ),
        (with("scope", json!("galaxy")), "scope"),
        (with("features", json!(["teleport"])), "features"),
        (with("quota_erasures", json!(0)), "quota_erasures"),
        (
            with("quota_erasures", json!(1_000_000_001)),
            "quota_erasures",
        ),
        (without("quota_erasures"), "quota_erasures"),
        (with("days", json!(0)), "days"),
        (with("days", json!(1096)), "days"),
        (with("reason", json!("   ")), "reason"),
        (with("reason", json!("x".repeat(501))), "reason"),
        (with("reason", json!("line\u{7}bell")), "reason"),
        (with("station_id", json!(" ")), MSG_STATION_ID_REQUIRED),
        (without("station_id"), MSG_STATION_ID_REQUIRED),
        (with("seats", json!(3)), "seats"),
        (without("reason"), "reason"),
    ];
    for (body, field) in cases {
        let resp = f.issue(&org, body.clone()).await;
        let code = resp.status();
        let v: Value = resp.json().await.unwrap();
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}: {v}");
        let msg = v["error"].as_str().unwrap();
        assert!(msg.contains(field), "{body}: {msg}");
    }
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn unknown_non_uuid_and_vendor_organizations_are_404() {
    let f = fixture().await;
    let vendor: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE is_vendor")
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    let before = f.counts().await;
    for org in [
        Uuid::new_v4().to_string(),
        "not-a-uuid".into(),
        vendor.to_string(),
    ] {
        assert_error(
            f.issue(&org, machine_body(&new_key())).await,
            StatusCode::NOT_FOUND,
            MSG_NO_ORGANIZATION,
        )
        .await;
        assert_error(
            f.admin_get(&format!("/api/admin/organizations/{org}/licences"))
                .await,
            StatusCode::NOT_FOUND,
            MSG_NO_ORGANIZATION,
        )
        .await;
    }
    assert_eq!(f.counts().await, before);
}

// ---------------------------------------------------------------------------
// No vendor root

#[tokio::test]
async fn without_a_vendor_root_issue_is_503_but_view_and_inspect_work() {
    let hub = spawn_hub().await;
    let admin = bootstrap_admin(&hub).await;
    let owner = hub.verified_owner("Rootless Ltd").await;
    let org: Uuid = sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1::uuid")
        .bind(&owner.id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    let before = (hub.count("licences").await, hub.count("audit_log").await);
    assert_error(
        hub.post(
            &format!("/api/admin/organizations/{org}/licences"),
            machine_body(&new_key()),
            Some(&admin.cookie),
        )
        .await,
        StatusCode::SERVICE_UNAVAILABLE,
        MSG_ISSUANCE_UNAVAILABLE,
    )
    .await;
    assert_eq!(
        (hub.count("licences").await, hub.count("audit_log").await),
        before
    );

    // A licence signed elsewhere, stored as if this hub had issued it.
    let other = VendorRoot::generate();
    let id = Uuid::new_v4();
    let now = OffsetDateTime::now_utc();
    let cert = other
        .issue(
            id.to_string(),
            new_key(),
            Entitlements {
                customer_id: org.to_string(),
                customer_name: "Rootless Ltd".into(),
                quota: Quota::Unlimited,
                scope: Scope::Site {
                    site_id: org.to_string(),
                },
                not_before: now,
                not_after: now + time::Duration::days(10),
                features: vec![],
                allowed_methods: AllowedMethods::All,
                machine_binding: None,
            },
            now,
        )
        .unwrap();
    let document = serde_json::to_string_pretty(&cert).unwrap();
    sqlx::query(
        "INSERT INTO licences (id, organization_id, instance_key_id, root_key_id, not_after, \
                               issued_at, document, source, issue_reason) \
         VALUES ($1, $2, $3, $4, now() + interval '10 days', now(), $5, 'vendor', 'test')",
    )
    .bind(id)
    .bind(org)
    .bind(&cert.body.instance_public_key_id)
    .bind(&cert.body.root_key_id)
    .bind(&document)
    .execute(&hub.db.pool)
    .await
    .unwrap();

    let v = ok_json(
        hub.get(&format!("/api/admin/licences/{id}"), Some(&admin.cookie))
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        v["verification"],
        json!({ "status": "no_root", "root_key_id": null, "detail": null })
    );
    let v = ok_json(
        hub.post(
            "/api/admin/licences/inspect",
            json!({ "document": document }),
            Some(&admin.cookie),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(v["verification"]["status"], "no_root");
    assert_eq!(v["licence"]["id"], json!(id));
}

// ---------------------------------------------------------------------------
// List and view

#[tokio::test]
async fn the_list_shows_station_and_vendor_licences_newest_first_and_is_audited() {
    let f = fixture().await;
    let (station, station_licence) = f.register(&new_key()).await;
    let vendor = f.issue_machine(&new_key()).await;
    let vendor_id = vendor["id"].as_str().unwrap();
    let v = ok_json(
        f.admin_get(&format!("/api/admin/organizations/{}/licences", f.org_id))
            .await,
        StatusCode::OK,
    )
    .await;
    let list = v["licences"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0], vendor);
    assert_eq!(list[1]["id"], json!(station_licence));
    assert_eq!(list[1]["source"], "station");
    assert_eq!(
        list[1]["station"],
        json!({ "id": station, "station_id": "bench-1", "label": "Bench 1" })
    );
    assert_eq!(list[1]["entitlement_id"], json!(f.machine));
    assert_eq!(list[1]["issue_reason"], Value::Null);
    let mut keys: Vec<&String> = list[1].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "entitlement_id",
            "id",
            "instance_key_id",
            "issue_reason",
            "issued_at",
            "lapse_policy",
            "not_after",
            "organization",
            "revocation_reason",
            "revoked_at",
            "root_key_id",
            "source",
            "station",
            "status",
            "superseded_at"
        ]
    );
    assert_eq!(
        f.hub.audited("admin.organization_licences_viewed").await,
        [(
            f.actor(),
            format!("organization:{}", f.org_id),
            json!({ "licence_ids": [vendor_id, station_licence] }),
        )]
    );
}

#[tokio::test]
async fn viewing_a_licence_decodes_and_verifies_it() {
    let f = fixture().await;
    let key = new_key();
    let licence = f.issue_machine(&key).await;
    let id = licence["id"].as_str().unwrap();
    let v = ok_json(
        f.admin_get(&format!("/api/admin/licences/{id}")).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(v["licence"], licence);
    let cert: LicenseCertificate = serde_json::from_str(&f.stored_document(id).await).unwrap();
    assert_eq!(v["body"], serde_json::to_value(&cert.body).unwrap());
    assert_eq!(
        v["verification"],
        json!({ "status": "verified", "root_key_id": f.root.public_key_id(), "detail": null })
    );
    assert_eq!(
        f.hub.audited("admin.licence_viewed").await,
        [(
            f.actor(),
            format!("licence:{id}"),
            json!({ "organization_id": f.org_id }),
        )]
    );

    // Unknown and non-UUID ids: 404, nothing audited.
    let before = f.counts().await;
    for bad in [Uuid::new_v4().to_string(), "nope".into()] {
        for path in [
            format!("/api/admin/licences/{bad}"),
            format!("/api/admin/licences/{bad}/download"),
        ] {
            assert_error(
                f.admin_get(&path).await,
                StatusCode::NOT_FOUND,
                MSG_NO_LICENCE,
            )
            .await;
        }
        assert_error(
            f.revoke(&bad, "reason").await,
            StatusCode::NOT_FOUND,
            MSG_NO_LICENCE,
        )
        .await;
    }
    assert_eq!(f.counts().await, before);
}

// ---------------------------------------------------------------------------
// Inspect

#[tokio::test]
async fn inspecting_a_pasted_file_verifies_it_and_finds_the_record() {
    let f = fixture().await;
    let licence = f.issue_machine(&new_key()).await;
    let id = licence["id"].as_str().unwrap();
    let document = f.stored_document(id).await;
    let cert: LicenseCertificate = serde_json::from_str(&document).unwrap();

    let v = ok_json(
        f.inspect(json!({ "document": document })).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(v["body"], serde_json::to_value(&cert.body).unwrap());
    assert_eq!(v["verification"]["status"], "verified");
    assert_eq!(v["licence"], licence);

    // Tampered: an edited customer name.
    let mut edited: Value = serde_json::from_str(&document).unwrap();
    edited["body"]["entitlements"]["customer_name"] = json!("Mallory Ltd");
    let tampered = serde_json::to_string_pretty(&edited).unwrap();
    let v = ok_json(
        f.inspect(json!({ "document": tampered })).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(v["verification"]["status"], "failed");
    assert!(
        v["verification"]["detail"]
            .as_str()
            .unwrap()
            .contains("tampered"),
        "{v}"
    );
    assert_eq!(v["body"]["entitlements"]["customer_name"], "Mallory Ltd");
    assert_eq!(v["licence"]["id"], json!(id));

    // Signed by another root, unknown to the hub.
    let other = VendorRoot::generate();
    let foreign = other
        .issue(
            "not-a-uuid",
            new_key(),
            cert.body.entitlements.clone(),
            cert.body.issued_at,
        )
        .unwrap();
    let v = ok_json(
        f.inspect(json!({ "document": serde_json::to_string(&foreign).unwrap() }))
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(v["verification"]["status"], "failed");
    assert!(v["verification"]["detail"]
        .as_str()
        .unwrap()
        .contains("untrusted root"));
    assert_eq!(v["licence"], Value::Null);

    let inspected = f.hub.audited("admin.licence_file_inspected").await;
    assert_eq!(inspected.len(), 3);
    assert_eq!(
        inspected[0],
        (
            f.actor(),
            "inspect:licence-file".to_string(),
            json!({
                "license_id": id,
                "canonical_sha256_hex": cert.signature.canonical_sha256_hex,
                "verified": true,
                "known": true,
            }),
        )
    );
    assert_eq!(inspected[1].2["verified"], false);
    assert_eq!(inspected[1].2["known"], true);
    // The tampered file still claims the original hash; the log records the
    // hash of what was actually pasted.
    assert_ne!(
        inspected[1].2["canonical_sha256_hex"],
        json!(cert.signature.canonical_sha256_hex)
    );
    // A pasted id that is not a UUID never reaches the append-only log.
    assert_eq!(inspected[2].2["license_id"], Value::Null);
    assert_eq!(inspected[2].2["known"], false);

    // Not JSON, not a licence, too big, or no document: 400, nothing audited.
    let before = f.counts().await;
    let mut big: Value = serde_json::from_str(&document).unwrap();
    big["padding"] = json!("x".repeat(64 * 1024));
    for body in [
        json!({ "document": "not json" }),
        json!({ "document": "{\"hello\": 1}" }),
        json!({ "document": serde_json::to_string(&big).unwrap() }),
        json!({ "doc": document }),
        json!([]),
    ] {
        assert_error(
            f.inspect(body).await,
            StatusCode::BAD_REQUEST,
            MSG_NOT_A_LICENCE,
        )
        .await;
    }
    assert_eq!(f.counts().await, before);
}

// ---------------------------------------------------------------------------
// Revoke

#[tokio::test]
async fn revoking_a_station_licence_is_terminal_audited_and_seen_by_the_customer() {
    let f = fixture().await;
    let (station, licence) = f.register(&new_key()).await;

    // A bad reason: 400, nothing written.
    let before = f.counts().await;
    for reason in ["  ", &"x".repeat(501), "a\u{0}b"] {
        let resp = f.revoke(&licence, reason).await;
        let (code, msg) = support::hub::error_of(resp).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert!(msg.contains("reason"), "{msg}");
    }
    assert_eq!(f.counts().await, before);

    let v = ok_json(
        f.revoke(&licence, "  chargeback fraud, ticket 4411 ").await,
        StatusCode::OK,
    )
    .await;
    let revoked = &v["licence"];
    assert_eq!(revoked["status"], "revoked");
    assert!(revoked["revoked_at"].as_str().is_some());
    assert_eq!(
        revoked["revocation_reason"],
        "chargeback fraud, ticket 4411"
    );
    assert_eq!(revoked["superseded_at"], Value::Null);
    assert_eq!(
        f.hub.audited("licence.revoked").await,
        [(
            f.actor(),
            format!("licence:{licence}"),
            json!({
                "organization_id": f.org_id,
                "station_id": station,
                "previous_status": "active",
                "reason": "chargeback fraud, ticket 4411",
            }),
        )]
    );

    // Terminal.
    let before = f.counts().await;
    assert_error(
        f.revoke(&licence, "again").await,
        StatusCode::CONFLICT,
        MSG_ALREADY_REVOKED,
    )
    .await;
    assert_eq!(f.counts().await, before);

    // The customer sees it revoked, without the reason, even after a renewal.
    f.extend_term().await;
    for who in [&f.owner, &f.member] {
        let s = f
            .stations(who)
            .await
            .into_iter()
            .find(|s| s["id"] == json!(station))
            .unwrap();
        assert_eq!(s["licence"]["status"], "revoked");
        assert_eq!(s["licence"]["revoked_at"], revoked["revoked_at"]);
        assert_eq!(s["renewal_available"], false);
        assert!(!s.to_string().contains("chargeback"), "{s}");
        assert!(s["licence"].get("revocation_reason").is_none());
        assert_error(
            f.hub
                .get(
                    &format!("/api/licences/{licence}/download"),
                    Some(&who.cookie),
                )
                .await,
            StatusCode::CONFLICT,
            MSG_LICENCE_REVOKED,
        )
        .await;
    }
    let before = f.counts().await;
    assert_error(
        f.hub
            .post_empty(
                &format!("/api/stations/{station}/reissue"),
                Some(&f.owner.cookie),
            )
            .await,
        StatusCode::CONFLICT,
        MSG_LICENCE_REVOKED,
    )
    .await;
    assert_eq!(f.counts().await, before);
    // The registration and seat stay until the owner releases it.
    let v = ok_json(
        f.hub
            .post_empty(
                &format!("/api/stations/{station}/release"),
                Some(&f.owner.cookie),
            )
            .await,
        StatusCode::OK,
    )
    .await;
    assert!(v["station"]["released_at"].as_str().is_some());
    assert_eq!(v["station"]["licence"]["status"], "revoked");
    // No supersession was recorded for the revoked licence.
    assert!(f.hub.audited("licence.superseded").await.is_empty());
    // The admin still downloads the revoked file.
    let resp = f
        .admin_get(&format!("/api/admin/licences/{licence}/download"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_superseded_licence_and_a_vendor_licence_can_be_revoked() {
    let f = fixture().await;
    let (station, licence) = f.register(&new_key()).await;
    ok_json(
        f.hub
            .post_empty(
                &format!("/api/stations/{station}/release"),
                Some(&f.owner.cookie),
            )
            .await,
        StatusCode::OK,
    )
    .await;
    let v = ok_json(f.revoke(&licence, "lost bench").await, StatusCode::OK).await;
    assert_eq!(v["licence"]["status"], "revoked");
    assert!(v["licence"]["superseded_at"].as_str().is_some());
    assert_eq!(
        f.hub.audited("licence.revoked").await[0].2["previous_status"],
        "superseded"
    );

    let vendor = f.issue_machine(&new_key()).await;
    let id = vendor["id"].as_str().unwrap();
    let v = ok_json(f.revoke(id, "trial ended early").await, StatusCode::OK).await;
    assert_eq!(v["licence"]["status"], "revoked");
    assert_eq!(
        f.hub.audited("licence.revoked").await[1].2,
        json!({
            "organization_id": f.org_id,
            "station_id": null,
            "previous_status": "active",
            "reason": "trial ended early",
        })
    );
}

#[tokio::test]
async fn revoking_during_a_concurrent_reissue_never_fails_the_server() {
    let f = fixture().await;
    for round in 0..4 {
        let (station, old) = f.register(&new_key()).await;
        f.extend_term().await;
        let path = format!("/api/stations/{station}/reissue");
        let reissue = f.hub.post_empty(&path, Some(&f.owner.cookie));
        let revoke = f.revoke(&old, "race");
        let (reissued, revoked) = tokio::join!(reissue, revoke);
        let (reissue_code, revoke_code) = (reissued.status(), revoked.status());
        let reissue_body: Value = reissued.json().await.unwrap();
        let revoke_body: Value = revoked.json().await.unwrap();
        assert_eq!(revoke_code, StatusCode::OK, "round {round}: {revoke_body}");
        match reissue_code {
            // Reissue first: the old licence was superseded, then revoked.
            StatusCode::CREATED => {
                assert_eq!(revoke_body["licence"]["status"], "revoked");
                assert!(revoke_body["licence"]["superseded_at"].as_str().is_some());
                let s = f
                    .stations(&f.owner)
                    .await
                    .into_iter()
                    .find(|s| s["id"] == json!(station))
                    .unwrap();
                assert_eq!(s["licence"]["status"], "active");
            }
            // Revoke first: the reissue is refused.
            StatusCode::CONFLICT => {
                assert_eq!(reissue_body["error"], MSG_LICENCE_REVOKED, "round {round}")
            }
            other => panic!("round {round}: reissue {other}: {reissue_body}"),
        }
        ok_json(
            f.hub
                .post_empty(
                    &format!("/api/stations/{station}/release"),
                    Some(&f.owner.cookie),
                )
                .await,
            StatusCode::OK,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Who gets in

#[tokio::test]
async fn customers_and_anonymous_callers_reach_none_of_the_licence_routes() {
    let f = fixture().await;
    let (_, licence) = f.register(&new_key()).await;
    let org = f.org_id.to_string();
    let before = f.counts().await;
    for (who, code) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(&f.owner), StatusCode::FORBIDDEN),
        (Some(&f.member), StatusCode::FORBIDDEN),
    ] {
        let cookie = who.map(|w| w.cookie.as_str());
        let resps = [
            f.hub
                .get(&format!("/api/admin/organizations/{org}/licences"), cookie)
                .await,
            f.hub
                .post(
                    &format!("/api/admin/organizations/{org}/licences"),
                    machine_body(&new_key()),
                    cookie,
                )
                .await,
            f.hub
                .get(&format!("/api/admin/licences/{licence}"), cookie)
                .await,
            f.hub
                .get(&format!("/api/admin/licences/{licence}/download"), cookie)
                .await,
            f.hub
                .post(
                    "/api/admin/licences/inspect",
                    json!({ "document": "{}" }),
                    cookie,
                )
                .await,
            f.hub
                .post(
                    &format!("/api/admin/licences/{licence}/revoke"),
                    json!({ "reason": "x" }),
                    cookie,
                )
                .await,
        ];
        for resp in resps {
            let url = resp.url().to_string();
            let (got, msg) = support::hub::error_of(resp).await;
            assert_eq!(got, code, "{url}");
            if code == StatusCode::FORBIDDEN {
                assert_eq!(msg, MSG_ADMIN_ONLY);
            }
        }
    }
    assert_eq!(f.counts().await, before);
    let status: String = sqlx::query_scalar("SELECT status FROM licences WHERE id = $1::uuid")
        .bind(&licence)
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
}

/// Revoke waits on the station's row lock, so a reissue holding that lock
/// finishes first and the revocation then lands on the superseded licence:
/// a 200, never a 500 or a trigger refusal.
#[tokio::test]
async fn a_revoke_waits_for_a_reissue_holding_the_station_lock() {
    let f = fixture().await;
    let (station, old) = f.register(&new_key()).await;

    let mut holder = f.hub.db.pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM stations WHERE id = $1::uuid FOR UPDATE")
        .bind(&station)
        .execute(&mut *holder)
        .await
        .unwrap();

    let req = f
        .hub
        .post_req(
            &format!("/api/admin/licences/{old}/revoke"),
            Some(&f.admin.cookie),
        )
        .json(&json!({ "reason": "held" }));
    let revoke = tokio::spawn(async move { req.send().await.unwrap() });

    // Wait until the revoke is queued behind the holder's station lock.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
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
            "the revoke never waited on the station lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // What a reissue does under that lock: supersede the active licence.
    sqlx::query(
        "UPDATE licences SET status = 'superseded', superseded_at = now() \
         WHERE id = $1::uuid AND status = 'active'",
    )
    .bind(&old)
    .execute(&mut *holder)
    .await
    .unwrap();
    holder.commit().await.unwrap();

    let v = ok_json(revoke.await.unwrap(), StatusCode::OK).await;
    assert_eq!(v["licence"]["status"], "revoked");
    assert!(v["licence"]["superseded_at"].as_str().is_some());
    let revoked = f.hub.audited("licence.revoked").await;
    assert_eq!(revoked.last().unwrap().2["previous_status"], "superseded");
}

// ---------------------------------------------------------------------------
// Lapse policy (story 4, CAP-6, ADR-0012)

#[tokio::test]
async fn a_vendor_issued_licence_carries_the_organizations_lapse_policy() {
    let f = fixture().await;
    let resp = f
        .hub
        .post(
            "/api/organization/lapse-policy",
            json!({ "lapse_policy": "stop" }),
            Some(&f.owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let key = new_key();
    let licence = f.issue_machine(&key).await;
    let id = licence["id"].as_str().unwrap();
    assert_eq!(licence["lapse_policy"], "stop");
    let file = f.stored_document(id).await;
    let doc: Value = serde_json::from_str(&file).unwrap();
    assert_eq!(doc["body"]["lapse_policy"], "stop");
    let cert = install(&file, &key, &f.root);
    assert_eq!(cert.body.lapse_policy, wipe_license::LapsePolicy::Stop);
    assert_eq!(
        f.hub.audited("licence.issued").await[0].2["lapse_policy"],
        "stop"
    );

    // The admin licence page and organization page show it read-only.
    let v = ok_json(
        f.admin_get(&format!("/api/admin/licences/{id}")).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(v["licence"]["lapse_policy"], "stop");
    assert_eq!(v["body"]["lapse_policy"], "stop");
    let v = ok_json(
        f.admin_get(&format!("/api/admin/organizations/{}", f.org_id))
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(v["lapse_policy"], "stop");

    // The vendor has no route that changes a customer's policy: its own
    // POST is refused, and changes nothing of the customer's.
    let resp = f
        .hub
        .post(
            "/api/organization/lapse-policy",
            json!({ "lapse_policy": "evaluation" }),
            Some(&f.admin.cookie),
        )
        .await;
    assert_error(
        resp,
        StatusCode::FORBIDDEN,
        wipe_hub::stations::MSG_OWNER_ONLY_LAPSE_POLICY,
    )
    .await;
    let policy: String = sqlx::query_scalar("SELECT lapse_policy FROM organizations WHERE id = $1")
        .bind(f.org_id)
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(policy, "stop");
}

#[tokio::test]
async fn an_organization_on_evaluation_shows_evaluation_to_the_admin() {
    let f = fixture().await;
    let licence = f.issue_machine(&new_key()).await;
    assert_eq!(licence["lapse_policy"], "evaluation");
    let doc: Value =
        serde_json::from_str(&f.stored_document(licence["id"].as_str().unwrap()).await).unwrap();
    assert!(doc["body"].get("lapse_policy").is_none(), "{doc}");
    let v = ok_json(
        f.admin_get(&format!("/api/admin/organizations/{}", f.org_id))
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(v["lapse_policy"], "evaluation");
}
