//! The offerings admin area (offerings-and-payments story 1, CAP-1):
//! `/api/admin/offerings*` create, edit, publish, retire, list and view,
//! each audited in its own transaction. One test per row of the story's I/O
//! matrix, through HTTP, checking the response, the stored row and the audit
//! rows. Needs `docker compose up -d`; fails, never skips, when it is down.

mod support;

use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{assert_error, bootstrap_admin, spawn_hub, Hub, Who};
use uuid::Uuid;
use wipe_hub::admin_routes::MSG_NO_OFFERING;
use wipe_hub::offering_admin::{
    MSG_DUPLICATE_NAME, MSG_PUBLISH_NOT_DRAFT, MSG_RETIRED, MSG_RETIRE_NOT_PUBLISHED,
    MSG_TERMS_FROZEN,
};

const ADMIN_ONLY: &str = "only vendor admins can use the admin area";
const NOT_SIGNED_IN: &str = "not signed in";

struct Fixture {
    hub: Hub,
    admin: Who,
}

async fn fixture() -> Fixture {
    let hub = spawn_hub().await;
    let admin = bootstrap_admin(&hub).await;
    Fixture { hub, admin }
}

/// A valid create body.
fn body(name: &str) -> Value {
    json!({
        "name": name,
        "description": "One station, unlimited erasures.",
        "price_minor": 120000,
        "currency": "usd",
        "interval": "annual",
        "grants": {
            "scope": "machine",
            "seats": 1,
            "quota_erasures": null,
            "features": ["pdf_certificates", "hub_sync", "pdf_certificates"],
            "term_months": 12
        }
    })
}

impl Fixture {
    async fn get(&self, path: &str) -> Response {
        self.hub.get(path, Some(&self.admin.cookie)).await
    }

    async fn post(&self, path: &str, body: Value) -> Response {
        self.hub.post(path, body, Some(&self.admin.cookie)).await
    }

    async fn post_empty(&self, path: &str) -> Response {
        self.hub.post_empty(path, Some(&self.admin.cookie)).await
    }

    /// Create `body`, expecting 201; the offering.
    async fn create(&self, body: Value) -> Value {
        let resp = self.post("/api/admin/offerings", body).await;
        let code = resp.status();
        let v: Value = resp.json().await.unwrap();
        assert_eq!(code, StatusCode::CREATED, "{v}");
        v["offering"].clone()
    }

    async fn publish(&self, id: &str) -> Response {
        self.post_empty(&format!("/api/admin/offerings/{id}/publish"))
            .await
    }

    async fn retire(&self, id: &str) -> Response {
        self.post_empty(&format!("/api/admin/offerings/{id}/retire"))
            .await
    }

    /// A published offering's id.
    async fn published(&self, name: &str) -> String {
        let id = id_of(&self.create(body(name)).await);
        ok_json(self.publish(&id).await).await;
        id
    }

    /// Every audit row for `action`, checking the actor; `(subject, detail)`.
    async fn audits(&self, action: &str) -> Vec<(String, Value)> {
        let rows = self.hub.audited(action).await;
        for row in &rows {
            assert_eq!(row.0, format!("user:{}", self.admin.id), "{action}");
        }
        rows.into_iter().map(|r| (r.1, r.2)).collect()
    }

    /// The stored row as JSON, every column.
    async fn stored(&self, id: &str) -> Value {
        sqlx::query_scalar("SELECT row_to_json(o) FROM offerings o WHERE id = $1::uuid")
            .bind(id)
            .fetch_one(&self.hub.db.pool)
            .await
            .unwrap()
    }

    async fn audit_count(&self) -> i64 {
        self.hub.count("audit_log").await
    }
}

async fn ok_json(resp: Response) -> Value {
    let code = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(code, StatusCode::OK, "{body}");
    body
}

fn id_of(offering: &Value) -> String {
    offering["id"].as_str().unwrap().to_string()
}

fn subject(id: &str) -> String {
    format!("offering:{id}")
}

