//! Embedded migrations and the per-test database harness. Needs
//! `docker compose up -d`; fails, never skips, when Postgres is down.

mod support;

use support::{database_exists, TestDb};
use wipe_hub::db;

async fn migration_rows(db: &TestDb) -> Vec<(i64, bool)> {
    sqlx::query_as("SELECT version, success FROM _sqlx_migrations ORDER BY version")
        .fetch_all(&db.pool)
        .await
        .expect("read _sqlx_migrations")
}

fn embedded_versions() -> Vec<i64> {
    db::MIGRATOR
        .iter()
        .filter(|m| m.migration_type.is_up_migration())
        .map(|m| m.version)
        .collect()
}

#[tokio::test]
async fn empty_database_migrates_to_every_embedded_migration() {
    let t = TestDb::empty().await;
    db::migrate(&t.pool).await.unwrap();
    let rows = migration_rows(&t).await;
    assert_eq!(rows.len(), db::embedded_count());
    assert_eq!(
        rows.iter().map(|r| r.0).collect::<Vec<_>>(),
        embedded_versions()
    );
    assert!(rows.iter().all(|r| r.1), "{rows:?}");
}

#[tokio::test]
async fn migrating_twice_is_a_no_op() {
    let t = TestDb::migrated().await;
    let first = migration_rows(&t).await;
    db::migrate(&t.pool).await.unwrap();
    db::migrate_url(&t.url).await.unwrap();
    assert_eq!(migration_rows(&t).await, first);
}

#[tokio::test]
async fn migrate_url_applies_every_migration() {
    let t = TestDb::empty().await;
    db::migrate_url(&t.url).await.unwrap();
    assert_eq!(migration_rows(&t).await.len(), db::embedded_count());
}

#[tokio::test]
async fn two_test_databases_are_isolated() {
    let a = TestDb::migrated().await;
    let b = TestDb::migrated().await;
    assert_ne!(a.name, b.name);
    sqlx::query("CREATE TABLE scratch (id int)")
        .execute(&a.pool)
        .await
        .unwrap();
    let in_a: Option<String> = sqlx::query_scalar("SELECT to_regclass('public.scratch')::text")
        .fetch_one(&a.pool)
        .await
        .unwrap();
    let in_b: Option<String> = sqlx::query_scalar("SELECT to_regclass('public.scratch')::text")
        .fetch_one(&b.pool)
        .await
        .unwrap();
    assert!(in_a.is_some());
    assert!(
        in_b.is_none(),
        "a write in {} is visible in {}",
        a.name,
        b.name
    );
}

#[tokio::test]
async fn dropped_guard_drops_the_database() {
    let t = TestDb::migrated().await;
    let name = t.name.clone();
    assert!(database_exists(&name).await);
    drop(t);
    assert!(!database_exists(&name).await, "{name} was left behind");
}

#[tokio::test]
async fn panicking_test_still_drops_its_database() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let t = TestDb::migrated().await;
        tx.send(t.name.clone()).unwrap();
        panic!("deliberate panic while holding {}", t.name);
    });
    let name = rx.await.expect("task sent its database name");
    let err = task.await.expect_err("the task panicked");
    assert!(err.is_panic());
    assert!(!database_exists(&name).await, "{name} was left behind");
}

#[tokio::test]
#[should_panic(expected = "run docker compose up -d")]
async fn unreachable_postgres_panics_with_the_compose_hint() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    TestDb::empty_on(&format!(
        "postgres://wipehub:wipehub@127.0.0.1:{port}/wipehub"
    ))
    .await;
}

#[tokio::test]
async fn auth_tokens_accept_the_story_3_purposes_only() {
    let t = TestDb::migrated().await;
    let org: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Org') RETURNING id")
            .fetch_one(&t.pool)
            .await
            .unwrap();
    let user: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (organization_id, email, name, password_hash, role) \
         VALUES ($1, 'a@example.com', 'A', 'x', 'owner') RETURNING id",
    )
    .bind(org)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    let insert = |n: u8, purpose: &'static str| {
        let pool = t.pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO auth_tokens (token_hash, user_id, purpose, expires_at) \
                 VALUES ($1, $2, $3, now())",
            )
            .bind(vec![n; 32])
            .bind(user)
            .bind(purpose)
            .execute(&pool)
            .await
        }
    };
    for (n, purpose) in [
        (1, "verify_email"),
        (2, "magic_link"),
        (3, "password_reset"),
    ] {
        insert(n, purpose)
            .await
            .unwrap_or_else(|e| panic!("{purpose}: {e}"));
    }
    let err = insert(4, "other").await.unwrap_err();
    match err {
        sqlx::Error::Database(d) => {
            assert_eq!(d.constraint(), Some("auth_tokens_purpose_check"))
        }
        other => panic!("expected a CHECK violation, got {other:?}"),
    }
}

#[tokio::test]
async fn invitations_accept_owner_and_member_roles_and_32_byte_unique_hashes() {
    let t = TestDb::migrated().await;
    let org: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Org') RETURNING id")
            .fetch_one(&t.pool)
            .await
            .unwrap();
    let insert = |hash: Vec<u8>, role: &'static str| {
        let pool = t.pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO invitations (organization_id, email, role, token_hash, expires_at) \
                 VALUES ($1, 'a@example.com', $2, $3, now() + interval '7 days')",
            )
            .bind(org)
            .bind(role)
            .bind(hash)
            .execute(&pool)
            .await
        }
    };
    insert(vec![1; 32], "owner").await.unwrap();
    insert(vec![2; 32], "member").await.unwrap();
    let constraint = |e: sqlx::Error| match e {
        sqlx::Error::Database(d) => d.constraint().map(str::to_string),
        other => panic!("expected a constraint violation, got {other:?}"),
    };
    assert_eq!(
        constraint(insert(vec![3; 32], "vendor_admin").await.unwrap_err()).as_deref(),
        Some("invitations_role_check")
    );
    assert_eq!(
        constraint(insert(vec![4; 16], "member").await.unwrap_err()).as_deref(),
        Some("invitations_token_hash_check")
    );
    assert_eq!(
        constraint(insert(vec![1; 32], "member").await.unwrap_err()).as_deref(),
        Some("invitations_token_hash_key")
    );
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexname::text FROM pg_indexes WHERE tablename = 'invitations' ORDER BY 1",
    )
    .fetch_all(&t.pool)
    .await
    .unwrap();
    for want in [
        "invitations_invited_by_idx",
        "invitations_organization_email_idx",
    ] {
        assert!(indexes.iter().any(|i| i == want), "{want} in {indexes:?}");
    }
}

