//! The sales-lead admin area (lead-capture story 2, CAP-3 and the erasure
//! half of CAP-4): `/api/admin/leads*` list, view, status, notes, CSV export
//! and erasure, each audited without personal data. Leads are created
//! through `POST /api/leads` as the marketing site does. One test per row of
//! the story's I/O matrix. Needs `docker compose up -d`; fails, never skips,
//! when it is down.

mod support;

use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{assert_error, bootstrap_admin, mail_texts, spawn_hub, token_in, Hub, Who};
use support::unique_address;
use uuid::Uuid;
use wipe_hub::leads::{CONSENT_CONTACT, CONSENT_EARLY_ACCESS};
use wipe_hub::mail::LEAD_EARLY_ACCESS_SUBJECT;

const ADMIN_ONLY: &str = "only vendor admins can use the admin area";
const NOT_SIGNED_IN: &str = "not signed in";
const NO_LEAD: &str = "no such sales lead";
const BAD_FILTER: &str = wipe_hub::admin_routes::MSG_BAD_FILTER;
const SITE: &str = wipe_hub::config::DEFAULT_SITE_ORIGIN;

/// A lead as submitted; its fields are unique to the test so a leak into an
/// audit row cannot hide behind a shared value.
struct Sub {
    email: String,
    name: String,
    company: String,
    message: String,
}

fn sub() -> Sub {
    let tag = Uuid::new_v4().simple().to_string();
    Sub {
        email: unique_address(),
        name: format!("Ada {tag}"),
        company: format!("Engines {tag}"),
        message: format!("Please call me {tag}"),
    }
}

/// `POST /api/leads` from the site (early access when `segment` is given,
/// contact otherwise); returns the lead's id.
async fn submit(hub: &Hub, s: &Sub, segment: Option<&str>) -> Uuid {
    let body = match segment {
        Some(segment) => json!({
            "kind": "early-access",
            "name": s.name,
            "email": s.email,
            "company": s.company,
            "role": "Head of operations",
            "segment": segment,
            "message": s.message,
            "consent": true,
            "consent_text": CONSENT_EARLY_ACCESS,
        }),
        None => json!({
            "kind": "contact",
            "name": s.name,
            "email": s.email,
            "company": s.company,
            "message": s.message,
            "consent": true,
            "consent_text": CONSENT_CONTACT,
        }),
    };
    let resp = hub
        .post_req("/api/leads", None)
        .header("origin", SITE)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    sqlx::query_scalar("SELECT id FROM leads WHERE lower(email) = lower($1)")
        .bind(&s.email)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
}

struct Fixture {
    hub: Hub,
    admin: Who,
}

async fn fixture() -> Fixture {
    let hub = spawn_hub().await;
    let admin = bootstrap_admin(&hub).await;
    Fixture { hub, admin }
}

impl Fixture {
    async fn get(&self, path: &str) -> Response {
        self.hub.get(path, Some(&self.admin.cookie)).await
    }

    async fn post(&self, path: &str, body: Value) -> Response {
        self.hub.post(path, body, Some(&self.admin.cookie)).await
    }

    async fn list(&self, query: &str) -> Value {
        ok_json(self.get(&format!("/api/admin/leads{query}")).await).await
    }

    async fn listed_ids(&self, query: &str) -> Vec<Uuid> {
        ids(&self.list(query).await["leads"])
    }

    /// Every audit row for `action`, checking actor and subject.
    async fn audits(&self, action: &str, subject: &str) -> Vec<Value> {
        let rows = self.hub.audited(action).await;
        for row in &rows {
            assert_eq!(row.0, format!("user:{}", self.admin.id), "{action}");
            assert_eq!(row.1, subject, "{action}");
        }
        rows.into_iter().map(|r| r.2).collect()
    }
}

async fn ok_json(resp: Response) -> Value {
    let code = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(code, StatusCode::OK, "{body}");
    body
}

fn ids(list: &Value) -> Vec<Uuid> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|x| Uuid::parse_str(x["id"].as_str().unwrap()).unwrap())
        .collect()
}

fn uuids(v: &Value) -> Vec<Uuid> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| Uuid::parse_str(x.as_str().unwrap()).unwrap())
        .collect()
}

/// Today's UTC date, `YYYY-MM-DD`, by the database clock. Tests read it
/// once, before submitting fixtures, and derive every other date from it
/// with [`shift`], so a run across UTC midnight cannot fail.
async fn utc_today(hub: &Hub) -> String {
    sqlx::query_scalar("SELECT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD')")
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
}

