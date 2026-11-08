//! Tenant isolation (ADR-0007 §3, portal-accounts story 5): one organization
//! can never read or change another's records through the API.
//!
//! - The sweep calls every route that needs a customer session (and every
//!   admin route) as organization A's owner and as A's member, with B's ids
//!   in the path and B's addresses in the bodies. No response body may carry
//!   any of B's identifiers that the request did not carry itself, and B's
//!   rows must be byte-for-byte unchanged afterwards.
//! - The completeness guard scans `src/*.rs` for `/api` path literals and
//!   fails on any route missing from [`ROUTES`], or listed there but gone
//!   from the source, so a new route forces a classification here.
//!
//! Needs `docker compose up -d`; fails, never skips, when it is down.

mod support;

use std::collections::BTreeSet;

use reqwest::StatusCode;
use serde_json::{json, Value};
use support::hub::{spawn_hub_with_root, Hub, Who, NEW_PASSWORD, PASSWORD};
use support::unique_address;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Needs a customer session; swept.
    Customer,
    /// Needs no session; the reason it exposes no organization's records.
    Anonymous(&'static str),
    /// Vendor admins only; swept as customers, who must get 403.
    Admin,
    /// The JSON 404 for unknown `/api` paths.
    CatchAll,
}

use Class::*;