#[tokio::test]
async fn at_most_one_organization_is_the_vendor_organization() {
    let t = TestDb::migrated().await;
    let insert = |name: &'static str, is_vendor: bool| {
        let pool = t.pool.clone();
        async move {
            sqlx::query("INSERT INTO organizations (name, is_vendor) VALUES ($1, $2)")
                .bind(name)
                .bind(is_vendor)
                .execute(&pool)
                .await
        }
    };
    // The flag defaults to false, and any number of customers may exist.
    sqlx::query("INSERT INTO organizations (name) VALUES ('Customer')")
        .execute(&t.pool)
        .await
        .unwrap();
    insert("Another customer", false).await.unwrap();
    insert("Vendor staff", true).await.unwrap();
    match insert("Second vendor", true).await.unwrap_err() {
        sqlx::Error::Database(d) => assert_eq!(d.constraint(), Some("organizations_one_vendor")),
        other => panic!("expected a unique violation, got {other:?}"),
    }
    let vendors: i64 = sqlx::query_scalar("SELECT count(*) FROM organizations WHERE is_vendor")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(vendors, 1);
}

#[tokio::test]
async fn leads_are_unique_by_lowercased_email_and_constrained() {
    let t = TestDb::migrated().await;
    let lead = |email: &str, segment: Option<&'static str>, status: &'static str| {
        let email = email.to_string();
        let pool = t.pool.clone();
        async move {
            sqlx::query_scalar::<_, uuid::Uuid>(
                "INSERT INTO leads (email, name, company, segment, status) \
                 VALUES ($1, 'Ada', 'Engines', $2, $3) RETURNING id",
            )
            .bind(email)
            .bind(segment)
            .bind(status)
            .fetch_one(&pool)
            .await
        }
    };
    let constraint = |e: sqlx::Error| match e {
        sqlx::Error::Database(d) => d.constraint().map(str::to_string),
        other => panic!("expected a constraint violation, got {other:?}"),
    };
    let id = lead("ada@example.com", Some("itad"), "new").await.unwrap();
    for status in ["contacted", "qualified", "closed"] {
        lead(&format!("{status}@example.com"), None, status)
            .await
            .unwrap();
    }
    assert_eq!(
        constraint(lead("ADA@example.com", None, "new").await.unwrap_err()).as_deref(),
        Some("leads_email_key")
    );
    assert_eq!(
        constraint(
            lead("b@example.com", Some("ITAD"), "new")
                .await
                .unwrap_err()
        )
        .as_deref(),
        Some("leads_segment_check")
    );
    assert_eq!(
        constraint(lead("c@example.com", None, "won").await.unwrap_err()).as_deref(),
        Some("leads_status_check")
    );

    let submission = |kind: &'static str, consent: &'static str, hash: Vec<u8>| {
        let pool = t.pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO lead_submissions \
                     (lead_id, kind, consent_text, unsubscribe_token_hash) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(kind)
            .bind(consent)
            .bind(hash)
            .execute(&pool)
            .await
        }
    };
    submission("early-access", "I agree.", vec![1; 32])
        .await
        .unwrap();
    submission("contact", "I agree.", vec![2; 32])
        .await
        .unwrap();
    for (kind, consent, hash, want) in [
        (
            "newsletter",
            "I agree.",
            vec![3; 32],
            "lead_submissions_kind_check",
        ),
        (
            "contact",
            " ",
            vec![4; 32],
            "lead_submissions_consent_text_check",
        ),
        (
            "contact",
            "I agree.",
            vec![5; 16],
            "lead_submissions_unsubscribe_token_hash_check",
        ),
        (
            "contact",
            "I agree.",
            vec![1; 32],
            "lead_submissions_unsubscribe_token_hash_key",
        ),
    ] {
        assert_eq!(
            constraint(submission(kind, consent, hash).await.unwrap_err()).as_deref(),
            Some(want)
        );
    }
    // Deleting a lead deletes its submissions.
    sqlx::query("DELETE FROM leads WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM lead_submissions")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn lead_notes_need_a_body_and_go_with_their_lead_but_not_their_author() {
    let t = TestDb::migrated().await;
    let lead: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO leads (email, name, company) VALUES ('ada@example.com', 'Ada', 'Engines') \
         RETURNING id",
    )
    .fetch_one(&t.pool)
    .await
    .unwrap();
    let org: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Vendor') RETURNING id")
            .fetch_one(&t.pool)
            .await
            .unwrap();
    let author: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (organization_id, email, name, password_hash, role) \
         VALUES ($1, 'vera@example.com', 'Vera', 'x', 'vendor_admin') RETURNING id",
    )
    .bind(org)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    let note = |body: &'static str| {
        let pool = t.pool.clone();
        async move {
            sqlx::query("INSERT INTO lead_notes (lead_id, author_id, body) VALUES ($1, $2, $3)")
                .bind(lead)
                .bind(author)
                .bind(body)
                .execute(&pool)
                .await
        }
    };
    note("called").await.unwrap();
    note("left a message").await.unwrap();
    match note(" \n ").await.unwrap_err() {
        sqlx::Error::Database(d) => assert_eq!(d.constraint(), Some("lead_notes_body_check")),
        other => panic!("expected a constraint violation, got {other:?}"),
    }
    let count = |sql: &'static str| {
        let pool = t.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };

    // Removing the author keeps the notes, unattributed.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(author)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        count("SELECT count(*) FROM lead_notes WHERE author_id IS NULL").await,
        2
    );

    // Deleting the lead deletes its notes.
    sqlx::query("DELETE FROM leads WHERE id = $1")
        .bind(lead)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(count("SELECT count(*) FROM lead_notes").await, 0);
}

