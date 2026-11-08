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