/// Every `/api` route the hub serves, by method and axum path.
const ROUTES: &[(&str, &str, Class)] = &[
    (
        "GET",
        "/api/health",
        Anonymous("liveness of the database and mail; no account data"),
    ),
    (
        "GET",
        "/api/deployment",
        Anonymous("hub configuration only; no account data"),
    ),
    (
        "GET",
        "/api/auth/csrf",
        Anonymous("issues a CSRF token; no account data"),
    ),
    (
        "POST",
        "/api/auth/sign-up",
        Anonymous("creates a new organization of the caller's own"),
    ),
    (
        "POST",
        "/api/auth/verify-email",
        Anonymous("the emailed single-use token is the credential"),
    ),
    (
        "POST",
        "/api/auth/sign-in",
        Anonymous("the password is the credential"),
    ),
    (
        "POST",
        "/api/auth/magic-link",
        Anonymous("answers 202 {} whether or not the account exists"),
    ),
    (
        "POST",
        "/api/auth/magic-link/sign-in",
        Anonymous("the emailed single-use token is the credential"),
    ),
    (
        "POST",
        "/api/auth/password-reset",
        Anonymous("answers 202 {} whether or not the account exists"),
    ),
    (
        "POST",
        "/api/auth/password-reset/complete",
        Anonymous("the emailed single-use token is the credential"),
    ),
    (
        "POST",
        "/api/invitations/lookup",
        Anonymous("the emailed invitation token is the credential"),
    ),
    (
        "POST",
        "/api/invitations/accept",
        Anonymous("the emailed invitation token is the credential"),
    ),
    (
        "POST",
        "/api/leads",
        Anonymous("stores a sales lead; answers 202 {} whether or not it is known"),
    ),
    (
        "POST",
        "/api/leads/unsubscribe",
        Anonymous("the emailed unsubscribe token is the credential"),
    ),
    (
        "POST",
        "/api/payments/webhook",
        Anonymous("the provider's signature is the credential; answers only {received}"),
    ),
    ("POST", "/api/auth/sign-out", Customer),
    ("POST", "/api/auth/sign-out-everywhere", Customer),
    ("GET", "/api/auth/me", Customer),
    ("POST", "/api/auth/verification-email", Customer),
    ("POST", "/api/auth/password", Customer),
    ("POST", "/api/account/profile", Customer),
    ("POST", "/api/organization", Customer),
    ("GET", "/api/organization/members", Customer),
    ("POST", "/api/organization/members/:id/role", Customer),
    ("POST", "/api/organization/members/:id/remove", Customer),
    ("GET", "/api/organization/invitations", Customer),
    ("POST", "/api/organization/invitations", Customer),
    ("POST", "/api/organization/invitations/:id/revoke", Customer),
    (
        "GET",
        "/api/offerings",
        Anonymous("published offerings only; the public price list, no organization's records"),
    ),
    ("POST", "/api/checkout", Customer),
    ("GET", "/api/orders", Customer),
    ("GET", "/api/orders/:id", Customer),
    ("GET", "/api/entitlements", Customer),
    ("POST", "/api/billing/portal", Customer),
    ("GET", "/api/payments/fake/sessions/:id", Customer),
    ("POST", "/api/payments/fake/sessions/:id/pay", Customer),
    ("POST", "/api/payments/fake/sessions/:id/abandon", Customer),
    (
        "POST",
        "/api/payments/fake/entitlements/:id/renew",
        Customer,
    ),
    (
        "POST",
        "/api/payments/fake/entitlements/:id/cancel",
        Customer,
    ),
    ("GET", "/api/stations", Customer),
    ("POST", "/api/stations", Customer),
    ("POST", "/api/stations/:id/release", Customer),
    ("POST", "/api/stations/:id/reissue", Customer),
    ("GET", "/api/licences/:id/download", Customer),
    ("GET", "/api/licensing/root", Customer),
    ("GET", "/api/organization/lapse-policy", Customer),
    ("POST", "/api/organization/lapse-policy", Customer),
    ("GET", "/api/downloads", Customer),
    ("POST", "/api/downloads/:artifact_id/link", Customer),
    (
        "GET",
        "/api/downloads/:artifact_id/file",
        Anonymous("the signed link is the credential; re-checked against its user's organization"),
    ),
    ("GET", "/api/admin/organizations", Admin),
    ("GET", "/api/admin/users", Admin),
    ("GET", "/api/admin/organizations/:id", Admin),
    ("GET", "/api/admin/leads", Admin),
    ("POST", "/api/admin/leads/export", Admin),
    ("GET", "/api/admin/leads/:id", Admin),
    ("POST", "/api/admin/leads/:id/status", Admin),
    ("POST", "/api/admin/leads/:id/notes", Admin),
    ("POST", "/api/admin/leads/:id/erase", Admin),
    ("GET", "/api/admin/offerings", Admin),
    ("POST", "/api/admin/offerings", Admin),
    ("GET", "/api/admin/offerings/:id", Admin),
    ("POST", "/api/admin/offerings/:id", Admin),
    ("POST", "/api/admin/offerings/:id/publish", Admin),
    ("POST", "/api/admin/offerings/:id/retire", Admin),
    ("GET", "/api/admin/organizations/:id/licences", Admin),
    ("POST", "/api/admin/organizations/:id/licences", Admin),
    ("POST", "/api/admin/licences/inspect", Admin),
    ("GET", "/api/admin/licences/:id", Admin),
    ("GET", "/api/admin/licences/:id/download", Admin),
    ("POST", "/api/admin/licences/:id/revoke", Admin),
    ("GET", "/api/admin/releases", Admin),
    ("POST", "/api/admin/releases", Admin),
    ("GET", "/api/admin/releases/:id", Admin),
    ("GET", "/api/admin/releases/:id/downloads", Admin),
    ("POST", "/api/admin/releases/:id/artifacts", Admin),
    (
        "POST",
        "/api/admin/releases/:id/artifacts/:artifact_id/remove",
        Admin,
    ),
    ("POST", "/api/admin/releases/:id/publish", Admin),
    ("POST", "/api/admin/releases/:id/withdraw", Admin),
    ("ANY", "/api", CatchAll),
    ("ANY", "/api/", CatchAll),
    ("ANY", "/api/*rest", CatchAll),
];

// ---------------------------------------------------------------------------
// Completeness guard

/// Every string literal in `source` that is `"/api"` or starts `"/api/"`
/// (so `"/apis"`, a negative case in the CSRF unit tests, is not a route).
fn api_literals(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(i) = rest.find("\"/api") {
        let after = &rest[i + 1..];
        let end = after.find('"').expect("unterminated string literal");
        let lit = &after[..end];
        if lit == "/api" || lit.starts_with("/api/") {
            out.push(lit.to_string());
        }
        rest = &after[end + 1..];
    }
    out
}