/// A paid order (organization, offering, order) inserted directly: the
/// order's and organization's ids.
async fn paid_order(t: &TestDb, interval: &str) -> (uuid::Uuid, uuid::Uuid) {
    let org: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Acme') RETURNING id")
            .fetch_one(&t.pool)
            .await
            .unwrap();
    let offering: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO offerings (name, price_minor, currency, billing_interval, scope, seats, \
                                term_months) \
         VALUES ('Station ' || gen_random_uuid(), 120000, 'USD', $1, 'machine', 1, 12) \
         RETURNING id",
    )
    .bind(interval)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    let order: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO orders (organization_id, offering_id, offering_name, price_minor, currency, \
                             billing_interval, scope, seats, term_months, provider, \
                             provider_session_id) \
         VALUES ($1, $2, 'Station', 120000, 'USD', $3, 'machine', 1, 12, 'stripe', \
                 'cs_' || gen_random_uuid()) RETURNING id",
    )
    .bind(org)
    .bind(offering)
    .bind(interval)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE orders SET status = 'paid', paid_at = now() WHERE id = $1")
        .bind(order)
        .execute(&t.pool)
        .await
        .unwrap();
    (order, org)
}

#[tokio::test]
async fn orders_gain_a_terminal_refunded_status_after_paid_only() {
    let t = TestDb::migrated().await;
    let (order, _) = paid_order(&t, "annual").await;
    let pool = &t.pool;
    for sql in [
        // Not without its stamp, and the payment time stays.
        "UPDATE orders SET status = 'refunded' WHERE id = $1",
        "UPDATE orders SET status = 'refunded', refunded_at = now(), paid_at = NULL WHERE id = $1",
        "UPDATE orders SET status = 'refunded', refunded_at = now(), \
         paid_at = paid_at - interval '1 day' WHERE id = $1",
        "UPDATE orders SET refunded_at = now() WHERE id = $1",
        "UPDATE orders SET status = 'expired', expired_at = now(), paid_at = NULL WHERE id = $1",
    ] {
        sqlx::query(sql)
            .bind(order)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    sqlx::query(
        "UPDATE orders SET status = 'refunded', refunded_at = now(), updated_at = now() \
         WHERE id = $1",
    )
    .bind(order)
    .execute(pool)
    .await
    .unwrap();
    for sql in [
        "UPDATE orders SET status = 'paid', refunded_at = NULL WHERE id = $1",
        "UPDATE orders SET refunded_at = now() - interval '1 day' WHERE id = $1",
        "UPDATE orders SET updated_at = now() + interval '1 day' WHERE id = $1",
        "DELETE FROM orders WHERE id = $1",
    ] {
        sqlx::query(sql)
            .bind(order)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    // A pending order cannot be refunded.
    let pending: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO orders (organization_id, offering_id, offering_name, price_minor, currency, \
                             billing_interval, scope, seats, term_months, provider, \
                             provider_session_id) \
         SELECT organization_id, offering_id, offering_name, price_minor, currency, \
                billing_interval, scope, seats, term_months, provider, 'cs_pending' \
         FROM orders WHERE id = $1 RETURNING id",
    )
    .bind(order)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE orders SET status = 'refunded', refunded_at = now() WHERE id = $1")
        .bind(pending)
        .execute(pool)
        .await
        .expect_err("pending -> refunded");
}

#[tokio::test]
async fn entitlements_and_renewals_refuse_illegal_changes() {
    let t = TestDb::migrated().await;
    let (order, org) = paid_order(&t, "annual").await;
    let pool = &t.pool;
    let ent: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO entitlements (organization_id, order_id, provider, subscription_id, \
                                   payment_ref, term_start, term_end, renews) \
         VALUES ($1, $2, 'stripe', 'sub_1', 'in_0', now(), now() + interval '12 months', true) \
         RETURNING id",
    )
    .bind(org)
    .bind(order)
    .fetch_one(pool)
    .await
    .unwrap();
    // One entitlement per order, one per subscription; sane terms.
    for sql in [
        "INSERT INTO entitlements (organization_id, order_id, provider, term_start, term_end, \
                                   renews) \
         SELECT organization_id, order_id, provider, now(), now() + interval '1 month', false \
         FROM entitlements WHERE id = $1",
        "UPDATE entitlements SET term_end = term_start WHERE id = $1",
    ] {
        sqlx::query(sql)
            .bind(ent)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    let renewal = |invoice: &'static str| {
        sqlx::query(
            "INSERT INTO entitlement_renewals (provider, invoice_id, entitlement_id, \
                                               previous_term_end, term_end) \
             SELECT 'stripe', $2, id, term_end, term_end + interval '12 months' \
             FROM entitlements WHERE id = $1",
        )
        .bind(ent)
        .bind(invoice)
        .execute(pool)
    };
    renewal("in_1").await.unwrap();
    renewal("in_1").await.expect_err("an invoice applies once");
    for sql in [
        "UPDATE entitlement_renewals SET term_end = term_end + interval '1 day' \
         WHERE entitlement_id = $1",
        "DELETE FROM entitlement_renewals WHERE entitlement_id = $1",
    ] {
        sqlx::query(sql)
            .bind(ent)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    for sql in [
        "DELETE FROM entitlements WHERE id = $1",
        "UPDATE entitlements SET term_end = term_end - interval '1 day' WHERE id = $1",
        "UPDATE entitlements SET term_start = term_start - interval '1 day' WHERE id = $1",
        "UPDATE entitlements SET payment_ref = 'pi_other' WHERE id = $1",
        "UPDATE entitlements SET subscription_id = NULL, renews = false WHERE id = $1",
        "UPDATE entitlements SET order_id = gen_random_uuid() WHERE id = $1",
        // Status steps need their stamps; a cancelled entitlement never renews.
        "UPDATE entitlements SET status = 'cancelled', renews = false WHERE id = $1",
        "UPDATE entitlements SET status = 'cancelled', cancelled_at = now() WHERE id = $1",
        "UPDATE entitlements SET status = 'refunded', renews = false WHERE id = $1",
        "UPDATE entitlements SET status = 'bogus' WHERE id = $1",
    ] {
        sqlx::query(sql)
            .bind(ent)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    // Legal: renews off and on, the term growing, then cancellation.
    for sql in [
        "UPDATE entitlements SET renews = false, renewal_changed_at = now() WHERE id = $1",
        "UPDATE entitlements SET term_end = term_end + interval '12 months' WHERE id = $1",
        "UPDATE entitlements SET status = 'cancelled', cancelled_at = now() WHERE id = $1",
    ] {
        sqlx::query(sql).bind(ent).execute(pool).await.expect(sql);
    }
    for sql in [
        "UPDATE entitlements SET status = 'active', cancelled_at = NULL WHERE id = $1",
        "UPDATE entitlements SET cancelled_at = now() + interval '1 day' WHERE id = $1",
        "UPDATE entitlements SET renews = true WHERE id = $1",
    ] {
        sqlx::query(sql)
            .bind(ent)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    // A cancelled entitlement's term never grows (a renewal paid after the
    // subscription ended is refunded at the provider, never granted).
    let err = sqlx::query(
        "UPDATE entitlements SET term_end = term_end + interval '12 months' WHERE id = $1",
    )
    .bind(ent)
    .execute(pool)
    .await
    .expect_err("a cancelled entitlement's term grew");
    assert!(
        err.to_string()
            .contains("only an active entitlement's term can be extended"),
        "{err}"
    );
    sqlx::query("UPDATE entitlements SET status = 'refunded', refunded_at = now() WHERE id = $1")
        .bind(ent)
        .execute(pool)
        .await
        .unwrap();
    // A refunded entitlement never changes again.
    for sql in [
        "UPDATE entitlements SET term_end = term_end + interval '12 months' WHERE id = $1",
        "UPDATE entitlements SET updated_at = now() + interval '1 day' WHERE id = $1",
        "UPDATE entitlements SET status = 'cancelled' WHERE id = $1",
        "UPDATE entitlements SET status = 'active', refunded_at = NULL, cancelled_at = NULL \
         WHERE id = $1",
        "DELETE FROM entitlements WHERE id = $1",
    ] {
        sqlx::query(sql)
            .bind(ent)
            .execute(pool)
            .await
            .expect_err(sql);
    }
}

/// A licence row of `source` inserted directly for organization `org`: its
/// id. A station licence names `station` and `ent`; a vendor one neither.
async fn insert_licence(
    t: &TestDb,
    org: uuid::Uuid,
    station: Option<(uuid::Uuid, uuid::Uuid)>,
    reason: Option<&str>,
) -> Result<uuid::Uuid, sqlx::Error> {
    let source = if station.is_some() {
        "station"
    } else {
        "vendor"
    };
    sqlx::query_scalar(
        "INSERT INTO licences (id, organization_id, station_id, entitlement_id, instance_key_id, \
                               root_key_id, not_after, issued_at, document, source, \
                               issue_reason) \
         VALUES (gen_random_uuid(), $1, $2, $3, 'ed25519:AAAAAAAAAAAAAAAAAAAAAA', \
                 'ed25519:BBBBBBBBBBBBBBBBBBBBBB', now() + interval '30 days', now(), '{}', $4, \
                 $5) RETURNING id",
    )
    .bind(org)
    .bind(station.map(|s| s.0))
    .bind(station.map(|s| s.1))
    .bind(source)
    .bind(reason)
    .fetch_one(&t.pool)
    .await
}

#[tokio::test]
async fn licences_refuse_illegal_sources_and_revocations() {
    let t = TestDb::migrated().await;
    let (order, org) = paid_order(&t, "annual").await;
    let pool = &t.pool;
    let ent: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO entitlements (organization_id, order_id, provider, term_start, term_end, \
                                   renews) \
         VALUES ($1, $2, 'stripe', now(), now() + interval '12 months', false) RETURNING id",
    )
    .bind(org)
    .bind(order)
    .fetch_one(pool)
    .await
    .unwrap();
    let user: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (organization_id, email, name, password_hash, role) \
         VALUES ($1, 'o@example.test', 'O', 'x', 'owner') RETURNING id",
    )
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();
    let station: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO stations (organization_id, entitlement_id, instance_key_id, registered_by) \
         VALUES ($1, $2, 'ed25519:AAAAAAAAAAAAAAAAAAAAAA', $3) RETURNING id",
    )
    .bind(org)
    .bind(ent)
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap();

    // Source shape: a station licence has a station and entitlement and no
    // reason; a vendor licence the reverse, with a non-blank reason.
    let long = "x".repeat(501);
    for (s, reason) in [
        (Some((station, ent)), Some("why")),
        (None, None),
        (None, Some("   ")),
        (None, Some(long.as_str())),
    ] {
        insert_licence(&t, org, s, reason)
            .await
            .expect_err(&format!("{s:?} {reason:?}"));
    }
    sqlx::query(
        "INSERT INTO licences (id, organization_id, station_id, instance_key_id, root_key_id, \
                               not_after, issued_at, document, source) \
         VALUES (gen_random_uuid(), $1, $2, 'ed25519:AAAAAAAAAAAAAAAAAAAAAA', 'r', \
                 now() + interval '1 day', now(), '{}', 'station')",
    )
    .bind(org)
    .bind(station)
    .execute(pool)
    .await
    .expect_err("a station licence without its entitlement");
    let station_licence = insert_licence(&t, org, Some((station, ent)), None)
        .await
        .unwrap();
    let vendor = insert_licence(&t, org, None, Some("evaluation"))
        .await
        .unwrap();

    // Content, source and reason never change; status needs its stamps.
    for sql in [
        "UPDATE licences SET source = 'station' WHERE id = $1",
        "UPDATE licences SET issue_reason = 'other' WHERE id = $1",
        "UPDATE licences SET status = 'bogus' WHERE id = $1",
        "UPDATE licences SET status = 'revoked' WHERE id = $1",
        "UPDATE licences SET status = 'revoked', revoked_at = now() WHERE id = $1",
        "UPDATE licences SET status = 'revoked', revoked_at = now(), revocation_reason = ' ' \
         WHERE id = $1",
        "UPDATE licences SET status = 'revoked', revoked_at = now(), revocation_reason = 'r', \
         superseded_at = now() WHERE id = $1",
        "UPDATE licences SET revoked_at = now(), revocation_reason = 'r' WHERE id = $1",
        "DELETE FROM licences WHERE id = $1",
    ] {
        sqlx::query(sql)
            .bind(vendor)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    // active -> revoked.
    sqlx::query(
        "UPDATE licences SET status = 'revoked', revoked_at = now(), \
         revocation_reason = 'fraud' WHERE id = $1",
    )
    .bind(vendor)
    .execute(pool)
    .await
    .unwrap();
    // A revoked licence never changes again.
    for sql in [
        "UPDATE licences SET status = 'active', revoked_at = NULL, revocation_reason = NULL \
         WHERE id = $1",
        "UPDATE licences SET status = 'superseded', superseded_at = now(), revoked_at = NULL, \
         revocation_reason = NULL WHERE id = $1",
        "UPDATE licences SET revocation_reason = 'other' WHERE id = $1",
        "UPDATE licences SET revoked_at = now() - interval '1 day' WHERE id = $1",
    ] {
        sqlx::query(sql)
            .bind(vendor)
            .execute(pool)
            .await
            .expect_err(sql);
    }

    // active -> superseded -> revoked, keeping when it was superseded.
    sqlx::query(
        "UPDATE licences SET status = 'superseded', superseded_at = now() - interval '1 hour' \
         WHERE id = $1",
    )
    .bind(station_licence)
    .execute(pool)
    .await
    .unwrap();
    for sql in [
        "UPDATE licences SET status = 'active', superseded_at = NULL WHERE id = $1",
        "UPDATE licences SET status = 'revoked', revoked_at = now(), revocation_reason = 'r', \
         superseded_at = now() WHERE id = $1",
        "UPDATE licences SET status = 'revoked', revoked_at = now(), revocation_reason = 'r', \
         superseded_at = NULL WHERE id = $1",
    ] {
        sqlx::query(sql)
            .bind(station_licence)
            .execute(pool)
            .await
            .expect_err(sql);
    }
    sqlx::query(
        "UPDATE licences SET status = 'revoked', revoked_at = now(), revocation_reason = 'r' \
         WHERE id = $1",
    )
    .bind(station_licence)
    .execute(pool)
    .await
    .unwrap();
    let superseded: bool =
        sqlx::query_scalar("SELECT superseded_at IS NOT NULL FROM licences WHERE id = $1")
            .bind(station_licence)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(superseded);
}

#[tokio::test]
async fn an_organization_lapse_policy_defaults_to_evaluation_and_is_checked() {
    let t = TestDb::migrated().await;
    let (id, policy): (uuid::Uuid, String) = sqlx::query_as(
        "INSERT INTO organizations (name) VALUES ('Customer') RETURNING id, lapse_policy",
    )
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!(policy, "evaluation");
    sqlx::query("UPDATE organizations SET lapse_policy = 'stop' WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    for bad in ["sometimes", "Stop", ""] {
        let err = sqlx::query("UPDATE organizations SET lapse_policy = $2 WHERE id = $1")
            .bind(id)
            .bind(bad)
            .execute(&t.pool)
            .await
            .unwrap_err();
        match err {
            sqlx::Error::Database(d) => {
                assert_eq!(
                    d.constraint(),
                    Some("organizations_lapse_policy_check"),
                    "{bad}"
                )
            }
            other => panic!("expected a check violation, got {other:?}"),
        }
    }
    let err = sqlx::query("UPDATE organizations SET lapse_policy = NULL WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap_err();
    assert!(matches!(err, sqlx::Error::Database(_)), "{err:?}");
}

/// A draft release (version `version`) with one artifact: their ids.
async fn draft_release_with_artifact(
    pool: &sqlx::PgPool,
    version: &str,
) -> (uuid::Uuid, uuid::Uuid) {
    let release: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO releases (version, notes, channel) VALUES ($1, 'Notes', 'stable') \
         RETURNING id",
    )
    .bind(version)
    .fetch_one(pool)
    .await
    .unwrap();
    let artifact = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO release_artifacts (id, release_id, platform, filename, size_bytes, sha256) \
         VALUES ($1, $2, 'linux-x86_64', 'wipestation', 3, $3)",
    )
    .bind(artifact)
    .bind(release)
    .bind("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    .execute(pool)
    .await
    .unwrap();
    (release, artifact)
}

async fn refused(pool: &sqlx::PgPool, sql: &str, id: uuid::Uuid) -> String {
    let err = sqlx::query(sql)
        .bind(id)
        .execute(pool)
        .await
        .expect_err(sql);
    match err {
        sqlx::Error::Database(d) => d.message().to_string(),
        other => panic!("{sql}: expected a database error, got {other:?}"),
    }
}

#[tokio::test]
async fn releases_follow_their_lifecycle_and_never_change_or_go() {
    let t = TestDb::migrated().await;
    let pool = &t.pool;
    let (release, artifact) = draft_release_with_artifact(pool, "0.2.0").await;

    // A release is created as a draft only.
    let err = sqlx::query(
        "INSERT INTO releases (version, notes, channel, status, published_at) \
         VALUES ('9.9.9', 'n', 'stable', 'published', now())",
    )
    .execute(pool)
    .await
    .unwrap_err();
    assert!(err.to_string().contains("created as a draft"), "{err}");

    // Stamps without their status step are refused.
    for sql in [
        // A published_at without publishing.
        "UPDATE releases SET published_at = now() WHERE id = $1",
        // Withdrawn without a time.
        "UPDATE releases SET status = 'withdrawn' WHERE id = $1",
    ] {
        sqlx::query(sql)
            .bind(release)
            .execute(pool)
            .await
            .expect_err(sql);
    }

    // Version, channel and notes never change, even in a draft.
    for sql in [
        "UPDATE releases SET version = '0.2.1' WHERE id = $1",
        "UPDATE releases SET notes = 'Other' WHERE id = $1",
        "UPDATE releases SET channel = 'preview' WHERE id = $1",
        "UPDATE releases SET created_at = now() - interval '1 day' WHERE id = $1",
    ] {
        let m = refused(pool, sql, release).await;
        assert!(m.contains("cannot change"), "{sql}: {m}");
    }
    // Artifacts are never updated.
    let m = refused(
        pool,
        "UPDATE release_artifacts SET sha256 = repeat('0', 64) WHERE id = $1",
        artifact,
    )
    .await;
    assert!(m.contains("never updated"), "{m}");

    // Publish, then the published release is frozen.
    sqlx::query("UPDATE releases SET status = 'published', published_at = now() WHERE id = $1")
        .bind(release)
        .execute(pool)
        .await
        .unwrap();
    for sql in [
        "UPDATE releases SET version = '0.3.0' WHERE id = $1",
        "UPDATE releases SET status = 'draft', published_at = NULL WHERE id = $1",
        "UPDATE releases SET published_at = now() - interval '1 day' WHERE id = $1",
        "DELETE FROM releases WHERE id = $1",
    ] {
        refused(pool, sql, release).await;
    }
    let m = refused(pool, "DELETE FROM releases WHERE id = $1", release).await;
    assert!(m.contains("never deleted"), "{m}");
    // No file can be added to, or removed from, a published release.
    let m = refused(
        pool,
        "INSERT INTO release_artifacts (id, release_id, platform, filename, size_bytes, sha256) \
         VALUES (gen_random_uuid(), $1, 'windows-x86_64', 'w.exe', 1, repeat('a', 64))",
        release,
    )
    .await;
    assert!(m.contains("only while their release is a draft"), "{m}");
    let m = refused(
        pool,
        "DELETE FROM release_artifacts WHERE id = $1",
        artifact,
    )
    .await;
    assert!(m.contains("only while their release is a draft"), "{m}");

    // Withdraw; withdrawn is terminal.
    sqlx::query("UPDATE releases SET status = 'withdrawn', withdrawn_at = now() WHERE id = $1")
        .bind(release)
        .execute(pool)
        .await
        .unwrap();
    for sql in [
        "UPDATE releases SET status = 'published', withdrawn_at = NULL WHERE id = $1",
        "UPDATE releases SET withdrawn_at = now() - interval '1 day' WHERE id = $1",
        "DELETE FROM releases WHERE id = $1",
    ] {
        refused(pool, sql, release).await;
    }

    // The version stays taken, withdrawn or not.
    let err = sqlx::query(
        "INSERT INTO releases (version, notes, channel) VALUES ('0.2.0', 'Again', 'stable')",
    )
    .execute(pool)
    .await
    .unwrap_err();
    match err {
        sqlx::Error::Database(d) => assert_eq!(d.constraint(), Some("releases_version_key")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_release_skips_no_step_and_publishes_only_with_a_file() {
    let t = TestDb::migrated().await;
    let pool = &t.pool;
    let empty: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO releases (version, notes, channel) VALUES ('1.0.0', 'n', 'preview') \
         RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let m = refused(
        pool,
        "UPDATE releases SET status = 'published', published_at = now() WHERE id = $1",
        empty,
    )
    .await;
    assert!(m.contains("at least one artifact"), "{m}");

    // draft -> withdrawn is a step; draft -> published -> draft is not.
    let (release, artifact) = draft_release_with_artifact(pool, "1.1.0").await;
    // A draft's files can go.
    sqlx::query("DELETE FROM release_artifacts WHERE id = $1")
        .bind(artifact)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE releases SET status = 'withdrawn', withdrawn_at = now() WHERE id = $1")
        .bind(release)
        .execute(pool)
        .await
        .unwrap();
    let m = refused(
        pool,
        "UPDATE releases SET status = 'published', published_at = now(), withdrawn_at = NULL \
         WHERE id = $1",
        release,
    )
    .await;
    assert!(m.contains("cannot go from withdrawn to published"), "{m}");

    // Bad values are refused by CHECKs.
    for (sql, constraint) in [
        (
            "INSERT INTO releases (version, notes, channel) VALUES ('2.0.0', ' ', 'stable')",
            "releases_notes_check",
        ),
        (
            "INSERT INTO releases (version, notes, channel) VALUES ('2.0.0', 'n', 'beta')",
            "releases_channel_check",
        ),
    ] {
        match sqlx::query(sql).execute(pool).await.unwrap_err() {
            sqlx::Error::Database(d) => assert_eq!(d.constraint(), Some(constraint), "{sql}"),
            other => panic!("{other:?}"),
        }
    }
    let (draft, _) = draft_release_with_artifact(pool, "3.0.0").await;
    for (platform, filename, size, sha) in [
        ("Linux", "f", 1, "a"),
        ("linux", ".f", 1, "a"),
        ("linux", "f", 0, "a"),
        ("linux", "f", 1, "A"),
    ] {
        let sha = if sha == "a" {
            "a".repeat(64)
        } else {
            "A".repeat(64)
        };
        let err = sqlx::query(
            "INSERT INTO release_artifacts (id, release_id, platform, filename, size_bytes, \
                                            sha256) \
             VALUES (gen_random_uuid(), $1, $2, $3, $4, $5)",
        )
        .bind(draft)
        .bind(platform)
        .bind(filename)
        .bind(size as i64)
        .bind(&sha)
        .execute(pool)
        .await
        .unwrap_err();
        assert!(
            matches!(err, sqlx::Error::Database(ref d) if d.constraint().is_some()),
            "{platform} {filename} {size}: {err:?}"
        );
    }
    // One file per platform.
    let err = sqlx::query(
        "INSERT INTO release_artifacts (id, release_id, platform, filename, size_bytes, sha256) \
         VALUES (gen_random_uuid(), $1, 'linux-x86_64', 'other', 1, repeat('b', 64))",
    )
    .bind(draft)
    .execute(pool)
    .await
    .unwrap_err();
    match err {
        sqlx::Error::Database(d) => {
            assert_eq!(d.constraint(), Some("release_artifacts_platform_key"))
        }
        other => panic!("{other:?}"),
    }
}

/// A customer organization and one of its users: their ids.
async fn customer(pool: &sqlx::PgPool) -> (uuid::Uuid, uuid::Uuid) {
    let org: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Acme') RETURNING id")
            .fetch_one(pool)
            .await
            .unwrap();
    (org, uuid::Uuid::new_v4())
}

async fn insert_download(
    pool: &sqlx::PgPool,
    release: uuid::Uuid,
    artifact: uuid::Uuid,
    org: uuid::Uuid,
    user: uuid::Uuid,
) -> Result<uuid::Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO release_downloads (release_id, artifact_id, organization_id, user_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(release)
    .bind(artifact)
    .bind(org)
    .bind(user)
    .fetch_one(pool)
    .await
}

fn db_message(err: sqlx::Error) -> String {
    match err {
        sqlx::Error::Database(d) => d.message().to_string(),
        other => panic!("expected a database error, got {other:?}"),
    }
}

#[tokio::test]
async fn release_downloads_are_append_only_and_of_published_files_only() {
    let t = TestDb::migrated().await;
    let pool = &t.pool;
    let (org, user) = customer(pool).await;
    let (release, artifact) = draft_release_with_artifact(pool, "1.0.0").await;

    // A draft release's file cannot be recorded as downloaded.
    let m = db_message(
        insert_download(pool, release, artifact, org, user)
            .await
            .unwrap_err(),
    );
    assert!(m.contains("only a published release"), "{m}");

    sqlx::query("UPDATE releases SET status = 'published', published_at = now() WHERE id = $1")
        .bind(release)
        .execute(pool)
        .await
        .unwrap();
    // The user need not exist (team removal deletes users).
    let id = insert_download(pool, release, artifact, org, user)
        .await
        .unwrap();

    // Another release's artifact is refused.
    let (other, other_artifact) = draft_release_with_artifact(pool, "2.0.0").await;
    sqlx::query("UPDATE releases SET status = 'published', published_at = now() WHERE id = $1")
        .bind(other)
        .execute(pool)
        .await
        .unwrap();
    let m = db_message(
        insert_download(pool, release, other_artifact, org, user)
            .await
            .unwrap_err(),
    );
    assert!(m.contains("must belong to its release"), "{m}");

    // Never updated, deleted or truncated.
    for sql in [
        "UPDATE release_downloads SET user_id = gen_random_uuid() WHERE id = $1",
        "UPDATE release_downloads SET downloaded_at = now() - interval '1 day' WHERE id = $1",
        "DELETE FROM release_downloads WHERE id = $1",
    ] {
        let m = refused(pool, sql, id).await;
        assert!(m.contains("append-only"), "{sql}: {m}");
    }
    let err = sqlx::query("TRUNCATE release_downloads")
        .execute(pool)
        .await
        .unwrap_err();
    assert!(db_message(err).contains("append-only"));

    // A withdrawn release's file can no longer be recorded; its rows stay.
    sqlx::query("UPDATE releases SET status = 'withdrawn', withdrawn_at = now() WHERE id = $1")
        .bind(release)
        .execute(pool)
        .await
        .unwrap();
    let m = db_message(
        insert_download(pool, release, artifact, org, user)
            .await
            .unwrap_err(),
    );
    assert!(m.contains("only a published release"), "{m}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM release_downloads")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

/// Migration 0020 (controlled-licensing story 1): free trial offerings and
/// the orders and entitlements that taking one writes.
#[tokio::test]
async fn trial_offerings_orders_and_entitlements_are_checked() {
    let t = TestDb::migrated().await;
    let pool = &t.pool;
    // An existing kind of offering defaults to not a trial.
    let (paid, org) = paid_order(&t, "annual").await;
    let trial: bool = sqlx::query_scalar(
        "SELECT f.trial FROM offerings f JOIN orders o ON o.offering_id = f.id WHERE o.id = $1",
    )
    .bind(paid)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(!trial);

    let offering: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO offerings (name, price_minor, currency, billing_interval, scope, seats, \
                                quota_erasures, term_months, trial) \
         VALUES ('Trial', 0, 'USD', 'one_off', 'site', 1, 25, 1, true) RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let insert_order = |price: i64, provider: &'static str| {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO orders (organization_id, offering_id, offering_name, price_minor, \
                                 currency, billing_interval, scope, seats, quota_erasures, \
                                 term_months, provider, provider_session_id, status, paid_at) \
             VALUES ($1, $2, 'Trial', $3, 'USD', 'one_off', 'site', 1, 25, 1, $4, \
                     'trial:' || gen_random_uuid(), 'paid', now()) RETURNING id",
        )
        .bind(org)
        .bind(offering)
        .bind(price)
        .bind(provider)
    };
    // Free exactly when a trial; never a negative price or another provider.
    for (price, provider) in [(0, "fake"), (0, "stripe"), (1, "trial"), (-1, "trial")] {
        insert_order(price, provider)
            .fetch_one(pool)
            .await
            .expect_err(provider);
    }
    insert_order(0, "gift")
        .fetch_one(pool)
        .await
        .expect_err("gift");
    let order = insert_order(0, "trial").fetch_one(pool).await.unwrap();
    // Once per organization per trial offering.
    let e = insert_order(0, "trial").fetch_one(pool).await.unwrap_err();
    assert_eq!(
        e.as_database_error().and_then(|d| d.constraint()),
        Some("orders_trial_once_key")
    );

    // The entitlement accepts provider `trial`, nothing unknown.
    let insert_ent = |provider: &'static str| {
        sqlx::query(
            "INSERT INTO entitlements (organization_id, order_id, provider, term_start, \
                                       term_end, renews) \
             SELECT organization_id, id, $2, paid_at, paid_at + interval '1 month', false \
             FROM orders WHERE id = $1",
        )
        .bind(order)
        .bind(provider)
    };
    insert_ent("gift").execute(pool).await.expect_err("gift");
    insert_ent("trial").execute(pool).await.unwrap();
}

/// Migration 0022 (controlled-licensing story 3): an offering's credit
/// target reserve exists only with a counted quota and freezes at
/// publication; an existing offering keeps NULL.
#[tokio::test]
async fn an_offering_target_reserve_needs_a_quota_and_freezes_at_publication() {
    let t = TestDb::migrated().await;
    let pool = &t.pool;
    let insert = |quota: Option<i64>, reserve: Option<i64>| {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO offerings (name, price_minor, currency, billing_interval, scope, seats, \
                                    quota_erasures, term_months, credit_target_reserve) \
             VALUES ('Pool ' || gen_random_uuid(), 1000, 'USD', 'one_off', 'site', 1, $1, 12, \
                     $2) RETURNING id",
        )
        .bind(quota)
        .bind(reserve)
    };
    for (quota, reserve) in [
        (None, Some(50)),
        (Some(500), Some(0)),
        (Some(500), Some(1_000_000_001)),
    ] {
        let err = insert(quota, reserve).fetch_one(pool).await.unwrap_err();
        assert_eq!(
            err.as_database_error().and_then(|d| d.constraint()),
            Some("offerings_credit_target_reserve_check"),
            "{quota:?} {reserve:?}"
        );
    }
    insert(None, None).fetch_one(pool).await.unwrap();
    insert(Some(500), None).fetch_one(pool).await.unwrap();
    let id = insert(Some(500), Some(80)).fetch_one(pool).await.unwrap();
    // A draft may change it; a published offering may not.
    sqlx::query("UPDATE offerings SET credit_target_reserve = 90 WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE offerings SET status = 'published', published_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    let m = refused(
        pool,
        "UPDATE offerings SET credit_target_reserve = 100 WHERE id = $1",
        id,
    )
    .await;
    assert!(m.contains("commercial terms cannot change"), "{m}");
}

/// Migration 0022: a station's reported balance is never negative and comes
/// with its report time.
#[tokio::test]
async fn a_station_balance_is_never_negative_and_has_a_report_time() {
    let t = TestDb::migrated().await;
    let pool = &t.pool;
    for sql in [
        "ALTER TABLE stations VALIDATE CONSTRAINT stations_credit_balance_reported_check",
        "SELECT 1 FROM credit_grants",
        "SELECT 1 FROM credit_returns",
    ] {
        sqlx::query(sql).execute(pool).await.unwrap();
    }
    let checks: Vec<String> = sqlx::query_scalar(
        "SELECT conname::text FROM pg_constraint WHERE conrelid = 'stations'::regclass \
         AND contype = 'c' ORDER BY conname",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(
        checks.contains(&"stations_credit_balance_check".to_string()),
        "{checks:?}"
    );
    assert!(
        checks.contains(&"stations_credit_balance_reported_check".to_string()),
        "{checks:?}"
    );
}

/// Migration 0023 (controlled-licensing story 4): an offering's top-up
/// threshold exists only with a counted quota, lies between 1 and the
/// target reserve (NULL reading as 50) and freezes at publication.
#[tokio::test]
async fn an_offering_topup_threshold_needs_a_quota_and_freezes_at_publication() {
    let t = TestDb::migrated().await;
    let pool = &t.pool;
    let insert = |quota: Option<i64>, reserve: Option<i64>, threshold: Option<i64>| {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO offerings (name, price_minor, currency, billing_interval, scope, seats, \
                                    quota_erasures, term_months, credit_target_reserve, \
                                    credit_topup_threshold) \
             VALUES ('Pool ' || gen_random_uuid(), 1000, 'USD', 'one_off', 'site', 1, $1, 12, \
                     $2, $3) RETURNING id",
        )
        .bind(quota)
        .bind(reserve)
        .bind(threshold)
    };
    for (quota, reserve, threshold) in [
        (None, None, Some(5)),
        (Some(500), None, Some(0)),
        (Some(500), None, Some(51)),
        (Some(500), Some(20), Some(21)),
    ] {
        let err = insert(quota, reserve, threshold)
            .fetch_one(pool)
            .await
            .unwrap_err();
        assert_eq!(
            err.as_database_error().and_then(|d| d.constraint()),
            Some("offerings_credit_topup_threshold_check"),
            "{quota:?} {reserve:?} {threshold:?}"
        );
    }
    insert(Some(500), None, None).fetch_one(pool).await.unwrap();
    insert(Some(500), None, Some(50))
        .fetch_one(pool)
        .await
        .unwrap();
    let id = insert(Some(500), Some(20), Some(20))
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE offerings SET credit_topup_threshold = 15 WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE offerings SET status = 'published', published_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    let m = refused(
        pool,
        "UPDATE offerings SET credit_topup_threshold = 16 WHERE id = $1",
        id,
    )
    .await;
    assert!(m.contains("commercial terms cannot change"), "{m}");
}

/// Migration 0023: a top-up grant has no licence, a station's reported
/// spent count is never negative, and the mismatch flag is nullable.
#[tokio::test]
async fn a_topup_grant_needs_no_licence_and_spent_reports_are_never_negative() {
    let t = TestDb::migrated().await;
    let pool = &t.pool;
    let nullable: Vec<(String, String)> = sqlx::query_as(
        "SELECT table_name::text, column_name::text FROM information_schema.columns \
         WHERE ((table_name = 'credit_grants' AND column_name = 'licence_id') \
                OR (table_name = 'stations' AND column_name IN \
                    ('credit_spent_reported', 'credit_report_mismatch_at'))) \
           AND is_nullable = 'YES' ORDER BY 1, 2",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(nullable.len(), 3, "{nullable:?}");
    let checks: Vec<String> = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(oid) FROM pg_constraint \
         WHERE conrelid = 'stations'::regclass AND contype = 'c'",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(
        checks
            .iter()
            .any(|c| c.contains("credit_spent_reported >= 0")),
        "{checks:?}"
    );
}

/// Migration 0024 (controlled-licensing story 5): a renewal failure time is
/// nullable, and only a station licence may end at or before its issue (a
/// licence whose renewal grace is withdrawn after its paid term ended).
#[tokio::test]
async fn a_renewal_failure_is_nullable_and_only_station_licences_may_end_before_issue() {
    let t = TestDb::migrated().await;
    let (order, org) = paid_order(&t, "annual").await;
    let pool = &t.pool;
    let nullable: bool = sqlx::query_scalar(
        "SELECT is_nullable = 'YES' FROM information_schema.columns \
         WHERE table_name = 'entitlements' AND column_name = 'renewal_failed_at'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(nullable);
    let ent: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO entitlements (organization_id, order_id, provider, subscription_id, \
                                   term_start, term_end, renews) \
         VALUES ($1, $2, 'stripe', 'sub_1', now() - interval '13 months', \
                 now() - interval '1 month', true) RETURNING id",
    )
    .bind(org)
    .bind(order)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE entitlements SET renewal_failed_at = now() WHERE id = $1")
        .bind(ent)
        .execute(pool)
        .await
        .unwrap();
    let user: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (organization_id, email, name, password_hash, role) \
         VALUES ($1, 'o@example.test', 'O', 'x', 'owner') RETURNING id",
    )
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();
    let station: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO stations (organization_id, entitlement_id, instance_key_id, registered_by) \
         VALUES ($1, $2, 'ed25519:AAAAAAAAAAAAAAAAAAAAAA', $3) RETURNING id",
    )
    .bind(org)
    .bind(ent)
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap();
    let ended = |source: &'static str| {
        sqlx::query(
            "INSERT INTO licences (id, organization_id, station_id, entitlement_id, \
                                   instance_key_id, root_key_id, not_after, issued_at, document, \
                                   source, issue_reason) \
             VALUES (gen_random_uuid(), $1, $2, $3, 'ed25519:AAAAAAAAAAAAAAAAAAAAAA', 'r', \
                     now() - interval '1 month', now(), '{}', $4, $5)",
        )
        .bind(org)
        .bind((source == "station").then_some(station))
        .bind((source == "station").then_some(ent))
        .bind(source)
        .bind((source == "vendor").then_some("evaluation"))
    };
    ended("station").execute(pool).await.unwrap();
    let refused = ended("vendor").execute(pool).await.unwrap_err();
    assert!(
        refused.to_string().contains("licences_window_check"),
        "{refused}"
    );
}