#[tokio::test]
async fn create_returns_a_normalised_draft_and_audits_it_in_full() {
    let f = fixture().await;
    let before = f.audit_count().await;
    let o = f.create(body("Station — annual, 1 seat")).await;
    let id = id_of(&o);
    assert_eq!(o["status"], "draft");
    assert_eq!(o["name"], "Station — annual, 1 seat");
    assert_eq!(o["currency"], "USD");
    assert_eq!(o["price_minor"], 120000);
    assert_eq!(o["interval"], "annual");
    assert_eq!(
        o["grants"],
        json!({"scope": "machine", "seats": 1, "quota_erasures": null,
               "features": ["hub_sync", "pdf_certificates"], "term_months": 12})
    );
    for k in ["created_at", "updated_at"] {
        assert!(o[k].as_str().unwrap().ends_with('Z'), "{k}");
    }
    assert_eq!(o["published_at"], Value::Null);
    assert_eq!(o["retired_at"], Value::Null);
    let keys: Vec<&String> = o.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 12, "{keys:?}");

    let row = f.stored(&id).await;
    assert_eq!(row["status"], "draft");
    assert_eq!(row["currency"], "USD");
    assert_eq!(row["billing_interval"], "annual");
    assert_eq!(row["features"], json!(["hub_sync", "pdf_certificates"]));
    assert_eq!(row["created_by"], json!(f.admin.id));

    assert_eq!(f.audit_count().await, before + 1);
    let audits = f.audits("admin.offering_created").await;
    assert_eq!(audits, [(subject(&id), json!({ "offering": o }))]);
}

#[tokio::test]
async fn invalid_bodies_are_400_naming_the_field_with_no_row_and_no_audit() {
    let f = fixture().await;
    let before = f.audit_count().await;
    let cases: Vec<(&str, Value, &str)> = vec![
        ("/name", json!(" "), "name"),
        ("/name", json!("x".repeat(101)), "name"),
        ("/description", json!("x".repeat(2001)), "description"),
        ("/price_minor", json!(0), "price_minor"),
        ("/price_minor", json!(-5), "price_minor"),
        ("/price_minor", json!(100_000_000), "price_minor"),
        ("/price_minor", json!(12.5), "price_minor"),
        ("/currency", json!("US"), "currency"),
        ("/currency", json!("US1"), "currency"),
        ("/interval", json!("monthly"), "interval"),
        ("/grants/scope", json!("fleet"), "grants.scope"),
        ("/grants/seats", json!(0), "grants.seats"),
        ("/grants/seats", json!(10_001), "grants.seats"),
        ("/grants/quota_erasures", json!(0), "grants.quota_erasures"),
        ("/grants/features", json!(["teleport"]), "grants.features"),
        ("/grants/term_months", json!(24), "grants.term_months"),
        ("/status", json!("published"), "status"),
        ("/grants/fingerprint", json!("abc"), "fingerprint"),
    ];
    for (pointer, value, field) in cases {
        let mut b = body("Valid name");
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        b.pointer_mut(if parent.is_empty() { "" } else { parent })
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.to_string(), value.clone());
        let resp = f.post("/api/admin/offerings", b).await;
        let code = resp.status();
        let err: Value = resp.json().await.unwrap();
        assert_eq!(code, StatusCode::BAD_REQUEST, "{pointer}={value}: {err}");
        let msg = err["error"].as_str().unwrap();
        assert!(msg.contains(field), "{pointer}={value}: {msg}");
    }
    // One-off terms out of range.
    for term in [0, 121] {
        let mut b = body("Valid name");
        b["interval"] = json!("one_off");
        b["grants"]["term_months"] = json!(term);
        let resp = f.post("/api/admin/offerings", b).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{term}");
    }
    // A missing quota is not silently unlimited.
    let mut b = body("Valid name");
    b["grants"]
        .as_object_mut()
        .unwrap()
        .remove("quota_erasures");
    let resp = f.post("/api/admin/offerings", b).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(f.hub.count("offerings").await, 0);
    assert_eq!(f.audit_count().await, before);
}

