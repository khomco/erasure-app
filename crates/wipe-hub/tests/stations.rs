//! Stations and licences (licence-delivery stories 1 and 2, CAP-1, CAP-2,
//! CAP-3, CAP-5): one test per row of each story's I/O matrix, through HTTP, checking the
//! response, the stored rows and the audit rows. The downloaded file is
//! installed the way `wipestation serve --license … --vendor-root-b64 …`
//! installs it (`load_license`, `check_installable`, `evaluate` with the
//! station's fingerprint). Needs `docker compose up -d`; fails, never skips,
//! when it is down.

mod support;

use reqwest::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{assert_error, spawn_hub, spawn_hub_with_root, Hub, Who};
use time::OffsetDateTime;
use uuid::Uuid;
use wipe_hub::order_routes::MSG_NO_ENTITLEMENT;
use wipe_hub::stations::{
    MSG_ALREADY_REGISTERED, MSG_ALREADY_RELEASED, MSG_BAD_INSTANCE_KEY,
    MSG_ENTITLEMENT_NOT_CURRENT, MSG_ISSUANCE_UNAVAILABLE, MSG_LICENCE_UP_TO_DATE,
    MSG_NO_FREE_SEAT, MSG_NO_LICENCE, MSG_NO_STATION, MSG_OWNER_ONLY_STATIONS,
    MSG_STATION_ID_REQUIRED,
};
use wipe_license::{
    check_installable, evaluate, load_license, machine_fingerprint, AllowedMethods, Feature,
    LeaseState, LeaseStatus, LicenseCertificate, Quota, Scope, VendorRoot,
};

struct Fixture {
    hub: Hub,
    root: VendorRoot,
    owner: Who,
    member: Who,
    org_id: Uuid,
    /// Annual, one machine seat, unlimited, `hub_sync` + `pdf_certificates`.
    machine: String,
    /// One-off, two site seats, 5000 erasures, `pdf_certificates`.
    site: String,
}

/// A station's instance key id, as `wipestation` prints it.
fn new_key() -> String {
    wipe_cert::SigningKey::generate().public_key_id()
}

async fn ok_json(resp: Response, want: StatusCode) -> Value {
    let code = resp.status();
    let v: Value = resp.json().await.unwrap();
    assert_eq!(code, want, "{v}");
    v
}

