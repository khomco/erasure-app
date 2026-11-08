//! The vendor admin area (portal-accounts story 5): `wipe-hub admin create`
//! run as the real binary, the vendor-admin check, and the audited
//! `/api/admin/*` searches and views, over real HTTP against a migrated test
//! database and Mailpit. Needs `docker compose up -d`; fails, never skips,
//! when the compose services are down.

mod support;

use std::process::{Command, Output};

use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{
    assert_error, cookie_pair, mail_texts, spawn_hub, token_in, Hub, Who, BASE, NEW_PASSWORD,
};
use support::{unique_address, Mailpit};
use uuid::Uuid;
use wipe_hub::mail::PASSWORD_RESET_SUBJECT;

const ADMIN_ONLY: &str = "only vendor admins can use the admin area";
const NOT_SIGNED_IN: &str = "not signed in";
const NO_ORGANIZATION: &str = "no such organization";
const BAD_QUERY: &str = "query must be at most 200 characters without control characters";

/// Every admin route, with a valid organization id where one is needed.
fn admin_paths(org: &str) -> Vec<String> {
    vec![
        "/api/admin/organizations".into(),
        "/api/admin/users".into(),
        format!("/api/admin/organizations/{org}"),
    ]
}

// ---------------------------------------------------------------------------
// The CLI

/// `wipe-hub admin create` against `db_url`, with every `WIPE_HUB_*` var
/// removed so a developer's environment cannot change the outcome.
fn admin_create(db_url: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wipe-hub"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("WIPE_HUB_") {
            cmd.env_remove(&key);
        }
    }
    cmd.env("RUST_LOG", "warn")
        .args(["admin", "create", "--database-url", db_url])
        .args(["--public-base-url", BASE])
        .args(args);
    cmd
}

fn run(db_url: &str, args: &[&str]) -> Output {
    admin_create(db_url, args).output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
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

fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// The emailed set-password token for `email`.
async fn reset_token(email: &str) -> String {
    let texts = mail_texts(email, PASSWORD_RESET_SUBJECT, 1).await;
    token_in(&texts[0], "reset-password")
}

/// Create a vendor admin through the CLI, follow the emailed link to set a
/// password, and sign in.
async fn bootstrap_admin(hub: &Hub) -> Who {
    let email = unique_address();
    assert_ok(&run(
        &hub.db.url,
        &["--email", &email, "--name", "Vera Vendor"],
    ));
    let token = reset_token(&email).await;
    let resp = hub
        .post(
            "/api/auth/password-reset/complete",
            json!({"token": token, "password": NEW_PASSWORD}),
            None,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = hub.sign_in(&email, NEW_PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = cookie_pair(&resp);
    let (_, me) = hub.me(&cookie).await;
    Who {
        id: me["user"]["id"].as_str().unwrap().to_string(),
        email,
        cookie,
    }
}

async fn vendor_orgs(hub: &Hub) -> Vec<(Uuid, String)> {
    sqlx::query_as("SELECT id, name FROM organizations WHERE is_vendor")
        .fetch_all(&hub.db.pool)
        .await
        .unwrap()
}

async fn org_of(hub: &Hub, user_id: &str) -> Uuid {
    sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1::uuid")
        .bind(user_id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap()
}

async fn admin_get(hub: &Hub, path: &str, q: Option<&str>, cookie: Option<&str>) -> Response {
    let mut req = hub.http.get(hub.url(path));
    if let Some(q) = q {
        req = req.query(&[("q", q)]);
    }
    if let Some(c) = cookie {
        req = req.header(reqwest::header::COOKIE, c);
    }
    req.send().await.unwrap()
}

async fn ok_json(resp: Response) -> Value {
    let code = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(code, StatusCode::OK, "{body}");
    body
}

fn ids(list: &Value) -> Vec<Value> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].clone())
        .collect()
}

#[tokio::test]
async fn bootstrap_creates_the_vendor_organization_and_emails_a_link() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let out = run(&hub.db.url, &["--email", &email]);
    assert_ok(&out);

    let (id, role, name, verified, org): (Uuid, String, String, bool, Uuid) = sqlx::query_as(
        "SELECT id, role, name, email_verified_at IS NOT NULL, organization_id FROM users \
         WHERE email = $1",
    )
    .bind(&email)
    .fetch_one(&hub.db.pool)
    .await
    .unwrap();
    assert_eq!(role, "vendor_admin");
    assert!(!verified);
    // The name defaults to the email's local part.
    assert_eq!(name, email.split('@').next().unwrap());
    assert_eq!(vendor_orgs(&hub).await, [(org, "Vendor staff".to_string())]);
    assert!(
        stdout(&out).contains(&format!("created vendor admin {email} ({id})")),
        "{}",
        stdout(&out)
    );

    let mails = Mailpit::new().messages_to(&email).await;
    let texts = mail_texts(&email, PASSWORD_RESET_SUBJECT, 1).await;
    assert_eq!(mails.len(), 1);
    let token = token_in(&texts[0], "reset-password");
    // The raw token is never printed or audited.
    assert!(!stdout(&out).contains(&token) && !stderr(&out).contains(&token));
    let all: Vec<String> = sqlx::query_scalar("SELECT detail::text FROM audit_log")
        .fetch_all(&hub.db.pool)
        .await
        .unwrap();
    assert!(all.iter().all(|d| !d.contains(&token)));

    let rows = hub.audited("admin.created").await;
    assert_eq!(rows.len(), 1);
    let (actor, subject, detail) = &rows[0];
    assert_eq!(actor, "cli:admin-create");
    assert_eq!(subject, &format!("user:{id}"));
    assert_eq!(detail["email"], json!(email));
    assert_eq!(detail["organization_id"], json!(org));
    assert_eq!(detail["vendor_organization_created"], json!(true));
    assert!(
        detail.as_object().unwrap().contains_key("os_user"),
        "{detail}"
    );
}