#[tokio::test]
async fn a_name_in_draft_or_for_sale_is_409_until_retired() {
    let f = fixture().await;
    let published = f.published("Station").await;
    f.create(body("Draft one")).await;
    let before = f.audit_count().await;
    for name in ["station", "  STATION ", "draft ONE"] {
        assert_error(
            f.post("/api/admin/offerings", body(name)).await,
            StatusCode::CONFLICT,
            MSG_DUPLICATE_NAME,
        )
        .await;
    }
    assert_eq!(f.audit_count().await, before);
    assert_eq!(f.hub.count("offerings").await, 2);
    // Renaming a draft onto a taken name is refused too.
    let other = id_of(&f.create(body("Other")).await);
    let before = f.audit_count().await;
    assert_error(
        f.post(&format!("/api/admin/offerings/{other}"), body("Station"))
            .await,
        StatusCode::CONFLICT,
        MSG_DUPLICATE_NAME,
    )
    .await;
    assert_eq!(f.audit_count().await, before);
    // Retiring frees the name.
    ok_json(f.retire(&published).await).await;
    f.create(body("Station")).await;
}

#[tokio::test]
async fn editing_a_draft_audits_only_the_changed_fields() {
    let f = fixture().await;
    let o = f.create(body("Station")).await;
    let id = id_of(&o);
    let path = format!("/api/admin/offerings/{id}");

    let mut edit = body("Station Pro");
    edit["price_minor"] = json!(99000);
    edit["grants"]["seats"] = json!(3);
    let after = ok_json(f.post(&path, edit).await).await["offering"].clone();
    assert_eq!(after["name"], "Station Pro");
    assert_eq!(after["price_minor"], 99000);
    assert_eq!(after["grants"]["seats"], 3);
    assert_eq!(after["status"], "draft");
    let row = f.stored(&id).await;
    assert_eq!(row["name"], "Station Pro");
    assert_eq!(row["seats"], 3);
    let mut grants_after = o["grants"].clone();
    grants_after["seats"] = json!(3);
    assert_eq!(
        f.audits("admin.offering_updated").await,
        [(
            subject(&id),
            json!({"changes": {
                "name": {"from": "Station", "to": "Station Pro"},
                "price_minor": {"from": 120000, "to": 99000},
                "grants": {"from": o["grants"], "to": grants_after},
            }})
        )]
    );

    // The same body again changes nothing and is audited as a view.
    let mut same = body("Station Pro");
    same["price_minor"] = json!(99000);
    same["grants"]["seats"] = json!(3);
    let before = f.audit_count().await;
    let again = ok_json(f.post(&path, same).await).await["offering"].clone();
    assert_eq!(again, after);
    assert_eq!(f.audit_count().await, before + 1);
    assert_eq!(f.audits("admin.offering_updated").await.len(), 1);
    assert_eq!(
        f.audits("admin.offering_viewed").await,
        [(subject(&id), json!({}))]
    );
}

#[tokio::test]
async fn a_published_offerings_terms_never_change_and_nothing_is_written() {
    let f = fixture().await;
    let id = f.published("Station").await;
    let path = format!("/api/admin/offerings/{id}");
    let row = f.stored(&id).await;
    let audits = f.audit_count().await;
    let mut edits = Vec::new();
    for (key, value) in [
        ("price_minor", json!(99000)),
        ("currency", json!("EUR")),
        ("interval", json!("one_off")),
    ] {
        let mut b = body("Station");
        b[key] = value;
        edits.push(b);
    }
    for (key, value) in [
        ("scope", json!("site")),
        ("seats", json!(2)),
        ("quota_erasures", json!(500)),
        ("features", json!([])),
    ] {
        let mut b = body("Station");
        b["grants"][key] = value;
        edits.push(b);
    }
    // A copy edit with a term change is refused whole.
    let mut b = body("Station renamed");
    b["price_minor"] = json!(1);
    edits.push(b);
    for b in edits {
        assert_error(
            f.post(&path, b).await,
            StatusCode::CONFLICT,
            MSG_TERMS_FROZEN,
        )
        .await;
    }
    assert_eq!(f.stored(&id).await, row);
    assert_eq!(f.audit_count().await, audits);
}