async fn org_of(hub: &Hub, who: &Who) -> Uuid {
    sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1::uuid")
        .bind(&who.id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
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

async fn fixture_on(hub: Hub, root: VendorRoot) -> Fixture {
    let owner = hub.verified_owner("Acme Recyclers").await;
    let member = hub.add(&owner, "member").await;
    let org_id = org_of(&hub, &owner).await;
    let annual = hub.published_offering("Station — annual, 1 seat").await;
    let one_off = hub.published_one_off_offering("Site — 2 years", 24).await;
    let machine = buy(&hub, &owner, &annual).await;
    let site = buy(&hub, &owner, &one_off).await;
    Fixture {
        hub,
        root,
        owner,
        member,
        org_id,
        machine,
        site,
    }
}

async fn fixture() -> Fixture {
    let (hub, root) = spawn_hub_with_root().await;
    fixture_on(hub, root).await
}

impl Fixture {
    async fn register(&self, who: &Who, body: Value) -> Response {
        self.hub
            .post("/api/stations", body, Some(&who.cookie))
            .await
    }

    /// The owner registers `key` on the machine entitlement as `bench-1`: 201.
    async fn register_machine(&self, key: &str) -> Value {
        ok_json(
            self.register(
                &self.owner,
                json!({
                    "entitlement_id": self.machine,
                    "instance_key_id": key,
                    "station_id": "bench-1",
                    "label": "Bench 1",
                }),
            )
            .await,
            StatusCode::CREATED,
        )
        .await
    }

    async fn download(&self, who: &Who, licence: &str) -> Response {
        self.hub
            .get(
                &format!("/api/licences/{licence}/download"),
                Some(&who.cookie),
            )
            .await
    }

    async fn release(&self, who: &Who, station: &str) -> Response {
        self.hub
            .post_empty(
                &format!("/api/stations/{station}/release"),
                Some(&who.cookie),
            )
            .await
    }

    async fn reissue(&self, who: &Who, station: &str) -> Response {
        self.hub
            .post_empty(
                &format!("/api/stations/{station}/reissue"),
                Some(&who.cookie),
            )
            .await
    }

    /// The owner pays a Fake renewal of `entitlement`: its term grows by a
    /// year.
    async fn renew(&self, entitlement: &str) {
        ok_json(
            self.hub
                .post_empty(
                    &format!("/api/payments/fake/entitlements/{entitlement}/renew"),
                    Some(&self.owner.cookie),
                )
                .await,
            StatusCode::OK,
        )
        .await;
    }

    /// Move `entitlement`'s term end out by a year in SQL, as a renewal
    /// would (the trigger allows it while the entitlement is active).
    async fn extend_term(&self, entitlement: &str) {
        sqlx::query(
            "UPDATE entitlements SET term_end = term_end + interval '1 year' \
             WHERE id = $1::uuid",
        )
        .bind(entitlement)
        .execute(&self.hub.db.pool)
        .await
        .unwrap();
    }

    async fn station(&self, who: &Who, id: &str) -> Value {
        self.stations(who)
            .await
            .into_iter()
            .find(|s| s["id"] == json!(id))
            .unwrap()
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

    /// Row counts a refused registration must leave unchanged.
    async fn counts(&self) -> (i64, i64, i64) {
        (
            self.hub.count("stations").await,
            self.hub.count("licences").await,
            self.hub.count("audit_log").await,
        )
    }

    async fn stored_document(&self, licence: &str) -> String {
        sqlx::query_scalar("SELECT document FROM licences WHERE id = $1::uuid")
            .bind(licence)
            .fetch_one(&self.hub.db.pool)
            .await
            .unwrap()
    }

    async fn term_end(&self, entitlement: &str) -> OffsetDateTime {
        let micros: i64 = sqlx::query_scalar(
            "SELECT (extract(epoch FROM term_end) * 1000000)::bigint FROM entitlements \
             WHERE id = $1::uuid",
        )
        .bind(entitlement)
        .fetch_one(&self.hub.db.pool)
        .await
        .unwrap();
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(micros) * 1000).unwrap()
    }
}

/// Install `file` the way `wipestation serve --license <file>
/// --vendor-root-b64 <root>` does, on a station whose signing key is `key`.
fn install(file: &str, key: &str, root: &VendorRoot) -> LicenseCertificate {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("licence.json");
    std::fs::write(&path, file).unwrap();
    let licence = load_license(&path).unwrap();
    // The station decodes the published root key from base64.
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

#[tokio::test]
async fn registering_a_machine_station_issues_a_licence_it_installs() {
    let f = fixture().await;
    let key = new_key();
    let v = f.register_machine(&format!("  {key} ")).await;
    let mut keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["licence", "station"]);
    // The exact shapes, nullable keys present.
    let sorted_keys = |v: &Value| {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    assert_eq!(
        sorted_keys(&v["station"]),
        [
            "entitlement_id",
            "id",
            "instance_key_id",
            "label",
            "licence",
            "offering_name",
            "registered_at",
            "released_at",
            "renewal_available",
            "scope",
            "site",
            "station_id"
        ]
    );
    assert_eq!(
        sorted_keys(&v["station"]["licence"]),
        [
            "entitlement_id",
            "id",
            "instance_key_id",
            "issued_at",
            "not_after",
            "revoked_at",
            "root_key_id",
            "station_id",
            "status",
            "superseded_at"
        ]
    );
    let station = &v["station"];
    let licence = &v["licence"];
    let station_id = station["id"].as_str().unwrap();
    let licence_id = licence["id"].as_str().unwrap();
    assert_eq!(station["entitlement_id"], json!(f.machine));
    assert_eq!(station["scope"], "machine");
    assert_eq!(station["instance_key_id"], json!(key));
    assert_eq!(station["station_id"], "bench-1");
    assert_eq!(station["label"], "Bench 1");
    assert_eq!(station["site"], Value::Null);
    assert_eq!(station["released_at"], Value::Null);
    assert_eq!(station["renewal_available"], false);
    assert_eq!(&station["licence"], licence);
    assert_eq!(licence["station_id"], json!(station_id));
    assert_eq!(licence["status"], "active");
    assert_eq!(licence["root_key_id"], json!(f.root.public_key_id()));
    assert_eq!(licence["superseded_at"], Value::Null);
    assert_eq!(licence["revoked_at"], Value::Null);

    // Every role downloads the exact stored file.
    let resp = f.download(&f.member, licence_id).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(
        resp.headers()[CONTENT_DISPOSITION],
        format!("attachment; filename=\"licence-{licence_id}.json\"").as_str()
    );
    assert_eq!(resp.headers()[reqwest::header::CACHE_CONTROL], "no-store");
    let file = resp.text().await.unwrap();
    assert_eq!(file, f.stored_document(licence_id).await);

    // The station accepts it, and it is licensed to the organization.
    let cert = install(&file, &key, &f.root);
    assert_eq!(
        file,
        serde_json::to_string_pretty(&cert).unwrap(),
        "not save_license's format"
    );
    let body = &cert.body;
    assert_eq!(body.license_id, licence_id);
    assert_eq!(body.instance_public_key_id, key);
    assert_eq!(body.root_key_id, f.root.public_key_id());
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
    assert_eq!(ent.machine_binding, None);
    assert_eq!(ent.features, [Feature::HubSync, Feature::PdfCertificates]);
    assert_eq!(ent.allowed_methods, AllowedMethods::All);
    assert_eq!(ent.not_before, body.issued_at);
    assert_eq!(ent.not_after, f.term_end(&f.machine).await);
    let now = OffsetDateTime::now_utc();
    assert!((now - body.issued_at).whole_seconds().abs() < 60);
    let status = evaluate(
        Some(ent),
        &LeaseState::new(now),
        &machine_fingerprint("bench-1"),
        now,
    );
    assert!(
        matches!(
            status,
            LeaseStatus::Valid {
                remaining_erasures: None,
                ..
            }
        ),
        "{status:?}"
    );
    // On a station started with another id it only signs evaluation certs.
    assert!(matches!(
        evaluate(
            Some(ent),
            &LeaseState::new(now),
            &machine_fingerprint("standalone"),
            now
        ),
        LeaseStatus::WrongMachine { .. }
    ));

    // Audit: registration, issuance and the download.
    let actor = format!("user:{}", f.owner.id);
    let registered = f.hub.audited("station.registered").await;
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0].0, actor);
    assert_eq!(registered[0].1, format!("station:{station_id}"));
    assert_eq!(registered[0].2["instance_key_id"], json!(key));
    let issued = f.hub.audited("licence.issued").await;
    assert_eq!(issued.len(), 1);
    assert_eq!(issued[0].0, actor);
    assert_eq!(issued[0].1, format!("licence:{licence_id}"));
    assert_eq!(
        issued[0].2,
        json!({
            "station_id": station_id,
            "entitlement_id": f.machine,
            "instance_key_id": key,
            "root_key_id": f.root.public_key_id(),
            "not_after": licence["not_after"],
        })
    );
    let downloaded = f.hub.audited("licence.downloaded").await;
    assert_eq!(
        downloaded,
        [(
            format!("user:{}", f.member.id),
            format!("licence:{licence_id}"),
            json!({ "station_id": station_id }),
        )]
    );

    // Downloads are byte-identical every time.
    let again = f.download(&f.owner, licence_id).await.text().await.unwrap();
    assert_eq!(again, file);

    // Every role lists it.
    let listed = f.stations(&f.member).await;
    assert_eq!(listed, std::slice::from_ref(station));
}

#[tokio::test]
async fn a_site_station_names_the_entered_site_or_the_organization() {
    let f = fixture().await;
    let key = new_key();
    let v = ok_json(
        f.register(
            &f.owner,
            json!({
                "entitlement_id": f.site,
                "instance_key_id": key,
                "site": " Dock 4 ",
            }),
        )
        .await,
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(v["station"]["site"], "Dock 4");
    assert_eq!(v["station"]["station_id"], Value::Null);
    let file = f
        .download(&f.owner, v["licence"]["id"].as_str().unwrap())
        .await
        .text()
        .await
        .unwrap();
    let cert = install(&file, &key, &f.root);
    let ent = &cert.body.entitlements;
    assert_eq!(
        ent.scope,
        Scope::Site {
            site_id: "Dock 4".into()
        }
    );
    assert_eq!(ent.quota, Quota::Count { erasures: 5000 });
    assert_eq!(ent.features, [Feature::PdfCertificates]);
    assert_eq!(ent.not_after, f.term_end(&f.site).await);
    let now = OffsetDateTime::now_utc();
    // A site licence binds no machine: any station id is fine.
    assert!(matches!(
        evaluate(
            Some(ent),
            &LeaseState::new(now),
            &machine_fingerprint("anything"),
            now
        ),
        LeaseStatus::Valid {
            remaining_erasures: Some(5000),
            ..
        }
    ));

    // Blank site: the organization id.
    let key = new_key();
    let v = ok_json(
        f.register(
            &f.owner,
            json!({
                "entitlement_id": f.site,
                "instance_key_id": key,
                "site": "  ",
            }),
        )
        .await,
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(v["station"]["site"], Value::Null);
    let cert = install(
        &f.stored_document(v["licence"]["id"].as_str().unwrap())
            .await,
        &key,
        &f.root,
    );
    assert_eq!(
        cert.body.entitlements.scope,
        Scope::Site {
            site_id: f.org_id.to_string()
        }
    );
}

#[tokio::test]
async fn a_full_entitlement_refuses_another_station() {
    let f = fixture().await;
    f.register_machine(&new_key()).await;
    let before = f.counts().await;
    let resp = f
        .register(
            &f.owner,
            json!({
                "entitlement_id": f.machine,
                "instance_key_id": new_key(),
                "station_id": "bench-2",
            }),
        )
        .await;
    assert_error(resp, StatusCode::CONFLICT, MSG_NO_FREE_SEAT).await;
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn a_machine_licence_needs_the_station_id() {
    let f = fixture().await;
    let before = f.counts().await;
    for station_id in [json!(null), json!(""), json!("   ")] {
        let resp = f
            .register(
                &f.owner,
                json!({
                    "entitlement_id": f.machine,
                    "instance_key_id": new_key(),
                    "station_id": station_id,
                }),
            )
            .await;
        assert_error(resp, StatusCode::BAD_REQUEST, MSG_STATION_ID_REQUIRED).await;
    }
    let resp = f
        .register(
            &f.owner,
            json!({ "entitlement_id": f.machine, "instance_key_id": new_key() }),
        )
        .await;
    assert_error(resp, StatusCode::BAD_REQUEST, MSG_STATION_ID_REQUIRED).await;
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn a_malformed_instance_key_id_is_refused() {
    let f = fixture().await;
    let before = f.counts().await;
    let key = new_key();
    for bad in [
        "".to_string(),
        "bench-1".to_string(),
        key.trim_start_matches("ed25519:").to_string(),
        format!("{key}A"),
        key.replace("ed25519:", "ed448:"),
    ] {
        let resp = f
            .register(
                &f.owner,
                json!({
                    "entitlement_id": f.site,
                    "instance_key_id": bad,
                }),
            )
            .await;
        assert_error(resp, StatusCode::BAD_REQUEST, MSG_BAD_INSTANCE_KEY).await;
    }
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn a_key_registered_in_the_organization_is_refused() {
    let f = fixture().await;
    let key = new_key();
    f.register_machine(&key).await;
    let before = f.counts().await;
    // Another entitlement with free seats changes nothing.
    let resp = f
        .register(
            &f.owner,
            json!({ "entitlement_id": f.site, "instance_key_id": key }),
        )
        .await;
    assert_error(resp, StatusCode::CONFLICT, MSG_ALREADY_REGISTERED).await;
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn an_entitlement_that_is_not_current_unknown_or_foreign_is_refused() {
    let f = fixture().await;
    let pool = &f.hub.db.pool;
    // Refunded.
    sqlx::query(
        "UPDATE entitlements SET status = 'refunded', refunded_at = now(), renews = false \
         WHERE id = $1::uuid",
    )
    .bind(&f.site)
    .execute(pool)
    .await
    .unwrap();
    // Ended: the term moved into the past (the trigger refuses this to the
    // hub, so it is lifted for the test).
    sqlx::query("ALTER TABLE entitlements DISABLE TRIGGER entitlements_lifecycle")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE entitlements SET term_start = term_start - interval '2 years', \
                term_end = term_end - interval '2 years' WHERE id = $1::uuid",
    )
    .bind(&f.machine)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE entitlements ENABLE TRIGGER entitlements_lifecycle")
        .execute(pool)
        .await
        .unwrap();
    // Another organization's current entitlement.
    let other = f.hub.verified_owner("Other Org").await;
    let offering = f.hub.published_offering("Other annual").await;
    let foreign = buy(&f.hub, &other, &offering).await;

    let before = f.counts().await;
    for (entitlement, code, message) in [
        (
            f.site.clone(),
            StatusCode::CONFLICT,
            MSG_ENTITLEMENT_NOT_CURRENT,
        ),
        (
            f.machine.clone(),
            StatusCode::CONFLICT,
            MSG_ENTITLEMENT_NOT_CURRENT,
        ),
        (
            Uuid::new_v4().to_string(),
            StatusCode::NOT_FOUND,
            MSG_NO_ENTITLEMENT,
        ),
        (foreign, StatusCode::NOT_FOUND, MSG_NO_ENTITLEMENT),
    ] {
        let resp = f
            .register(
                &f.owner,
                json!({
                    "entitlement_id": entitlement,
                    "instance_key_id": new_key(),
                    "station_id": "bench-1",
                }),
            )
            .await;
        assert_error(resp, code, message).await;
    }
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn members_cannot_register_renew_or_release() {
    let f = fixture().await;
    let v = f.register_machine(&new_key()).await;
    let station = v["station"]["id"].as_str().unwrap();
    let before = f.counts().await;
    let resp = f
        .register(
            &f.member,
            json!({
                "entitlement_id": f.site,
                "instance_key_id": new_key(),
            }),
        )
        .await;
    assert_error(resp, StatusCode::FORBIDDEN, MSG_OWNER_ONLY_STATIONS).await;
    let resp = f.release(&f.member, station).await;
    assert_error(resp, StatusCode::FORBIDDEN, MSG_OWNER_ONLY_STATIONS).await;
    assert_eq!(f.counts().await, before);
    // A member sees a renewal is available but cannot issue it.
    f.renew(&f.machine).await;
    let before = f.counts().await;
    let resp = f.reissue(&f.member, station).await;
    assert_error(resp, StatusCode::FORBIDDEN, MSG_OWNER_ONLY_STATIONS).await;
    assert_eq!(f.counts().await, before);
    let listed = f.stations(&f.member).await;
    assert_eq!(listed[0]["released_at"], Value::Null);
    assert_eq!(listed[0]["renewal_available"], true);
}

#[tokio::test]
async fn a_hub_without_a_vendor_root_issues_nothing() {
    let f = fixture_on(spawn_hub().await, VendorRoot::generate()).await;
    let before = f.counts().await;
    let resp = f
        .register(
            &f.owner,
            json!({
                "entitlement_id": f.machine,
                "instance_key_id": new_key(),
                "station_id": "bench-1",
            }),
        )
        .await;
    assert_error(
        resp,
        StatusCode::SERVICE_UNAVAILABLE,
        MSG_ISSUANCE_UNAVAILABLE,
    )
    .await;
    // A reissue is refused the same way, before the station is looked up.
    for station in [Uuid::new_v4().to_string(), "not-a-uuid".to_string()] {
        let resp = f.reissue(&f.owner, &station).await;
        assert_error(
            resp,
            StatusCode::SERVICE_UNAVAILABLE,
            MSG_ISSUANCE_UNAVAILABLE,
        )
        .await;
    }
    assert_eq!(f.counts().await, before);
    let root = ok_json(
        f.hub
            .get("/api/licensing/root", Some(&f.member.cookie))
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(root, json!({ "root": null }));
}

#[tokio::test]
async fn every_role_reads_the_vendor_root_public_key() {
    let f = fixture().await;
    let root = ok_json(
        f.hub
            .get("/api/licensing/root", Some(&f.member.cookie))
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        root,
        json!({
            "root": {
                "public_key_id": f.root.public_key_id(),
                "public_key_b64": f.root.verifying_key().to_base64(),
            }
        })
    );
    let resp = f.hub.get("/api/licensing/root", None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn releasing_a_station_frees_its_seat_and_supersedes_its_licence() {
    let f = fixture().await;
    let key = new_key();
    let v = f.register_machine(&key).await;
    let station = v["station"]["id"].as_str().unwrap();
    let licence = v["licence"]["id"].as_str().unwrap();
    let file = f.stored_document(licence).await;

    let released = ok_json(f.release(&f.owner, station).await, StatusCode::OK).await;
    let s = &released["station"];
    assert_eq!(s["id"], json!(station));
    assert!(s["released_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(s["licence"]["id"], json!(licence));
    assert_eq!(s["licence"]["status"], "superseded");
    assert!(s["licence"]["superseded_at"].as_str().is_some());
    // The stored file is unchanged and still downloadable, byte for byte.
    assert_eq!(f.stored_document(licence).await, file);
    let resp = f.download(&f.owner, licence).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), file);

    let actor = format!("user:{}", f.owner.id);
    let rel = f.hub.audited("station.released").await;
    assert_eq!(rel.len(), 1);
    assert_eq!(rel[0].0, actor);
    assert_eq!(rel[0].1, format!("station:{station}"));
    assert_eq!(
        f.hub.audited("licence.superseded").await,
        [(
            actor,
            format!("licence:{licence}"),
            json!({ "station_id": station, "reason": "station released" }),
        )]
    );

    // The seat is free again: the same key registers anew.
    let again = f.register_machine(&key).await;
    assert_ne!(again["station"]["id"], json!(station));
    let listed = f.stations(&f.owner).await;
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["id"], again["station"]["id"]);
    assert_eq!(listed[1]["id"], json!(station));

    let before = f.counts().await;
    let resp = f.release(&f.owner, station).await;
    assert_error(resp, StatusCode::CONFLICT, MSG_ALREADY_RELEASED).await;
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn released_stations_are_listed_after_active_ones() {
    let f = fixture().await;
    let register_site = |key: String| {
        f.register(
            &f.owner,
            json!({ "entitlement_id": f.site, "instance_key_id": key }),
        )
    };
    let older = ok_json(register_site(new_key()).await, StatusCode::CREATED).await;
    let newer = ok_json(register_site(new_key()).await, StatusCode::CREATED).await;
    let newer_id = newer["station"]["id"].as_str().unwrap();
    ok_json(f.release(&f.owner, newer_id).await, StatusCode::OK).await;
    let listed = f.stations(&f.owner).await;
    assert_eq!(listed.len(), 2);
    // Newest first would put the released one first.
    assert_eq!(listed[0]["id"], older["station"]["id"]);
    assert_eq!(listed[1]["id"], json!(newer_id));
}

#[tokio::test]
async fn another_organizations_station_and_licence_are_unknown() {
    let f = fixture().await;
    let v = f.register_machine(&new_key()).await;
    let station = v["station"]["id"].as_str().unwrap();
    let licence = v["licence"]["id"].as_str().unwrap();
    let other = f.hub.verified_owner("Other Org").await;
    let other_member = f.hub.add(&other, "member").await;
    let before = f.counts().await;
    assert_eq!(f.stations(&other).await, Vec::<Value>::new());
    for who in [&other, &other_member] {
        let resp = f.download(who, licence).await;
        assert_error(resp, StatusCode::NOT_FOUND, MSG_NO_LICENCE).await;
    }
    let resp = f.release(&other, station).await;
    assert_error(resp, StatusCode::NOT_FOUND, MSG_NO_STATION).await;
    for path in [
        format!("/api/licences/{}/download", Uuid::new_v4()),
        "/api/licences/not-a-uuid/download".to_string(),
    ] {
        let resp = f.hub.get(&path, Some(&f.owner.cookie)).await;
        assert_error(resp, StatusCode::NOT_FOUND, MSG_NO_LICENCE).await;
    }
    let resp = f.release(&f.owner, "not-a-uuid").await;
    assert_error(resp, StatusCode::NOT_FOUND, MSG_NO_STATION).await;
    assert_eq!(f.counts().await, before);
    assert_eq!(f.stations(&f.owner).await[0]["released_at"], Value::Null);
}

#[tokio::test]
async fn concurrent_registrations_never_exceed_the_seats() {
    let f = fixture().await;
    let attempts = 8;
    let mut calls = Vec::new();
    for i in 0..attempts {
        let body = json!({
            "entitlement_id": f.machine,
            "instance_key_id": new_key(),
            "station_id": format!("bench-{i}"),
        });
        calls.push(f.register(&f.owner, body));
    }
    let results = futures_join_all(calls).await;
    let mut created = 0;
    for resp in results {
        match resp.status() {
            StatusCode::CREATED => created += 1,
            _ => assert_error(resp, StatusCode::CONFLICT, MSG_NO_FREE_SEAT).await,
        }
    }
    assert_eq!(created, 1);
    assert_eq!(f.hub.count("stations").await, 1);
    assert_eq!(f.hub.count("licences").await, 1);
}

#[tokio::test]
async fn one_key_registered_concurrently_on_two_entitlements_is_registered_once() {
    let f = fixture().await;
    let key = new_key();
    let calls = vec![
        f.register(
            &f.owner,
            json!({
                "entitlement_id": f.machine,
                "instance_key_id": key,
                "station_id": "bench-1",
            }),
        ),
        f.register(
            &f.owner,
            json!({ "entitlement_id": f.site, "instance_key_id": key }),
        ),
    ];
    let mut created = 0;
    for resp in futures_join_all(calls).await {
        match resp.status() {
            StatusCode::CREATED => created += 1,
            // The other one loses on the existence check or, when both pass
            // it, on the partial unique index: 409 either way, never a 500.
            _ => assert_error(resp, StatusCode::CONFLICT, MSG_ALREADY_REGISTERED).await,
        }
    }
    assert_eq!(created, 1);
    assert_eq!(f.hub.count("stations").await, 1);
    assert_eq!(f.hub.count("licences").await, 1);
}

/// Run every future concurrently on this task; their outputs in order.
async fn futures_join_all<F: std::future::Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut pinned: Vec<_> = futures.into_iter().map(Box::pin).collect();
    let mut out: Vec<Option<F::Output>> = (0..pinned.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (i, f) in pinned.iter_mut().enumerate() {
            if out[i].is_none() {
                match f.as_mut().poll(cx) {
                    std::task::Poll::Ready(v) => out[i] = Some(v),
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    out.into_iter().map(Option::unwrap).collect()
}

#[tokio::test]
async fn the_database_refuses_illegal_station_and_licence_changes() {
    let f = fixture().await;
    let v = f.register_machine(&new_key()).await;
    let station = v["station"]["id"].as_str().unwrap();
    let licence = v["licence"]["id"].as_str().unwrap();
    let pool = &f.hub.db.pool;
    for sql in [
        "DELETE FROM stations WHERE id = $1::uuid",
        "UPDATE stations SET instance_key_id = 'ed25519:AAAAAAAAAAAAAAAAAAAAAA' WHERE id = $1::uuid",
        "UPDATE stations SET label = 'renamed' WHERE id = $1::uuid",
        "UPDATE stations SET released_at = now() WHERE id = $1::uuid",
        // A second active registration of the same key.
        "INSERT INTO stations (organization_id, entitlement_id, instance_key_id, station_id, \
                               registered_by) \
         SELECT organization_id, entitlement_id, instance_key_id, 'x', registered_by \
         FROM stations WHERE id = $1::uuid",
    ] {
        sqlx::query(sql)
            .bind(station)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    for sql in [
        "DELETE FROM licences WHERE id = $1::uuid",
        "UPDATE licences SET document = document || ' ' WHERE id = $1::uuid",
        "UPDATE licences SET not_after = not_after + interval '1 day' WHERE id = $1::uuid",
        "UPDATE licences SET status = 'superseded' WHERE id = $1::uuid",
        "UPDATE licences SET status = 'revoked', superseded_at = now() WHERE id = $1::uuid",
    ] {
        sqlx::query(sql)
            .bind(licence)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    // A licence for another organization's entitlement cannot be stored.
    let other = f.hub.verified_owner("Other Org").await;
    let offering = f.hub.published_offering("Other annual").await;
    let foreign = buy(&f.hub, &other, &offering).await;
    sqlx::query(
        "INSERT INTO stations (organization_id, entitlement_id, instance_key_id, registered_by) \
         SELECT organization_id, $2::uuid, 'ed25519:AAAAAAAAAAAAAAAAAAAAAA', registered_by \
         FROM stations WHERE id = $1::uuid",
    )
    .bind(station)
    .bind(&foreign)
    .execute(pool)
    .await
    .expect_err("a station on another organization's entitlement");

    // Released and superseded rows never change again.
    ok_json(f.release(&f.owner, station).await, StatusCode::OK).await;
    for (sql, id) in [
        (
            "UPDATE stations SET released_at = now() + interval '1 day' WHERE id = $1::uuid",
            station,
        ),
        (
            "UPDATE licences SET status = 'active', superseded_at = NULL WHERE id = $1::uuid",
            licence,
        ),
        (
            "UPDATE licences SET superseded_at = now() + interval '1 day' WHERE id = $1::uuid",
            licence,
        ),
    ] {
        sqlx::query(sql)
            .bind(id)
            .execute(pool)
            .await
            .expect_err(sql);
    }
}

// ---------------------------------------------------------------------------
// Renewal reissue (licence-delivery story 2)

#[tokio::test]
async fn a_renewed_station_is_reissued_a_licence_to_the_new_term_end() {
    let f = fixture().await;
    let key = new_key();
    let v = f.register_machine(&key).await;
    let station = v["station"]["id"].as_str().unwrap();
    let old = v["licence"]["id"].as_str().unwrap().to_string();
    let old_file = f.stored_document(&old).await;
    let term_end = f.term_end(&f.machine).await;
    assert_eq!(
        f.station(&f.member, station).await["renewal_available"],
        false
    );

    // A paid renewal moves the term a year on; every role sees the renewal.
    f.renew(&f.machine).await;
    let renewed_end = f.term_end(&f.machine).await;
    assert!(renewed_end > term_end + time::Duration::days(360));
    assert_eq!(
        f.station(&f.member, station).await["renewal_available"],
        true
    );
    // The renewal itself issued nothing.
    assert_eq!(f.hub.count("licences").await, 1);

    let seats_before = ok_json(
        f.hub.get("/api/entitlements", Some(&f.owner.cookie)).await,
        StatusCode::OK,
    )
    .await["available_seats"]
        .clone();
    let (stations_before, licences_before, audits_before) = f.counts().await;
    let r = ok_json(f.reissue(&f.owner, station).await, StatusCode::CREATED).await;
    let mut keys: Vec<&String> = r.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["licence", "station"]);
    let licence = &r["licence"];
    let new = licence["id"].as_str().unwrap();
    assert_ne!(new, old);
    assert_eq!(licence["status"], "active");
    assert_eq!(licence["station_id"], json!(station));
    assert_eq!(licence["entitlement_id"], json!(f.machine));
    assert_eq!(licence["instance_key_id"], json!(key));
    assert_eq!(licence["superseded_at"], Value::Null);
    let s = &r["station"];
    assert_eq!(s["id"], json!(station));
    assert_eq!(&s["licence"], licence);
    assert_eq!(s["renewal_available"], false);
    assert_eq!(s["released_at"], Value::Null);
    assert_eq!(s["station_id"], "bench-1");
    assert_eq!(s["label"], "Bench 1");

    // Same seat, same registration: one station, one more licence, two
    // audit rows.
    assert_eq!(
        f.counts().await,
        (stations_before, licences_before + 1, audits_before + 2)
    );
    let seats_after = ok_json(
        f.hub.get("/api/entitlements", Some(&f.owner.cookie)).await,
        StatusCode::OK,
    )
    .await["available_seats"]
        .clone();
    assert_eq!(seats_after, seats_before);
    assert_eq!(f.stations(&f.owner).await.len(), 1);

    // The old licence is superseded, still downloadable byte for byte.
    let old_status: (String, bool) = sqlx::query_as(
        "SELECT status, superseded_at IS NOT NULL FROM licences WHERE id = $1::uuid",
    )
    .bind(&old)
    .fetch_one(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(old_status, ("superseded".to_string(), true));
    let resp = f.download(&f.member, &old).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), old_file);

    // The new file installs and mirrors the entitlement, to the new term end.
    let file = f.download(&f.member, new).await.text().await.unwrap();
    assert_eq!(file, f.stored_document(new).await);
    let cert = install(&file, &key, &f.root);
    let old_cert = install(&old_file, &key, &f.root);
    assert_eq!(cert.body.license_id, new);
    assert_eq!(cert.body.instance_public_key_id, key);
    let ent = &cert.body.entitlements;
    let old_ent = &old_cert.body.entitlements;
    assert_eq!(ent.not_after, renewed_end);
    assert_eq!(old_ent.not_after, term_end);
    assert_eq!(ent.not_before, cert.body.issued_at);
    assert_eq!(ent.customer_id, old_ent.customer_id);
    assert_eq!(ent.customer_name, old_ent.customer_name);
    assert_eq!(ent.quota, old_ent.quota);
    assert_eq!(ent.scope, old_ent.scope);
    assert_eq!(
        ent.scope,
        Scope::Machine {
            fingerprint: machine_fingerprint("bench-1")
        }
    );
    assert_eq!(ent.features, old_ent.features);
    assert_eq!(ent.allowed_methods, old_ent.allowed_methods);
    assert_eq!(ent.machine_binding, None);

    // Now, before its own not_after, the superseded old file still works.
    let now = OffsetDateTime::now_utc();
    assert!(now < term_end);
    let status = evaluate(
        Some(old_ent),
        &LeaseState::new(now),
        &machine_fingerprint("bench-1"),
        now,
    );
    assert!(matches!(status, LeaseStatus::Valid { .. }), "{status:?}");

    // After the old term end, before the new one: the renewed file is valid
    // and the old one has expired.
    let later = term_end + time::Duration::days(30);
    assert!(later < renewed_end);
    let fingerprint = machine_fingerprint("bench-1");
    let status = evaluate(Some(ent), &LeaseState::new(later), &fingerprint, later);
    assert!(matches!(status, LeaseStatus::Valid { .. }), "{status:?}");
    let status = evaluate(Some(old_ent), &LeaseState::new(later), &fingerprint, later);
    assert!(matches!(status, LeaseStatus::Expired { .. }), "{status:?}");

    // Audit.
    let actor = format!("user:{}", f.owner.id);
    assert_eq!(
        f.hub.audited("licence.superseded").await,
        [(
            actor.clone(),
            format!("licence:{old}"),
            json!({ "station_id": station, "reason": "renewed", "superseded_by": new }),
        )]
    );
    let issued = f.hub.audited("licence.issued").await;
    assert_eq!(issued.len(), 2);
    assert_eq!(
        issued[1],
        (
            actor,
            format!("licence:{new}"),
            json!({
                "station_id": station,
                "entitlement_id": f.machine,
                "instance_key_id": key,
                "root_key_id": f.root.public_key_id(),
                "not_after": licence["not_after"],
                "reissue_of": old,
            }),
        )
    );

    // Now up to date: a second reissue is refused and writes nothing.
    let before = f.counts().await;
    let resp = f.reissue(&f.owner, station).await;
    assert_error(resp, StatusCode::CONFLICT, MSG_LICENCE_UP_TO_DATE).await;
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn a_site_station_is_reissued_for_the_same_site() {
    let f = fixture().await;
    let mut registered = Vec::new();
    for (site, want) in [
        (" Dock 4 ", "Dock 4".to_string()),
        ("", f.org_id.to_string()),
    ] {
        let key = new_key();
        let v = ok_json(
            f.register(
                &f.owner,
                json!({ "entitlement_id": f.site, "instance_key_id": key, "site": site }),
            )
            .await,
            StatusCode::CREATED,
        )
        .await;
        registered.push((v["station"]["id"].as_str().unwrap().to_string(), key, want));
    }
    // A one-off entitlement never renews; force the state.
    f.extend_term(&f.site).await;
    let term_end = f.term_end(&f.site).await;
    for (station, key, site_id) in registered {
        assert_eq!(
            f.station(&f.owner, &station).await["renewal_available"],
            true
        );
        let r = ok_json(f.reissue(&f.owner, &station).await, StatusCode::CREATED).await;
        let file = f
            .stored_document(r["licence"]["id"].as_str().unwrap())
            .await;
        let cert = install(&file, &key, &f.root);
        let ent = &cert.body.entitlements;
        assert_eq!(ent.scope, Scope::Site { site_id });
        assert_eq!(ent.quota, Quota::Count { erasures: 5000 });
        assert_eq!(ent.features, [Feature::PdfCertificates]);
        assert_eq!(ent.not_after, term_end);
        assert_eq!(r["station"]["renewal_available"], false);
    }
}

#[tokio::test]
async fn a_licence_already_at_term_end_is_up_to_date() {
    let f = fixture().await;
    let v = f.register_machine(&new_key()).await;
    let station = v["station"]["id"].as_str().unwrap();
    let before = f.counts().await;
    let resp = f.reissue(&f.owner, station).await;
    assert_error(resp, StatusCode::CONFLICT, MSG_LICENCE_UP_TO_DATE).await;
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn a_released_station_is_not_reissued() {
    let f = fixture().await;
    let v = f.register_machine(&new_key()).await;
    let station = v["station"]["id"].as_str().unwrap();
    ok_json(f.release(&f.owner, station).await, StatusCode::OK).await;
    f.renew(&f.machine).await;
    assert_eq!(
        f.station(&f.owner, station).await["renewal_available"],
        false
    );
    let before = f.counts().await;
    let resp = f.reissue(&f.owner, station).await;
    assert_error(resp, StatusCode::CONFLICT, MSG_ALREADY_RELEASED).await;
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn a_station_whose_entitlement_is_not_current_is_not_reissued() {
    let f = fixture().await;
    let pool = &f.hub.db.pool;
    let machine = f.register_machine(&new_key()).await["station"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let site = ok_json(
        f.register(
            &f.owner,
            json!({ "entitlement_id": f.site, "instance_key_id": new_key() }),
        )
        .await,
        StatusCode::CREATED,
    )
    .await["station"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Renewed, then refunded.
    f.extend_term(&f.site).await;
    sqlx::query(
        "UPDATE entitlements SET status = 'refunded', refunded_at = now(), renews = false \
         WHERE id = $1::uuid",
    )
    .bind(&f.site)
    .execute(pool)
    .await
    .unwrap();
    // Ended: the term moved into the past (the trigger refuses this to the
    // hub, so it is lifted for the test).
    sqlx::query("ALTER TABLE entitlements DISABLE TRIGGER entitlements_lifecycle")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE entitlements SET term_start = term_start - interval '2 years', \
                term_end = term_end - interval '2 years' WHERE id = $1::uuid",
    )
    .bind(&f.machine)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE entitlements ENABLE TRIGGER entitlements_lifecycle")
        .execute(pool)
        .await
        .unwrap();
    let before = f.counts().await;
    for station in [&machine, &site] {
        assert_eq!(
            f.station(&f.owner, station).await["renewal_available"],
            false
        );
        let resp = f.reissue(&f.owner, station).await;
        assert_error(resp, StatusCode::CONFLICT, MSG_ENTITLEMENT_NOT_CURRENT).await;
    }
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn an_unknown_foreign_or_malformed_station_is_not_reissued() {
    let f = fixture().await;
    let v = f.register_machine(&new_key()).await;
    let station = v["station"]["id"].as_str().unwrap();
    f.renew(&f.machine).await;
    let other = f.hub.verified_owner("Other Org").await;
    let before = f.counts().await;
    let resp = f.reissue(&other, station).await;
    assert_error(resp, StatusCode::NOT_FOUND, MSG_NO_STATION).await;
    for id in [Uuid::new_v4().to_string(), "not-a-uuid".to_string()] {
        let resp = f.reissue(&f.owner, &id).await;
        assert_error(resp, StatusCode::NOT_FOUND, MSG_NO_STATION).await;
    }
    assert_eq!(f.counts().await, before);
    assert_eq!(
        f.station(&f.owner, station).await["renewal_available"],
        true
    );
}

#[tokio::test]
async fn concurrent_reissues_of_one_station_issue_one_licence() {
    let f = fixture().await;
    let v = f.register_machine(&new_key()).await;
    let station = v["station"]["id"].as_str().unwrap();
    f.renew(&f.machine).await;
    let calls = (0..8).map(|_| f.reissue(&f.owner, station)).collect();
    let mut created = 0;
    for resp in futures_join_all(calls).await {
        match resp.status() {
            StatusCode::CREATED => created += 1,
            _ => assert_error(resp, StatusCode::CONFLICT, MSG_LICENCE_UP_TO_DATE).await,
        }
    }
    assert_eq!(created, 1);
    assert_eq!(f.hub.count("licences").await, 2);
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM licences WHERE station_id = $1::uuid AND status = 'active'",
    )
    .bind(station)
    .fetch_one(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(active, 1);
    assert_eq!(f.hub.audited("licence.superseded").await.len(), 1);
}