/// `date` moved by `days` (negative is earlier), as `YYYY-MM-DD`.
async fn shift(hub: &Hub, date: &str, days: i32) -> String {
    sqlx::query_scalar("SELECT to_char($1::date + $2, 'YYYY-MM-DD')")
        .bind(date)
        .bind(days)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
}

/// Set every submission of `lead` to noon UTC on `date`.
async fn submitted_on(hub: &Hub, lead: Uuid, date: &str) {
    submitted_at(hub, lead, date, "12:00:00").await;
}

/// Set every submission of `lead` to `time` (`HH:MM:SS`) UTC on `date`.
async fn submitted_at(hub: &Hub, lead: Uuid, date: &str, time: &str) {
    sqlx::query(
        "UPDATE lead_submissions \
         SET consented_at = ($2::date + $3::time) AT TIME ZONE 'UTC' WHERE lead_id = $1",
    )
    .bind(lead)
    .bind(date)
    .bind(time)
    .execute(&hub.db.pool)
    .await
    .unwrap();
}

/// A CSV export's text: checks the response, its UTF-8 byte-order mark,
/// and strips the mark. Read as bytes, since `text()` would drop it.
async fn csv_of(resp: Response) -> String {
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.bytes().await.unwrap();
    let rest = bytes
        .strip_prefix(b"\xef\xbb\xbf")
        .expect("the CSV starts with a UTF-8 BOM");
    String::from_utf8(rest.to_vec()).unwrap()
}

/// The `mail:to:` email-send throttle row keys for `email`.
async fn mail_throttle_rows(hub: &Hub, email: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM auth_throttle WHERE key = 'mail:to:' || lower($1)")
        .bind(email)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_list_filters_by_segment_and_status_and_is_audited_with_the_ids() {
    let f = fixture().await;
    let itad = submit(&f.hub, &sub(), Some("itad")).await;
    let gov = submit(&f.hub, &sub(), Some("government")).await;
    let contact = submit(&f.hub, &sub(), None).await;
    sqlx::query("UPDATE leads SET status = 'contacted' WHERE id = $1")
        .bind(gov)
        .execute(&f.hub.db.pool)
        .await
        .unwrap();

    let body = f.list("?segment=itad").await;
    assert_eq!(ids(&body["leads"]), [itad]);
    assert_eq!(body["truncated"], false);
    let rows = f.audits("admin.leads_listed", "search:leads").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["filter"],
        json!({"segment": "itad", "status": null, "from": null, "to": null})
    );
    assert_eq!(uuids(&rows[0]["lead_ids"]), [itad]);

    assert_eq!(f.listed_ids("?status=contacted").await, [gov]);
    assert_eq!(f.listed_ids("?segment=none").await, [contact]);
    assert_eq!(
        f.listed_ids("?segment=itad&status=contacted").await,
        Vec::<Uuid>::new()
    );
    // Empty values mean any; the latest submission comes first.
    assert_eq!(
        f.listed_ids("?segment=&status=&from=&to=").await,
        [contact, gov, itad]
    );

    // The row shape.
    let lead = &f.list("?segment=itad").await["leads"][0];
    let keys: Vec<&str> = lead
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    let mut want = [
        "id",
        "email",
        "name",
        "company",
        "role",
        "segment",
        "status",
        "created_at",
        "updated_at",
        "last_submitted_at",
        "submission_count",
        "unsubscribed_at",
    ];
    want.sort_unstable();
    let mut keys = keys;
    keys.sort_unstable();
    assert_eq!(keys, want);
    assert_eq!(lead["submission_count"], 1);
    assert_eq!(lead["unsubscribed_at"], Value::Null);
    let ts = lead["last_submitted_at"].as_str().unwrap();
    assert!(
        ts.len() == 20 && ts.ends_with('Z') && ts.as_bytes()[10] == b'T',
        "{ts}"
    );
}