#[tokio::test]
async fn the_admin_sets_a_password_signs_in_and_reaches_the_admin_area() {
    let hub = spawn_hub().await;
    let admin = bootstrap_admin(&hub).await;
    let (code, me) = hub.me(&admin.cookie).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(me["user"]["role"], "vendor_admin");
    assert_eq!(me["user"]["email_verified"], true);
    assert_eq!(me["organization"]["name"], "Vendor staff");
    let body =
        ok_json(admin_get(&hub, "/api/admin/organizations", None, Some(&admin.cookie)).await).await;
    assert_eq!(body["organizations"], json!([]));
    assert_eq!(body["truncated"], false);
}

#[tokio::test]
async fn a_second_admin_joins_the_same_vendor_organization() {
    let hub = spawn_hub().await;
    assert_ok(&run(&hub.db.url, &["--email", &unique_address()]));
    let first = vendor_orgs(&hub).await;
    assert_ok(&run(&hub.db.url, &["--email", &unique_address()]));
    assert_eq!(vendor_orgs(&hub).await, first);
    let rows = hub.audited("admin.created").await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].2["vendor_organization_created"], json!(false));
    assert_eq!(rows[1].2["organization_id"], json!(first[0].0));
}

#[tokio::test]
async fn concurrent_bootstraps_make_one_vendor_organization() {
    let hub = spawn_hub().await;
    let children: Vec<_> = (0..2)
        .map(|_| {
            admin_create(&hub.db.url, &["--email", &unique_address()])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        assert_ok(&child.wait_with_output().unwrap());
    }
    assert_eq!(vendor_orgs(&hub).await.len(), 1);
    let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'vendor_admin'")
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    assert_eq!(admins, 2);
    let created: Vec<bool> = hub
        .audited("admin.created")
        .await
        .iter()
        .map(|r| r.2["vendor_organization_created"].as_bool().unwrap())
        .collect();
    assert_eq!(created.iter().filter(|c| **c).count(), 1, "{created:?}");
}

#[tokio::test]
async fn an_existing_admin_is_refused_without_a_new_row_or_mail() {
    let hub = spawn_hub().await;
    let email = unique_address();
    assert_ok(&run(&hub.db.url, &["--email", &email]));
    mail_texts(&email, PASSWORD_RESET_SUBJECT, 1).await;
    let before = hub.counts().await;
    let out = run(&hub.db.url, &["--email", &email.to_uppercase()]);
    assert_failed_with(
        &out,
        &format!(
            "{} is already a vendor admin; use Forgot password on the portal",
            email.to_uppercase()
        ),
    );
    assert_eq!(hub.counts().await, before);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(Mailpit::new().messages_to(&email).await.len(), 1);
}

#[tokio::test]
async fn a_customers_address_is_refused_and_left_alone() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Acme").await;
    let org = org_of(&hub, &owner.id).await;
    let before = hub.counts().await;
    let out = run(&hub.db.url, &["--email", &owner.email]);
    assert_failed_with(&out, "an account with this email already exists");
    assert_eq!(hub.counts().await, before);
    assert!(vendor_orgs(&hub).await.is_empty());
    let (role, now_org): (String, Uuid) =
        sqlx::query_as("SELECT role, organization_id FROM users WHERE id = $1::uuid")
            .bind(&owner.id)
            .fetch_one(&hub.db.pool)
            .await
            .unwrap();
    assert_eq!((role.as_str(), now_org), ("owner", org));
}