#[test]
fn every_api_route_in_the_source_is_classified() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut found = BTreeSet::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        found.extend(api_literals(&source));
    }
    let listed: BTreeSet<String> = ROUTES.iter().map(|(_, p, _)| p.to_string()).collect();
    let unclassified: Vec<_> = found.difference(&listed).collect();
    assert!(
        unclassified.is_empty(),
        "classify these /api paths in tests/isolation.rs ROUTES: {unclassified:?}"
    );
    let gone: Vec<_> = listed.difference(&found).collect();
    assert!(
        gone.is_empty(),
        "ROUTES lists paths no longer in src/: {gone:?}"
    );
}

#[test]
fn the_literal_scanner_finds_routes_only() {
    let src = r#"
        .route("/api/a", get(a))
        .route(
            "/api/b/:id",
            post(b),
        )
        assert!(!is_api_path("/apis"));
        assert!(is_api_path("/api"));
        let s = "not an api path";
    "#;
    assert_eq!(api_literals(src), ["/api/a", "/api/b/:id", "/api"]);
}

// ---------------------------------------------------------------------------
// The sweep

/// One organization: verified owner, member, pending invitation, and a
/// pending order with its checkout session id.
struct Tenant {
    org: Uuid,
    owner: Who,
    member: Who,
    invitation: String,
    invited: String,
    order: String,
    session: String,
}