#[tokio::test]
async fn the_date_filter_matches_any_submission_in_the_range() {
    let f = fixture().await;
    let today = utc_today(&f.hub).await;
    let past = shift(&f.hub, &today, -30).await;
    let (before, after) = (
        shift(&f.hub, &today, -31).await,
        shift(&f.hub, &today, -29).await,
    );
    let a = submit(&f.hub, &sub(), Some("itad")).await;
    let b = submit(&f.hub, &sub(), Some("itad")).await;
    // Pinned to `today`, not the clock, so midnight cannot move them.
    submitted_on(&f.hub, a, &past).await;
    submitted_on(&f.hub, b, &today).await;

    assert_eq!(
        f.listed_ids(&format!("?from={today}&to={today}")).await,
        [b]
    );
    assert_eq!(
        f.listed_ids(&format!("?from={before}&to={after}")).await,
        [a]
    );
    assert_eq!(f.listed_ids(&format!("?from={past}&to={past}")).await, [a]);
    assert_eq!(f.listed_ids(&format!("?from={after}")).await, [b]);
    assert_eq!(f.listed_ids(&format!("?to={after}")).await, [a]);

    // A new submission (today, or tomorrow if midnight has passed) brings A
    // into a range from today too.
    let email: String = sqlx::query_scalar("SELECT email FROM leads WHERE id = $1")
        .bind(a)
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    let again = Sub { email, ..sub() };
    assert_eq!(submit(&f.hub, &again, Some("itad")).await, a);
    let mut both = f.listed_ids(&format!("?from={today}")).await;
    both.sort();
    let mut want = vec![a, b];
    want.sort();
    assert_eq!(both, want);
    assert_eq!(f.listed_ids(&format!("?from={past}&to={past}")).await, [a]);

    // The range's edges: the first second of `from` and the last second of
    // `to` are in; the first second of the day after `to` is out.
    let (from, to, next) = (
        shift(&f.hub, &today, -10).await,
        shift(&f.hub, &today, -8).await,
        shift(&f.hub, &today, -7).await,
    );
    let first = submit(&f.hub, &sub(), Some("itad")).await;
    let last = submit(&f.hub, &sub(), Some("itad")).await;
    let outside = submit(&f.hub, &sub(), Some("itad")).await;
    submitted_at(&f.hub, first, &from, "00:00:00").await;
    submitted_at(&f.hub, last, &to, "23:59:59").await;
    submitted_at(&f.hub, outside, &next, "00:00:00").await;
    let mut edges = f.listed_ids(&format!("?from={from}&to={to}")).await;
    edges.sort();
    let mut want = vec![first, last];
    want.sort();
    assert_eq!(edges, want);
    assert_eq!(
        f.listed_ids(&format!("?from={next}&to={next}")).await,
        [outside]
    );
}

#[tokio::test]
async fn a_bad_filter_is_400_without_an_audit_row() {
    let f = fixture().await;
    submit(&f.hub, &sub(), Some("itad")).await;
    let rows = f.hub.count("audit_log").await;
    for q in [
        "?status=open",
        "?from=2026-13-01",
        "?from=2026-02-02&to=2026-02-01",
        "?segment=x",
        "?to=yesterday",
        "?status=new&status=closed",
    ] {
        assert_error(
            f.get(&format!("/api/admin/leads{q}")).await,
            StatusCode::BAD_REQUEST,
            BAD_FILTER,
        )
        .await;
    }
    for body in [
        json!({"status": "open"}),
        json!({"from": "2026-13-01"}),
        json!({"from": "2026-02-02", "to": "2026-02-01"}),
        json!({"segment": "x"}),
    ] {
        assert_error(
            f.post("/api/admin/leads/export", body).await,
            StatusCode::BAD_REQUEST,
            BAD_FILTER,
        )
        .await;
    }
    assert_eq!(f.hub.count("audit_log").await, rows);
}