#[tokio::test]
async fn bad_input_writes_nothing() {
    let hub = spawn_hub().await;
    let before = hub.counts().await;
    let out = run(&hub.db.url, &["--email", "nope"]);
    assert_failed_with(&out, "email is not a valid email address");
    let email = unique_address();
    let out = run(
        &hub.db.url,
        &["--email", &email, "--name", "Vera\nBcc: x@evil.test"],
    );
    assert_failed_with(&out, "name must not contain control characters");
    let out = run(
        &hub.db.url,
        &["--email", &email, "--mail-from", "not a mailbox"],
    );
    assert_failed_with(&out, "--mail-from");
    assert_eq!(hub.counts().await, before);
    assert!(vendor_orgs(&hub).await.is_empty());
}

#[test]
fn unreachable_postgres_exits_non_zero() {
    let url = format!(
        "postgres://wipehub:wipehub@127.0.0.1:{}/wipehub",
        closed_port()
    );
    let out = run(&url, &["--email", &unique_address()]);
    assert_failed_with(&out, "docker compose up -d");
}

#[tokio::test]
async fn an_smtp_outage_still_creates_the_admin_and_says_use_forgot_password() {
    let hub = spawn_hub().await;
    let email = unique_address();
    let port = closed_port().to_string();
    let out = run(&hub.db.url, &["--email", &email, "--smtp-port", &port]);
    assert_ok(&out);
    assert!(
        stdout(&out).contains("created vendor admin"),
        "{}",
        stdout(&out)
    );
    assert!(stderr(&out).contains("Forgot password"), "{}", stderr(&out));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = $1")
        .bind(&email)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

// ---------------------------------------------------------------------------
// Who gets in

#[tokio::test]
async fn customers_and_anonymous_callers_are_refused_without_an_audit_row() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Acme").await;
    let member = hub.add(&owner, "member").await;
    let org = org_of(&hub, &owner.id).await.to_string();
    let audit_rows = hub.count("audit_log").await;
    for path in admin_paths(&org) {
        for who in [&owner, &member] {
            assert_error(
                admin_get(&hub, &path, Some("a"), Some(&who.cookie)).await,
                StatusCode::FORBIDDEN,
                ADMIN_ONLY,
            )
            .await;
        }
        // Refused before the query is read: a bad one is still 403.
        assert_error(
            admin_get(&hub, &path, Some("bad\nquery"), Some(&owner.cookie)).await,
            StatusCode::FORBIDDEN,
            ADMIN_ONLY,
        )
        .await;
        assert_error(
            admin_get(&hub, &path, None, None).await,
            StatusCode::UNAUTHORIZED,
            NOT_SIGNED_IN,
        )
        .await;
    }
    assert_eq!(hub.count("audit_log").await, audit_rows);
}

#[tokio::test]
async fn an_unverified_vendor_admin_is_refused() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Acme").await;
    let org = org_of(&hub, &owner.id).await.to_string();
    let admin = bootstrap_admin(&hub).await;
    sqlx::query("UPDATE users SET email_verified_at = NULL WHERE id = $1::uuid")
        .bind(&admin.id)
        .execute(&hub.db.pool)
        .await
        .unwrap();
    let audit_rows = hub.count("audit_log").await;
    for path in admin_paths(&org) {
        assert_error(
            admin_get(&hub, &path, None, Some(&admin.cookie)).await,
            StatusCode::FORBIDDEN,
            ADMIN_ONLY,
        )
        .await;
    }
    assert_eq!(hub.count("audit_log").await, audit_rows);
}

