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