#[tokio::test]
async fn a_published_offerings_copy_may_change() {
    let f = fixture().await;
    let id = f.published("Station").await;
    let mut b = body("Station (2026)");
    b["description"] = json!("Now with more words.");
    let o =
        ok_json(f.post(&format!("/api/admin/offerings/{id}"), b).await).await["offering"].clone();
    assert_eq!(o["status"], "published");
    assert_eq!(o["name"], "Station (2026)");
    assert_eq!(o["description"], "Now with more words.");
    assert_eq!(f.stored(&id).await["name"], "Station (2026)");
    assert_eq!(
        f.audits("admin.offering_updated").await,
        [(
            subject(&id),
            json!({"changes": {
                "name": {"from": "Station", "to": "Station (2026)"},
                "description": {"from": "One station, unlimited erasures.",
                                 "to": "Now with more words."},
            }})
        )]
    );
}

#[tokio::test]
async fn a_retired_offering_cannot_change() {
    let f = fixture().await;
    let id = f.published("Station").await;
    ok_json(f.retire(&id).await).await;
    let row = f.stored(&id).await;
    let audits = f.audit_count().await;
    let mut renamed = body("Station");
    renamed["name"] = json!("Renamed");
    // Even the unchanged body: a retired offering takes no edits at all.
    for b in [body("Station"), renamed] {
        assert_error(
            f.post(&format!("/api/admin/offerings/{id}"), b).await,
            StatusCode::CONFLICT,
            MSG_RETIRED,
        )
        .await;
    }
    assert_error(
        f.publish(&id).await,
        StatusCode::CONFLICT,
        MSG_PUBLISH_NOT_DRAFT,
    )
    .await;
    assert_error(
        f.retire(&id).await,
        StatusCode::CONFLICT,
        MSG_RETIRE_NOT_PUBLISHED,
    )
    .await;
    assert_eq!(f.stored(&id).await, row);
    assert_eq!(f.audit_count().await, audits);
}