#[tokio::test]
async fn a_vendor_admin_role_in_a_customer_organization_reaches_nothing() {
    let hub = spawn_hub().await;
    let owner = hub.verified_owner("Acme").await;
    let org = org_of(&hub, &owner.id).await;
    let hash = wipe_hub::accounts::hash_password(NEW_PASSWORD.into())
        .await
        .unwrap();
    let email = unique_address();
    sqlx::query(
        "INSERT INTO users (organization_id, email, name, password_hash, role, email_verified_at) \
         VALUES ($1, $2, 'Misplaced', $3, 'vendor_admin', now())",
    )
    .bind(org)
    .bind(&email)
    .bind(&hash)
    .execute(&hub.db.pool)
    .await
    .unwrap();
    let resp = hub.sign_in(&email, NEW_PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = cookie_pair(&resp);
    let audit_rows = hub.count("audit_log").await;
    for path in admin_paths(&org.to_string()) {
        assert_error(
            admin_get(&hub, &path, None, Some(&cookie)).await,
            StatusCode::FORBIDDEN,
            ADMIN_ONLY,
        )
        .await;
    }
    assert_eq!(hub.count("audit_log").await, audit_rows);

    // Nor is such a user ever listed to a real admin.
    let admin = bootstrap_admin(&hub).await;
    let body =
        ok_json(admin_get(&hub, "/api/admin/users", Some(""), Some(&admin.cookie)).await).await;
    let roles: Vec<&str> = body["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["owner"]);
    let body = ok_json(
        admin_get(
            &hub,
            &format!("/api/admin/organizations/{org}"),
            None,
            Some(&admin.cookie),
        )
        .await,
    )
    .await;
    assert_eq!(body["users"].as_array().unwrap().len(), 1);
    let body = ok_json(
        admin_get(
            &hub,
            "/api/admin/organizations",
            Some("acme"),
            Some(&admin.cookie),
        )
        .await,
    )
    .await;
    assert_eq!(body["organizations"][0]["user_count"], 1);
}

// ---------------------------------------------------------------------------
// Searches and views

/// `Acme` (owner + member) and `Beta` (owner + member), plus a signed-in admin.
struct Fixture {
    hub: Hub,
    admin: Who,
    acme: Uuid,
    beta: Uuid,
    acme_owner: Who,
    beta_member: Who,
}

async fn fixture() -> Fixture {
    let hub = spawn_hub().await;
    let acme_owner = hub.verified_owner("Acme").await;
    hub.add(&acme_owner, "member").await;
    let beta_owner = hub.verified_owner("Beta").await;
    let beta_member = hub.add(&beta_owner, "member").await;
    let acme = org_of(&hub, &acme_owner.id).await;
    let beta = org_of(&hub, &beta_owner.id).await;
    let admin = bootstrap_admin(&hub).await;
    Fixture {
        hub,
        admin,
        acme,
        beta,
        acme_owner,
        beta_member,
    }
}

impl Fixture {
    async fn get(&self, path: &str, q: Option<&str>) -> Response {
        admin_get(&self.hub, path, q, Some(&self.admin.cookie)).await
    }

    /// The single audit row for `action`, checking actor and subject.
    async fn only_audit(&self, action: &str, subject: &str) -> Value {
        let rows = self.hub.audited(action).await;
        assert_eq!(rows.len(), 1, "{action}: {rows:?}");
        assert_eq!(rows[0].0, format!("user:{}", self.admin.id));
        assert_eq!(rows[0].1, subject);
        rows[0].2.clone()
    }
}

#[tokio::test]
async fn searching_organizations_by_name_is_audited_with_the_returned_ids() {
    let f = fixture().await;
    let body = ok_json(f.get("/api/admin/organizations", Some(" acm ")).await).await;
    let orgs = body["organizations"].as_array().unwrap();
    assert_eq!(orgs.len(), 1, "{body}");
    assert_eq!(orgs[0]["id"], json!(f.acme));
    assert_eq!(orgs[0]["name"], "Acme");
    assert_eq!(orgs[0]["user_count"], 2);
    assert!(orgs[0]["created_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(body["truncated"], false);
    let detail = f
        .only_audit("admin.organizations_searched", "search:organizations")
        .await;
    assert_eq!(
        detail,
        json!({"query": "acm", "organization_ids": [f.acme]})
    );
    assert_eq!(
        detail["organization_ids"].as_array().unwrap(),
        &ids(&body["organizations"])
    );
}

#[tokio::test]
async fn organizations_are_found_by_a_members_email_and_listed_in_name_order() {
    let f = fixture().await;
    let fragment = &f.beta_member.email[2..14];
    let body = ok_json(f.get("/api/admin/organizations", Some(fragment)).await).await;
    assert_eq!(ids(&body["organizations"]), [json!(f.beta)]);
    // Upper case finds the same.
    let body = ok_json(
        f.get("/api/admin/organizations", Some(&fragment.to_uppercase()))
            .await,
    )
    .await;
    assert_eq!(ids(&body["organizations"]), [json!(f.beta)]);
    // Empty matches every customer organization, never the vendor one.
    let body = ok_json(f.get("/api/admin/organizations", None).await).await;
    assert_eq!(ids(&body["organizations"]), [json!(f.acme), json!(f.beta)]);
}

#[tokio::test]
async fn both_searches_match_a_persons_name() {
    let f = fixture().await;
    // Both members are "Grace Hopper": a fragment of that name finds both
    // organizations (neither organization name nor any email contains it).
    let body = ok_json(f.get("/api/admin/organizations", Some("HOPP")).await).await;
    assert_eq!(ids(&body["organizations"]), [json!(f.acme), json!(f.beta)]);
    let rows = f.hub.audited("admin.organizations_searched").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].2["organization_ids"].as_array().unwrap(),
        &ids(&body["organizations"])
    );

    // A name only Acme's owner has finds exactly that user.
    sqlx::query("UPDATE users SET name = 'Quentin Zebulon' WHERE id = $1::uuid")
        .bind(&f.acme_owner.id)
        .execute(&f.hub.db.pool)
        .await
        .unwrap();
    let body = ok_json(f.get("/api/admin/users", Some("zebul")).await).await;
    assert_eq!(ids(&body["users"]), [json!(f.acme_owner.id)]);
    assert_eq!(body["users"][0]["name"], "Quentin Zebulon");
    let rows = f.hub.audited("admin.users_searched").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].2["user_ids"].as_array().unwrap(),
        &ids(&body["users"])
    );
}