#[tokio::test]
async fn the_detail_shows_submissions_with_their_consent_and_notes_with_their_author() {
    let f = fixture().await;
    let s = sub();
    let id = submit(&f.hub, &s, Some("itad")).await;
    let contact = Sub {
        email: s.email.clone(),
        message: "A second message".into(),
        ..sub()
    };
    assert_eq!(submit(&f.hub, &contact, None).await, id);
    let resp = f
        .post(
            &format!("/api/admin/leads/{id}/notes"),
            json!({"body": "Called"}),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    let body = ok_json(f.get(&format!("/api/admin/leads/{id}")).await).await;
    assert_eq!(body["lead"]["id"], json!(id));
    assert_eq!(body["lead"]["submission_count"], 2);
    let subs = body["submissions"].as_array().unwrap();
    assert_eq!(subs.len(), 2);
    // Newest first, each with the exact text consented to.
    assert_eq!(subs[0]["kind"], "contact");
    assert_eq!(subs[0]["message"], "A second message");
    assert_eq!(subs[0]["consent_text"], CONSENT_CONTACT);
    assert_eq!(subs[1]["kind"], "early-access");
    assert_eq!(subs[1]["message"], json!(s.message));
    assert_eq!(subs[1]["consent_text"], CONSENT_EARLY_ACCESS);
    assert!(subs[0]["consented_at"].as_str().unwrap() >= subs[1]["consented_at"].as_str().unwrap());
    let keys: Vec<&String> = subs[0].as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 5, "{keys:?}");

    let notes = body["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["body"], "Called");
    assert_eq!(
        notes[0]["author"],
        json!({"id": f.admin.id, "name": "Vera Vendor", "email": f.admin.email})
    );

    let rows = f.audits("admin.lead_viewed", &format!("lead:{id}")).await;
    assert_eq!(rows, [json!({})]);
}

#[tokio::test]
async fn a_status_change_is_audited_once_and_a_repeat_is_a_no_op() {
    let f = fixture().await;
    let id = submit(&f.hub, &sub(), Some("itad")).await;
    sqlx::query("UPDATE leads SET updated_at = updated_at - interval '1 hour' WHERE id = $1")
        .bind(id)
        .execute(&f.hub.db.pool)
        .await
        .unwrap();
    let before: String = f.list("").await["leads"][0]["updated_at"]
        .as_str()
        .unwrap()
        .into();
    let path = format!("/api/admin/leads/{id}/status");

    let body = ok_json(f.post(&path, json!({"status": "contacted"})).await).await;
    assert_eq!(body["lead"]["id"], json!(id));
    assert_eq!(body["lead"]["status"], "contacted");
    let bumped: String = body["lead"]["updated_at"].as_str().unwrap().into();
    assert!(bumped > before, "{bumped} <= {before}");

    assert!(f
        .audits("admin.lead_viewed", &format!("lead:{id}"))
        .await
        .is_empty());

    // The repeat changes nothing, but it returns the lead's data, so it is
    // audited as a view.
    let body = ok_json(f.post(&path, json!({"status": "contacted"})).await).await;
    assert_eq!(body["lead"]["status"], "contacted");
    assert_eq!(body["lead"]["updated_at"], json!(bumped));

    let rows = f
        .audits("admin.lead_status_changed", &format!("lead:{id}"))
        .await;
    assert_eq!(rows, [json!({"from": "new", "to": "contacted"})]);
    let rows = f.audits("admin.lead_viewed", &format!("lead:{id}")).await;
    assert_eq!(rows, [json!({})]);

    let audit_rows = f.hub.count("audit_log").await;
    for bad in [json!({"status": "won"}), json!({"status": ""}), json!({})] {
        assert_error(
            f.post(&path, bad).await,
            StatusCode::BAD_REQUEST,
            "status must be one of new, contacted, qualified, closed",
        )
        .await;
    }
    assert_eq!(f.hub.count("audit_log").await, audit_rows);
}

#[tokio::test]
async fn a_note_is_trimmed_and_audited_by_id_only() {
    let f = fixture().await;
    let id = submit(&f.hub, &sub(), Some("itad")).await;
    let path = format!("/api/admin/leads/{id}/notes");

    let resp = f
        .post(&path, json!({"body": "  called, follow up  "}))
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value = resp.json().await.unwrap();
    let note = &body["note"];
    assert_eq!(note["body"], "called, follow up");
    assert_eq!(note["author"]["id"], json!(f.admin.id));
    assert!(note["created_at"].as_str().unwrap().ends_with('Z'));
    let note_id = note["id"].as_str().unwrap();

    let rows = f
        .audits("admin.lead_note_added", &format!("lead:{id}"))
        .await;
    assert_eq!(rows, [json!({"note_id": note_id})]);

    let audit_rows = f.hub.count("audit_log").await;
    for (bad, message) in [
        (json!({"body": " \n "}), "note must not be blank"),
        (json!({}), "note must not be blank"),
        (
            json!({"body": "a\u{7}b"}),
            "note must not contain control characters",
        ),
        (
            json!({"body": "x".repeat(5001)}),
            "note must be at most 5000 characters",
        ),
    ] {
        assert_error(f.post(&path, bad).await, StatusCode::BAD_REQUEST, message).await;
    }
    assert_eq!(f.hub.count("audit_log").await, audit_rows);
    assert_eq!(f.hub.count("lead_notes").await, 1);
}

#[tokio::test]
async fn the_export_is_a_guarded_csv_of_the_matching_leads_and_is_audited() {
    let f = fixture().await;
    let today = utc_today(&f.hub).await;
    let tomorrow = shift(&f.hub, &today, 1).await;
    let formula = Sub {
        name: "=cmd()".into(),
        ..sub()
    };
    let comma = Sub {
        company: "a,\"b\"".into(),
        ..sub()
    };
    let a = submit(&f.hub, &formula, Some("itad")).await;
    let b = submit(&f.hub, &comma, Some("itad")).await;
    let other = submit(&f.hub, &sub(), Some("government")).await;

    // No CSRF token: refused by the CSRF check, nothing exported or audited.
    let rows = f.hub.count("audit_log").await;
    let resp = f
        .hub
        .http
        .post(f.hub.url("/api/admin/leads/export"))
        .header(reqwest::header::COOKIE, &f.admin.cookie)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(f.hub.count("audit_log").await, rows);

    let resp = f
        .post("/api/admin/leads/export", json!({"segment": "itad"}))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let h = resp.headers().clone();
    assert_eq!(h["content-type"], "text/csv; charset=utf-8");
    assert_eq!(h["cache-control"], "no-store");
    // Named for the export's UTC date: today, or tomorrow if the run has
    // just crossed midnight.
    let disposition = h["content-disposition"].to_str().unwrap();
    assert!(
        [&today, &tomorrow]
            .iter()
            .any(|d| disposition == format!("attachment; filename=\"sales-leads-{d}.csv\"")),
        "{disposition}"
    );
    let csv = csv_of(resp).await;
    let lines: Vec<&str> = csv.split("\r\n").collect();
    assert_eq!(lines.len(), 4, "{csv:?}"); // header, 2 rows, trailing ""
    assert_eq!(
        lines[0],
        "id,email,name,company,role,segment,status,created_at,updated_at,\
         last_submitted_at,submission_count,unsubscribed_at"
    );
    assert_eq!(lines[3], "");
    // Latest submission first, as in the list.
    assert!(lines[1].starts_with(&format!("{b},")), "{}", lines[1]);
    assert!(lines[2].starts_with(&format!("{a},")), "{}", lines[2]);
    assert!(lines[2].contains(",'=cmd(),"), "{}", lines[2]);
    assert!(lines[1].contains(",\"a,\"\"b\"\"\","), "{}", lines[1]);
    assert!(!csv.contains(&formula.message) && !csv.contains(CONSENT_EARLY_ACCESS));
    assert!(!csv.contains(&other.to_string()));

    let rows = f.audits("admin.leads_exported", "search:leads").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["filter"]["segment"], "itad");
    assert_eq!(uuids(&rows[0]["lead_ids"]), [b, a]);
}