async fn tenant(hub: &Hub, org_name: &str, names: Option<(&str, &str)>, offering: &str) -> Tenant {
    let owner = hub.verified_owner(org_name).await;
    let member = hub.add(&owner, "member").await;
    let invited = unique_address();
    let resp = hub.invite(&owner, &invited, "member").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value = resp.json().await.unwrap();
    let invitation = body["invitation"]["id"].as_str().unwrap().to_string();
    let org: Uuid = sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1::uuid")
        .bind(&owner.id)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    let resp = hub
        .post(
            "/api/checkout",
            json!({ "offering_id": offering }),
            Some(&owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value = resp.json().await.unwrap();
    let order = body["order"]["id"].as_str().unwrap().to_string();
    let session: String =
        sqlx::query_scalar("SELECT provider_session_id FROM orders WHERE id = $1::uuid")
            .bind(&order)
            .fetch_one(&hub.db.pool)
            .await
            .unwrap();
    if let Some((owner_name, member_name)) = names {
        // Names no other user has, so a leak cannot hide behind a shared one.
        for (who, name) in [(&owner, owner_name), (&member, member_name)] {
            sqlx::query("UPDATE users SET name = $2 WHERE id = $1::uuid")
                .bind(&who.id)
                .bind(name)
                .execute(&hub.db.pool)
                .await
                .unwrap();
        }
    }
    Tenant {
        org,
        owner,
        member,
        invitation,
        invited,
        order,
        session,
    }
}

/// Everything stored about organization `org`, as JSON text: the
/// organization row, its users, its invitations, its orders, its billing
/// customers, its entitlements and their renewals, its stations and
/// licences, and its users' sessions and emailed tokens (hashes, expiries,
/// `used_at`), each in full.
async fn rows_of(hub: &Hub, org: Uuid) -> Vec<String> {
    let mut out = Vec::new();
    for sql in [
        "SELECT row_to_json(o)::text FROM organizations o WHERE id = $1",
        "SELECT row_to_json(u)::text FROM users u WHERE organization_id = $1 ORDER BY id",
        "SELECT row_to_json(i)::text FROM invitations i WHERE organization_id = $1 ORDER BY id",
        "SELECT row_to_json(o)::text FROM orders o WHERE organization_id = $1 ORDER BY id",
        "SELECT row_to_json(c)::text FROM billing_customers c WHERE organization_id = $1 \
         ORDER BY provider",
        "SELECT row_to_json(e)::text FROM entitlements e WHERE organization_id = $1 ORDER BY id",
        "SELECT row_to_json(r)::text FROM entitlement_renewals r \
         JOIN entitlements e ON e.id = r.entitlement_id \
         WHERE e.organization_id = $1 ORDER BY r.invoice_id",
        "SELECT row_to_json(s)::text FROM stations s WHERE organization_id = $1 ORDER BY id",
        "SELECT row_to_json(l)::text FROM licences l WHERE organization_id = $1 ORDER BY id",
        "SELECT row_to_json(s)::text FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE u.organization_id = $1 ORDER BY s.token_hash",
        "SELECT row_to_json(t)::text FROM auth_tokens t JOIN users u ON u.id = t.user_id \
         WHERE u.organization_id = $1 ORDER BY t.token_hash",
    ] {
        let rows: Vec<String> = sqlx::query_scalar(sql)
            .bind(org)
            .fetch_all(&hub.db.pool)
            .await
            .unwrap();
        out.extend(rows);
    }
    out
}

/// `owner` buys `offering` (annual) through the Fake provider and renews it
/// once: the entitlement's id.
async fn paid_renewed_entitlement(hub: &Hub, owner: &Who, offering: &str) -> String {
    let resp = hub
        .post(
            "/api/checkout",
            json!({ "offering_id": offering }),
            Some(&owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value = resp.json().await.unwrap();
    let order = body["order"]["id"].as_str().unwrap().to_string();
    let session: String =
        sqlx::query_scalar("SELECT provider_session_id FROM orders WHERE id = $1::uuid")
            .bind(&order)
            .fetch_one(&hub.db.pool)
            .await
            .unwrap();
    let resp = hub
        .post_empty(
            &format!("/api/payments/fake/sessions/{session}/pay"),
            Some(&owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let id: Uuid = sqlx::query_scalar("SELECT id FROM entitlements WHERE order_id = $1::uuid")
        .bind(&order)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    let resp = hub
        .post_empty(
            &format!("/api/payments/fake/entitlements/{id}/renew"),
            Some(&owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    id.to_string()
}

/// A fresh session cookie for `user_id`, so routes that end sessions or
/// change passwords cannot disturb the next call.
async fn fresh_cookie(hub: &Hub, user_id: &str) -> String {
    let id = Uuid::parse_str(user_id).unwrap();
    let token = wipe_hub::accounts::create_session(&hub.db.pool, id)
        .await
        .unwrap();
    format!("wipehub_session={token}")
}

#[tokio::test]
async fn organization_a_never_sees_or_changes_organization_b() {
    // With a vendor root, so registration reaches its entitlement lookup.
    let (hub, _root) = spawn_hub_with_root().await;
    // Offerings are the vendor's, the same for every organization.
    let offering = hub.published_offering("Station annual").await;
    let a = tenant(&hub, "Alpha Works", None, &offering).await;
    let b = tenant(
        &hub,
        "Bravo Holdings",
        Some(("Bertha Quill", "Boris Quill")),
        &offering,
    )
    .await;
    // B has paid through Stripe before: a stored billing customer.
    sqlx::query(
        "INSERT INTO billing_customers (organization_id, provider, customer_id) \
         VALUES ($1, 'stripe', 'cus_bravo_secret')",
    )
    .bind(b.org)
    .execute(&hub.db.pool)
    .await
    .unwrap();
    // B has a paid, renewing order (an entitlement) renewed once.
    let b_entitlement = paid_renewed_entitlement(&hub, &b.owner, &offering).await;
    // B registered a station on it and holds its licence.
    let b_key = wipe_cert::SigningKey::generate().public_key_id();
    let resp = hub
        .post(
            "/api/stations",
            json!({
                "entitlement_id": b_entitlement,
                "instance_key_id": b_key,
                "station_id": "bravo-bench",
            }),
            Some(&b.owner.cookie),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value = resp.json().await.unwrap();
    let b_station = body["station"]["id"].as_str().unwrap().to_string();
    let b_licence = body["licence"]["id"].as_str().unwrap().to_string();
    let b_before = rows_of(&hub, b.org).await;
    assert!(
        b_before.iter().any(|r| r.contains(&b_entitlement)),
        "{b_before:?}"
    );
    assert!(
        b_before.iter().any(|r| r.contains("fake_in_")),
        "{b_before:?}"
    );
    assert!(
        b_before.iter().any(|r| r.contains("cus_bravo_secret")),
        "{b_before:?}"
    );

    // Every identifier of B's that a response must not carry.
    let secrets: Vec<String> = vec![
        b.org.to_string(),
        "Bravo Holdings".into(),
        b.owner.id.clone(),
        b.member.id.clone(),
        b.owner.email.clone(),
        b.member.email.clone(),
        b.invited.clone(),
        "Bertha Quill".into(),
        "Boris Quill".into(),
        b.invitation.clone(),
        b.order.clone(),
        b.session.clone(),
        "cus_bravo_secret".into(),
        b_entitlement.clone(),
        b_station.clone(),
        b_licence.clone(),
        b_key.clone(),
        "bravo-bench".into(),
    ];

    let mut swept = 0;
    for &(method, path, class) in ROUTES {
        if !matches!(class, Customer | Admin) {
            continue;
        }
        for (role, who) in [("owner", &a.owner), ("member", &a.member)] {
            let target = if path.starts_with("/api/organization/members/") {
                b.member.id.clone()
            } else if path.starts_with("/api/organization/invitations/") {
                b.invitation.clone()
            } else if path.starts_with("/api/orders/") {
                b.order.clone()
            } else if path.starts_with("/api/payments/fake/sessions/") {
                b.session.clone()
            } else if path.starts_with("/api/payments/fake/entitlements/") {
                b_entitlement.clone()
            } else if path.starts_with("/api/stations/") {
                b_station.clone()
            } else if path.starts_with("/api/licences/") {
                b_licence.clone()
            } else {
                // Only admin routes remain with `:id`; they refuse a
                // customer before reading it.
                b.org.to_string()
            };
            let url_path = path.replace(":id", &target);
            let body = match path {
                "/api/auth/password" => {
                    Some(json!({"current_password": PASSWORD, "new_password": NEW_PASSWORD}))
                }
                "/api/account/profile" => Some(json!({"name": "Alpha Person"})),
                "/api/organization" => Some(json!({"name": "Alpha Works Renamed"})),
                "/api/organization/lapse-policy" if method == "POST" => {
                    Some(json!({"lapse_policy": "stop"}))
                }
                "/api/organization/members/:id/role" => Some(json!({"role": "owner"})),
                "/api/checkout" => Some(json!({ "offering_id": offering })),
                "/api/stations" if method == "POST" => Some(json!({
                    "entitlement_id": b_entitlement,
                    "instance_key_id": wipe_cert::SigningKey::generate().public_key_id(),
                    "station_id": "alpha-bench",
                })),
                "/api/organization/invitations" if method == "POST" => {
                    Some(json!({"email": b.member.email, "role": "member"}))
                }
                _ => None,
            };
            let cookie = fresh_cookie(&hub, &who.id).await;
            let resp = match (method, &body) {
                ("GET", _) => hub.get(&url_path, Some(&cookie)).await,
                ("POST", Some(v)) => hub.post(&url_path, v.clone(), Some(&cookie)).await,
                ("POST", None) => hub.post_empty(&url_path, Some(&cookie)).await,
                _ => unreachable!("{method} {path}"),
            };
            let status = resp.status();
            let text = resp.text().await.unwrap();
            assert!(
                !status.is_server_error(),
                "{role} {method} {url_path}: {status} {text}"
            );
            if class == Admin {
                assert_eq!(status, StatusCode::FORBIDDEN, "{role} {method} {url_path}");
            }
            if path.contains(":id") && class == Customer {
                // A's owner hears "no such …"; A's member is refused first,
                // except where every role may read (an order, a licence).
                let want = if role == "owner"
                    || path == "/api/orders/:id"
                    || path == "/api/licences/:id/download"
                {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::FORBIDDEN
                };
                assert_eq!(status, want, "{role} {method} {url_path}: {text}");
            }
            let carried = format!(
                "{url_path} {}",
                body.map(|v| v.to_string()).unwrap_or_default()
            );
            for secret in &secrets {
                assert!(
                    carried.contains(secret.as_str()) || !text.contains(secret.as_str()),
                    "{role} {method} {url_path} leaked {secret:?}: {text}"
                );
            }
            swept += 1;
        }
    }
    let expected = ROUTES
        .iter()
        .filter(|(_, _, c)| matches!(c, Customer | Admin))
        .count()
        * 2;
    assert_eq!(swept, expected);

    assert_eq!(rows_of(&hub, b.org).await, b_before, "B's records changed");
    // The sweep did reach A: its owner renamed it.
    let a_name: String = sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
        .bind(a.org)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    assert_eq!(a_name, "Alpha Works Renamed");
}