#[tokio::test]
async fn like_wildcards_match_literally() {
    let f = fixture().await;
    for q in ["%", "_", "\\"] {
        let body = ok_json(f.get("/api/admin/organizations", Some(q)).await).await;
        assert_eq!(body["organizations"], json!([]), "{q}");
        let body = ok_json(f.get("/api/admin/users", Some(q)).await).await;
        assert_eq!(body["users"], json!([]), "{q}");
    }
    sqlx::query("UPDATE organizations SET name = '100% Recycling' WHERE id = $1")
        .bind(f.acme)
        .execute(&f.hub.db.pool)
        .await
        .unwrap();
    let body = ok_json(f.get("/api/admin/organizations", Some("0%")).await).await;
    assert_eq!(ids(&body["organizations"]), [json!(f.acme)]);
}

#[tokio::test]
async fn a_bad_query_is_400_without_an_audit_row() {
    let f = fixture().await;
    let audit_rows = f.hub.count("audit_log").await;
    let long = "a".repeat(201);
    for path in ["/api/admin/organizations", "/api/admin/users"] {
        for q in [long.as_str(), "ac\nme"] {
            assert_error(
                f.get(path, Some(q)).await,
                StatusCode::BAD_REQUEST,
                BAD_QUERY,
            )
            .await;
        }
    }
    assert_eq!(f.hub.count("audit_log").await, audit_rows);
    // 200 characters, with edge spaces trimmed, is fine.
    let ok = format!("  {}  ", "a".repeat(200));
    ok_json(f.get("/api/admin/users", Some(&ok)).await).await;
}

#[tokio::test]
async fn searching_users_lists_customers_with_their_organization_only() {
    let f = fixture().await;
    let fragment = &f.acme_owner.email[2..14];
    let body = ok_json(f.get("/api/admin/users", Some(fragment)).await).await;
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 1, "{body}");
    let u = &users[0];
    assert_eq!(u["id"], json!(f.acme_owner.id));
    assert_eq!(u["email"], json!(f.acme_owner.email));
    assert_eq!(u["role"], "owner");
    assert_eq!(u["email_verified"], true);
    assert_eq!(u["organization"], json!({"id": f.acme, "name": "Acme"}));
    let detail = f.only_audit("admin.users_searched", "search:users").await;
    assert_eq!(
        detail,
        json!({"query": fragment, "user_ids": [f.acme_owner.id]})
    );

    // Everyone: four customers, no vendor admin, ordered by email.
    let body = ok_json(f.get("/api/admin/users", Some("")).await).await;
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 4, "{body}");
    assert!(users.iter().all(|u| u["role"] != "vendor_admin"));
    assert!(users.iter().all(|u| u["email"] != json!(f.admin.email)));
    let emails: Vec<String> = users
        .iter()
        .map(|u| u["email"].as_str().unwrap().to_lowercase())
        .collect();
    let mut sorted = emails.clone();
    sorted.sort();
    assert_eq!(emails, sorted);
    let rows = f.hub.audited("admin.users_searched").await;
    assert_eq!(
        rows[1].2["user_ids"].as_array().unwrap(),
        &ids(&body["users"])
    );
}