#[tokio::test]
async fn the_export_has_no_row_cap() {
    let f = fixture().await;
    submit(&f.hub, &sub(), Some("itad")).await;
    // More than the list's 100, inserted directly (the form is throttled).
    sqlx::query(
        "WITH l AS (INSERT INTO leads (email, name, company) \
                    SELECT 'bulk' || g || '@example.com', 'Bulk', 'Co' \
                    FROM generate_series(1, 120) g RETURNING id) \
         INSERT INTO lead_submissions (lead_id, kind, consent_text, unsubscribe_token_hash) \
         SELECT id, 'contact', 'I agree.', sha256(id::text::bytea) FROM l",
    )
    .execute(&f.hub.db.pool)
    .await
    .unwrap();
    let body = f.list("").await;
    assert_eq!(body["leads"].as_array().unwrap().len(), 100);
    assert_eq!(body["truncated"], true);
    let csv = csv_of(f.post("/api/admin/leads/export", json!({})).await).await;
    assert_eq!(csv.split("\r\n").count(), 1 + 121 + 1);
}

#[tokio::test]
async fn erasing_a_lead_removes_everything_and_leaves_no_personal_data_in_the_audit_log() {
    let f = fixture().await;
    let s = sub();
    let id = submit(&f.hub, &s, Some("itad")).await;
    let again = Sub {
        email: s.email.clone(),
        name: s.name.clone(),
        company: s.company.clone(),
        message: format!("{} again", s.message),
    };
    submit(&f.hub, &again, None).await;
    let token = token_in(
        &mail_texts(&s.email, LEAD_EARLY_ACCESS_SUBJECT, 1).await[0],
        "unsubscribe",
    );
    let resp = f
        .hub
        .post("/api/leads/unsubscribe", json!({ "token": token }), None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = f
        .post(
            &format!("/api/admin/leads/{id}/notes"),
            json!({"body": "Note about the lead"}),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    ok_json(
        f.post(
            &format!("/api/admin/leads/{id}/status"),
            json!({"status": "qualified"}),
        )
        .await,
    )
    .await;
    ok_json(f.get(&format!("/api/admin/leads/{id}")).await).await;
    f.list("").await;
    // The confirmation emails charged the address's email-send budget; make
    // that row a lock with refused sends, whose purge would be audited with
    // its key (the address).
    assert_eq!(mail_throttle_rows(&f.hub, &s.email).await, 1);
    sqlx::query(
        "UPDATE auth_throttle SET refused = 3, locked_until = now() + interval '1 hour' \
         WHERE key = 'mail:to:' || lower($1)",
    )
    .bind(&s.email)
    .execute(&f.hub.db.pool)
    .await
    .unwrap();

    let body = ok_json(
        f.post(&format!("/api/admin/leads/{id}/erase"), json!({}))
            .await,
    )
    .await;
    assert_eq!(body, json!({}));
    for table in ["leads", "lead_submissions", "lead_notes"] {
        assert_eq!(f.hub.count(table).await, 0, "{table}");
    }
    // No hub table keeps the address: its email-send throttle row is gone,
    // so no later purge or charge can audit a lock summary naming it.
    assert_eq!(mail_throttle_rows(&f.hub, &s.email).await, 0);
    let purged = wipe_hub::throttle::purge_stale(&f.hub.db.pool)
        .await
        .unwrap();
    assert!(
        purged
            .iter()
            .all(|p| !p.key.contains(&s.email.to_lowercase())),
        "{purged:?}"
    );
    // A later send (another lead's confirmation) runs the hub's own purge.
    submit(&f.hub, &sub(), Some("itad")).await;
    for row in f.hub.audited("auth.mail_lock_summary").await {
        let text = format!("{} {}", row.1, row.2).to_lowercase();
        assert!(!text.contains(&s.email.to_lowercase()), "{text}");
    }
    let rows = f.audits("admin.lead_erased", &format!("lead:{id}")).await;
    assert_eq!(rows, [json!({"submissions": 2, "notes": 1})]);

    // No audit row anywhere holds the lead's personal data.
    let all: Vec<(String, String, String)> =
        sqlx::query_as("SELECT action, subject, detail::text FROM audit_log")
            .fetch_all(&f.hub.db.pool)
            .await
            .unwrap();
    let lead_rows = all
        .iter()
        .filter(|(a, _, _)| a.starts_with("lead.") || a.starts_with("admin.lead"))
        .count();
    assert!(lead_rows >= 8, "{all:?}");
    for (action, subject, detail) in &all {
        for secret in [
            s.email.as_str(),
            &s.email.to_uppercase(),
            &s.name,
            &s.company,
            &s.message,
            &again.message,
            "Note about the lead",
        ] {
            let row = format!("{subject} {detail}").to_lowercase();
            assert!(
                !row.contains(&secret.to_lowercase()),
                "{action} keeps {secret:?}: {subject} {detail}"
            );
        }
    }

    assert_error(
        f.get(&format!("/api/admin/leads/{id}")).await,
        StatusCode::NOT_FOUND,
        NO_LEAD,
    )
    .await;
}

#[tokio::test]
async fn an_unknown_lead_is_404_on_every_id_route_without_an_audit_row() {
    let f = fixture().await;
    submit(&f.hub, &sub(), Some("itad")).await;
    let rows = f.hub.count("audit_log").await;
    for id in [Uuid::new_v4().to_string(), "nope".to_string()] {
        assert_error(
            f.get(&format!("/api/admin/leads/{id}")).await,
            StatusCode::NOT_FOUND,
            NO_LEAD,
        )
        .await;
        for (route, body) in [
            ("status", json!({"status": "closed"})),
            ("notes", json!({"body": "hello"})),
            ("erase", json!({})),
        ] {
            assert_error(
                f.post(&format!("/api/admin/leads/{id}/{route}"), body)
                    .await,
                StatusCode::NOT_FOUND,
                NO_LEAD,
            )
            .await;
        }
    }
    assert_eq!(f.hub.count("audit_log").await, rows);
    assert_eq!(f.hub.count("lead_notes").await, 0);
}

#[tokio::test]
async fn an_unsubscribed_lead_shows_its_date_in_the_list_detail_and_csv() {
    let f = fixture().await;
    let s = sub();
    let id = submit(&f.hub, &s, Some("itad")).await;
    assert_eq!(f.list("").await["leads"][0]["unsubscribed_at"], Value::Null);
    let token = token_in(
        &mail_texts(&s.email, LEAD_EARLY_ACCESS_SUBJECT, 1).await[0],
        "unsubscribe",
    );
    let resp = f
        .hub
        .post("/api/leads/unsubscribe", json!({ "token": token }), None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let listed = f.list("").await["leads"][0]["unsubscribed_at"].clone();
    let at = listed.as_str().expect("unsubscribed_at set").to_string();
    assert!(at.ends_with('Z'), "{at}");
    let detail = ok_json(f.get(&format!("/api/admin/leads/{id}")).await).await;
    assert_eq!(detail["lead"]["unsubscribed_at"], json!(at));
    let csv = csv_of(f.post("/api/admin/leads/export", json!({})).await).await;
    let row = csv.split("\r\n").nth(1).unwrap();
    assert!(row.ends_with(&format!(",{at}")), "{row}");
}

#[tokio::test]
async fn customers_and_anonymous_callers_are_refused_on_every_lead_route_without_an_audit_row() {
    let hub = spawn_hub().await;
    let id = submit(&hub, &sub(), Some("itad")).await;
    let owner = hub.verified_owner("Acme").await;
    let member = hub.add(&owner, "member").await;
    let rows = hub.count("audit_log").await;
    let gets = [
        "/api/admin/leads".to_string(),
        format!("/api/admin/leads/{id}"),
    ];
    let posts = [
        ("/api/admin/leads/export".to_string(), json!({})),
        (
            format!("/api/admin/leads/{id}/status"),
            json!({"status": "closed"}),
        ),
        (
            format!("/api/admin/leads/{id}/notes"),
            json!({"body": "hello"}),
        ),
        (format!("/api/admin/leads/{id}/erase"), json!({})),
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
        // Refused before the query string is read: a bad filter is no 400.
        assert_error(
            hub.get("/api/admin/leads?status=x", cookie).await,
            code,
            message,
        )
        .await;
        for (path, body) in &posts {
            assert_error(hub.post(path, body.clone(), cookie).await, code, message).await;
            // Refused before the body is read.
            assert_error(hub.post_empty(path, cookie).await, code, message).await;
        }
    }
    assert_eq!(hub.count("audit_log").await, rows);
    assert_eq!(hub.count("leads").await, 1);
    let status: String = sqlx::query_scalar("SELECT status FROM leads")
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    assert_eq!(status, "new");
}

#[tokio::test]
async fn the_same_address_after_erasure_is_a_new_lead() {
    let f = fixture().await;
    let s = sub();
    let id = submit(&f.hub, &s, Some("itad")).await;
    ok_json(
        f.post(&format!("/api/admin/leads/{id}/erase"), json!({}))
            .await,
    )
    .await;
    let again = submit(&f.hub, &s, Some("itad")).await;
    assert_ne!(again, id);
    let submitted = f.hub.audited("lead.submitted").await;
    let last = submitted.last().unwrap();
    assert_eq!(last.1, format!("lead:{again}"));
    assert_eq!(last.2["created"], true);
    assert_eq!(last.2["suppressed"], false);
    assert_eq!(f.listed_ids("").await, [again]);
}

/// One mutation started while another transaction holds the lead `FOR
/// UPDATE`, as an erasure in progress does; the holder then deletes the lead
/// and commits. The mutation must be a 404 with no new audit row.
async fn race_an_erasure(f: &Fixture, route: &str, body: Value) {
    let id = submit(&f.hub, &sub(), Some("itad")).await;
    let rows = f.hub.count("audit_log").await;

    let mut holder = f.hub.db.pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM leads WHERE id = $1 FOR UPDATE")
        .bind(id)
        .execute(&mut *holder)
        .await
        .unwrap();

    let req = f
        .hub
        .post_req(
            &format!("/api/admin/leads/{id}/{route}"),
            Some(&f.admin.cookie),
        )
        .json(&body);
    let mutation = tokio::spawn(async move { req.send().await.unwrap() });

    // Wait until the mutation's lock is queued behind the holder.
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
            "{route} never waited on the lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    sqlx::query("DELETE FROM leads WHERE id = $1")
        .bind(id)
        .execute(&mut *holder)
        .await
        .unwrap();
    holder.commit().await.unwrap();

    assert_error(mutation.await.unwrap(), StatusCode::NOT_FOUND, NO_LEAD).await;
    assert_eq!(f.hub.count("audit_log").await, rows, "{route}");
}

#[tokio::test]
async fn a_mutation_racing_an_erasure_is_a_404_not_a_500() {
    let f = fixture().await;
    for (route, body) in [
        ("notes", json!({"body": "racing"})),
        ("status", json!({"status": "closed"})),
        ("erase", json!({})),
    ] {
        race_an_erasure(&f, route, body).await;
    }
    assert_eq!(f.hub.count("lead_notes").await, 0);
    assert_eq!(f.hub.count("leads").await, 0);
}

#[tokio::test]
async fn erasing_a_mixed_case_address_removes_its_lower_cased_throttle_row() {
    let f = fixture().await;
    let s = Sub {
        email: unique_address().to_uppercase(),
        ..sub()
    };
    let id = submit(&f.hub, &s, Some("itad")).await;
    let lower = s.email.to_lowercase();
    assert_eq!(mail_throttle_rows(&f.hub, &lower).await, 1);
    // Stored as typed, so the erasure itself must lower-case the key.
    let stored: String = sqlx::query_scalar("SELECT email FROM leads WHERE id = $1")
        .bind(id)
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(stored, s.email);
    ok_json(
        f.post(&format!("/api/admin/leads/{id}/erase"), json!({}))
            .await,
    )
    .await;
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_throttle WHERE key = $1")
        .bind(format!("mail:to:{lower}"))
        .fetch_one(&f.hub.db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn notes_are_newest_first_and_keep_no_author_once_the_user_is_removed() {
    let f = fixture().await;
    let id = submit(&f.hub, &sub(), Some("itad")).await;
    let path = format!("/api/admin/leads/{id}/notes");
    let mut note_ids = Vec::new();
    for body in ["first", "second"] {
        let resp = f.post(&path, json!({ "body": body })).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let v: Value = resp.json().await.unwrap();
        note_ids.push(v["note"]["id"].clone());
    }

    // A second admin reads the lead after the first admin's account is gone.
    let other = bootstrap_admin(&f.hub).await;
    sqlx::query("DELETE FROM users WHERE id = $1::uuid")
        .bind(&f.admin.id)
        .execute(&f.hub.db.pool)
        .await
        .unwrap();
    let body = ok_json(
        f.hub
            .get(&format!("/api/admin/leads/{id}"), Some(&other.cookie))
            .await,
    )
    .await;
    let notes = body["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0]["body"], "second");
    assert_eq!(notes[0]["id"], note_ids[1]);
    assert_eq!(notes[1]["body"], "first");
    assert_eq!(notes[1]["id"], note_ids[0]);
    for n in notes {
        assert_eq!(n["author"], Value::Null, "{n}");
    }
}

#[tokio::test]
async fn an_unknown_filter_key_or_a_bad_export_body_is_400_without_an_audit_row() {
    let f = fixture().await;
    submit(&f.hub, &sub(), Some("itad")).await;
    let rows = f.hub.count("audit_log").await;
    for q in ["?Segment=itad", "?q=ada", "?segment=itad&limit=5"] {
        assert_error(
            f.get(&format!("/api/admin/leads{q}")).await,
            StatusCode::BAD_REQUEST,
            BAD_FILTER,
        )
        .await;
    }
    for body in [
        json!({"status": 1}),
        json!({"Segment": "itad"}),
        json!({"from": ["2026-01-01"]}),
        json!(["itad", null, null, null]),
        json!("itad"),
        json!(null),
    ] {
        assert_error(
            f.post("/api/admin/leads/export", body.clone()).await,
            StatusCode::BAD_REQUEST,
            BAD_FILTER,
        )
        .await;
    }
    // Not JSON at all, or no body.
    let resp = f
        .hub
        .post_req("/api/admin/leads/export", Some(&f.admin.cookie))
        .header("content-type", "application/json")
        .body("{segment")
        .send()
        .await
        .unwrap();
    assert_error(resp, StatusCode::BAD_REQUEST, BAD_FILTER).await;
    assert_error(
        f.hub
            .post_empty("/api/admin/leads/export", Some(&f.admin.cookie))
            .await,
        StatusCode::BAD_REQUEST,
        BAD_FILTER,
    )
    .await;
    assert_eq!(f.hub.count("audit_log").await, rows);
}