#[tokio::test]
async fn publishing_a_draft_stamps_it_once() {
    let f = fixture().await;
    let id = id_of(&f.create(body("Station")).await);
    let before = f.audit_count().await;
    let o = ok_json(f.publish(&id).await).await["offering"].clone();
    assert_eq!(o["status"], "published");
    assert!(o["published_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(o["retired_at"], Value::Null);
    assert_eq!(f.stored(&id).await["status"], "published");
    assert_eq!(f.audit_count().await, before + 1);
    assert_eq!(
        f.audits("admin.offering_published").await,
        [(subject(&id), json!({}))]
    );
    let row = f.stored(&id).await;
    assert_error(
        f.publish(&id).await,
        StatusCode::CONFLICT,
        MSG_PUBLISH_NOT_DRAFT,
    )
    .await;
    assert_eq!(f.stored(&id).await, row);
    assert_eq!(f.audit_count().await, before + 1);
}

#[tokio::test]
async fn retiring_needs_a_published_offering() {
    let f = fixture().await;
    let draft = id_of(&f.create(body("Draft")).await);
    let before = f.audit_count().await;
    assert_error(
        f.retire(&draft).await,
        StatusCode::CONFLICT,
        MSG_RETIRE_NOT_PUBLISHED,
    )
    .await;
    assert_eq!(f.stored(&draft).await["status"], "draft");
    assert_eq!(f.audit_count().await, before);

    let id = f.published("Station").await;
    let before = f.audit_count().await;
    let o = ok_json(f.retire(&id).await).await["offering"].clone();
    assert_eq!(o["status"], "retired");
    assert!(o["retired_at"].as_str().unwrap().ends_with('Z'));
    assert!(o["published_at"].is_string());
    assert_eq!(f.stored(&id).await["status"], "retired");
    assert_eq!(f.audit_count().await, before + 1);
    assert_eq!(
        f.audits("admin.offering_retired").await,
        [(subject(&id), json!({}))]
    );
}

#[tokio::test]
async fn the_list_shows_every_status_in_order_and_is_audited() {
    let f = fixture().await;
    let retired = f.published("alpha retired").await;
    ok_json(f.retire(&retired).await).await;
    let pub_b = f.published("Bravo").await;
    let pub_a = f.published("alpha").await;
    let draft_z = id_of(&f.create(body("zulu")).await);
    let draft_c = id_of(&f.create(body("Charlie")).await);
    let before = f.audit_count().await;
    let list = ok_json(f.get("/api/admin/offerings").await).await;
    let ids: Vec<String> = list["offerings"]
        .as_array()
        .unwrap()
        .iter()
        .map(id_of)
        .collect();
    let want = [draft_c, draft_z, pub_a, pub_b, retired];
    assert_eq!(ids, want);
    assert_eq!(f.audit_count().await, before + 1);
    assert_eq!(
        f.audits("admin.offerings_listed").await,
        [(
            "search:offerings".to_string(),
            json!({ "offering_ids": want })
        )]
    );
}

#[tokio::test]
async fn viewing_is_audited_and_unknown_ids_are_404_without_audit() {
    let f = fixture().await;
    let o = f.create(body("Station")).await;
    let id = id_of(&o);
    let before = f.audit_count().await;
    let got = ok_json(f.get(&format!("/api/admin/offerings/{id}")).await).await;
    assert_eq!(got, json!({ "offering": o }));
    assert_eq!(
        f.audits("admin.offering_viewed").await,
        [(subject(&id), json!({}))]
    );
    assert_eq!(f.audit_count().await, before + 1);

    let unknown = Uuid::new_v4().to_string();
    for bad in [unknown.as_str(), "not-a-uuid"] {
        let path = format!("/api/admin/offerings/{bad}");
        assert_error(f.get(&path).await, StatusCode::NOT_FOUND, MSG_NO_OFFERING).await;
        assert_error(
            f.post(&path, body("Other")).await,
            StatusCode::NOT_FOUND,
            MSG_NO_OFFERING,
        )
        .await;
        assert_error(f.publish(bad).await, StatusCode::NOT_FOUND, MSG_NO_OFFERING).await;
        assert_error(f.retire(bad).await, StatusCode::NOT_FOUND, MSG_NO_OFFERING).await;
    }
    assert_eq!(f.audit_count().await, before + 1);
}

#[tokio::test]
async fn customers_and_anonymous_callers_are_refused_on_every_offering_route() {
    let f = fixture().await;
    let id = id_of(&f.create(body("Station")).await);
    let row = f.stored(&id).await;
    let hub = &f.hub;
    let owner = hub.verified_owner("Acme").await;
    let member = hub.add(&owner, "member").await;
    let rows = hub.count("audit_log").await;
    let gets = [
        "/api/admin/offerings".to_string(),
        format!("/api/admin/offerings/{id}"),
    ];
    let posts = [
        ("/api/admin/offerings".to_string(), body("Sneaky")),
        (format!("/api/admin/offerings/{id}"), body("Sneaky")),
        (format!("/api/admin/offerings/{id}/publish"), json!({})),
        (format!("/api/admin/offerings/{id}/retire"), json!({})),
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
        assert_error(
            hub.get("/api/admin/offerings/not-a-uuid", cookie).await,
            code,
            message,
        )
        .await;
        for (path, b) in &posts {
            assert_error(hub.post(path, b.clone(), cookie).await, code, message).await;
            // Refused before the body is read.
            assert_error(hub.post_empty(path, cookie).await, code, message).await;
        }
    }
    assert_eq!(hub.count("audit_log").await, rows);
    assert_eq!(hub.count("offerings").await, 1);
    assert_eq!(f.stored(&id).await, row);
}

#[tokio::test]
async fn the_schema_refuses_what_the_hub_refuses() {
    let f = fixture().await;
    let id = f.published("Station").await;
    let pool = &f.hub.db.pool;
    for sql in [
        "UPDATE offerings SET price_minor = 1 WHERE id = $1::uuid",
        "UPDATE offerings SET features = '{}' WHERE id = $1::uuid",
        "UPDATE offerings SET status = 'draft', published_at = NULL WHERE id = $1::uuid",
        "DELETE FROM offerings WHERE id = $1::uuid",
    ] {
        let r = sqlx::query(sql).bind(&id).execute(pool).await;
        assert!(r.is_err(), "{sql}");
    }
    sqlx::query("UPDATE offerings SET name = 'Renamed' WHERE id = $1::uuid")
        .bind(&id)
        .execute(pool)
        .await
        .unwrap();
    ok_json(f.retire(&id).await).await;
    let r = sqlx::query("UPDATE offerings SET name = 'Again' WHERE id = $1::uuid")
        .bind(&id)
        .execute(pool)
        .await;
    assert!(r.is_err());
    let r = sqlx::query(
        "INSERT INTO offerings (name, price_minor, currency, billing_interval, scope, seats, \
                                term_months) \
         VALUES ('Bad', 100, 'USD', 'annual', 'site', 1, 6)",
    )
    .execute(pool)
    .await;
    assert!(r.is_err(), "annual offerings have a 12-month term");
}

#[tokio::test]
async fn create_publish_retire_each_write_exactly_one_audit_row() {
    let f = fixture().await;
    let start = f.audit_count().await;
    let o = f.create(body("Station")).await;
    let id = id_of(&o);
    assert_eq!(f.audit_count().await, start + 1);
    assert_eq!(
        ok_json(f.publish(&id).await).await["offering"]["status"],
        "published"
    );
    assert_eq!(f.audit_count().await, start + 2);
    assert_eq!(
        ok_json(f.retire(&id).await).await["offering"]["status"],
        "retired"
    );
    assert_eq!(f.audit_count().await, start + 3);
    let actions = f.hub.actions().await;
    assert_eq!(
        &actions[actions.len() - 3..],
        [
            "admin.offering_created",
            "admin.offering_published",
            "admin.offering_retired"
        ]
    );
}

/// A terms edit started while another transaction holds the draft `FOR
/// UPDATE` and then publishes it must see the published row (409), never
/// write over it or trip the schema's trigger (500).
#[tokio::test]
async fn an_edit_racing_a_publish_is_a_409_not_a_500() {
    let f = fixture().await;
    let id = id_of(&f.create(body("Station")).await);
    let rows = f.audit_count().await;

    let mut holder = f.hub.db.pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM offerings WHERE id = $1::uuid FOR UPDATE")
        .bind(&id)
        .execute(&mut *holder)
        .await
        .unwrap();

    let mut edit = body("Station");
    edit["price_minor"] = json!(99000);
    let req = f
        .hub
        .post_req(&format!("/api/admin/offerings/{id}"), Some(&f.admin.cookie))
        .json(&edit);
    let mutation = tokio::spawn(async move { req.send().await.unwrap() });

    // Wait until the edit's lock is queued behind the holder.
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
            "the edit never waited on the lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    sqlx::query(
        "UPDATE offerings SET status = 'published', published_at = now() WHERE id = $1::uuid",
    )
    .bind(&id)
    .execute(&mut *holder)
    .await
    .unwrap();
    holder.commit().await.unwrap();

    assert_error(
        mutation.await.unwrap(),
        StatusCode::CONFLICT,
        MSG_TERMS_FROZEN,
    )
    .await;
    assert_eq!(f.stored(&id).await["price_minor"], 120000);
    assert_eq!(f.audit_count().await, rows);
}