#[tokio::test]
async fn more_than_a_hundred_matches_are_truncated() {
    let f = fixture().await;
    sqlx::query(
        "INSERT INTO organizations (name) \
         SELECT 'Bulk ' || lpad(n::text, 3, '0') FROM generate_series(1, 101) AS n",
    )
    .execute(&f.hub.db.pool)
    .await
    .unwrap();
    let body = ok_json(f.get("/api/admin/organizations", Some("bulk")).await).await;
    let orgs = body["organizations"].as_array().unwrap();
    assert_eq!(orgs.len(), 100);
    assert_eq!(body["truncated"], true);
    assert_eq!(orgs[0]["name"], "Bulk 001");
    assert_eq!(orgs[99]["name"], "Bulk 100");
    let detail = f
        .only_audit("admin.organizations_searched", "search:organizations")
        .await;
    assert_eq!(detail["organization_ids"].as_array().unwrap().len(), 100);
}

#[tokio::test]
async fn viewing_an_organization_shows_its_users_and_is_audited() {
    let f = fixture().await;
    let body = ok_json(
        f.get(&format!("/api/admin/organizations/{}", f.beta), None)
            .await,
    )
    .await;
    assert_eq!(body["organization"]["id"], json!(f.beta));
    assert_eq!(body["organization"]["name"], "Beta");
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 2);
    // Ordered by name: "Ada Lovelace" (owner) before "Grace Hopper" (member).
    assert_eq!(users[0]["role"], "owner");
    assert_eq!(users[1]["id"], json!(f.beta_member.id));
    assert_eq!(users[1]["role"], "member");
    for u in users {
        let keys: Vec<&String> = u.as_object().unwrap().keys().collect();
        assert_eq!(
            keys.len(),
            6,
            "id, email, name, role, email_verified, created_at: {u}"
        );
    }
    let detail = f
        .only_audit(
            "admin.organization_viewed",
            &format!("organization:{}", f.beta),
        )
        .await;
    assert_eq!(detail, json!({"user_ids": ids(&body["users"])}));
}

#[tokio::test]
async fn unknown_non_uuid_and_vendor_organizations_are_404_without_an_audit_row() {
    let f = fixture().await;
    let vendor = vendor_orgs(&f.hub).await[0].0;
    let audit_rows = f.hub.count("audit_log").await;
    for id in [
        Uuid::new_v4().to_string(),
        "not-a-uuid".to_string(),
        vendor.to_string(),
    ] {
        assert_error(
            f.get(&format!("/api/admin/organizations/{id}"), None).await,
            StatusCode::NOT_FOUND,
            NO_ORGANIZATION,
        )
        .await;
    }
    assert_eq!(f.hub.count("audit_log").await, audit_rows);
}

#[tokio::test]
async fn admin_routes_are_get_only() {
    let f = fixture().await;
    let resp = f
        .hub
        .post("/api/admin/organizations", json!({}), Some(&f.admin.cookie))
        .await;
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn two_signups_and_a_bootstrapped_admin_viewing_both_are_audited_in_order() {
    let hub = spawn_hub().await;
    let a = hub.verified_owner("Acme").await;
    let b = hub.verified_owner("Beta").await;
    let admin = bootstrap_admin(&hub).await;
    for owner in [&a, &b] {
        let org = org_of(&hub, &owner.id).await;
        let body = ok_json(
            admin_get(
                &hub,
                &format!("/api/admin/organizations/{org}"),
                None,
                Some(&admin.cookie),
            )
            .await,
        )
        .await;
        assert_eq!(body["users"][0]["id"], json!(owner.id));
    }
    let wanted = [
        "auth.sign_up",
        "admin.created",
        "auth.sign_in",
        "admin.organization_viewed",
    ];
    let seen: Vec<String> = hub
        .actions()
        .await
        .into_iter()
        .filter(|a| wanted.contains(&a.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            "auth.sign_up",
            "auth.sign_up",
            "admin.created",
            "auth.sign_in",
            "admin.organization_viewed",
            "admin.organization_viewed",
        ]
    );
    let sign_in = hub.audited("auth.sign_in").await;
    assert_eq!(sign_in[0].0, format!("user:{}", admin.id));
}
