//! The append-only audit log (ADR-0007 §6). Needs `docker compose up -d`;
//! fails, never skips, when Postgres is down.

mod support;

use serde_json::{json, Value};
use support::TestDb;
use wipe_hub::audit::{self, AuditEntry, AuditError};

fn entry() -> AuditEntry<'static> {
    AuditEntry {
        actor: "vendor_admin:alice",
        action: "licence.issue",
        subject: "licence:42",
        detail: json!({"seats": 3}),
    }
}

async fn row_count(t: &TestDb) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&t.pool)
        .await
        .expect("count audit_log")
}

/// Every row, oldest first, as (id, actor, action, subject, detail).
async fn rows(t: &TestDb) -> Vec<(i64, String, String, String, Value)> {
    sqlx::query_as("SELECT id, actor, action, subject, detail FROM audit_log ORDER BY id")
        .fetch_all(&t.pool)
        .await
        .expect("read audit_log")
}

/// Two committed rows to try to tamper with.
async fn seeded() -> TestDb {
    let t = TestDb::migrated().await;
    audit::record(&t.pool, &entry()).await.unwrap();
    audit::record(&t.pool, &entry()).await.unwrap();
    t
}

fn assert_append_only(err: sqlx::Error) {
    let msg = err.to_string();
    assert!(msg.contains("audit_log is append-only"), "{msg}");
}

#[tokio::test]
async fn record_writes_the_entry_with_a_database_timestamp() {
    let t = TestDb::migrated().await;
    let id = audit::record(&t.pool, &entry()).await.unwrap();

    let (actor, action, subject, detail, skew): (String, String, String, Value, f64) =
        sqlx::query_as(
            "SELECT actor, action, subject, detail, \
                    abs(extract(epoch FROM now() - occurred_at))::float8 \
             FROM audit_log WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(actor, "vendor_admin:alice");
    assert_eq!(action, "licence.issue");
    assert_eq!(subject, "licence:42");
    assert_eq!(detail, json!({"seats": 3}));
    assert!(skew < 5.0, "occurred_at is {skew}s from now()");
}

#[tokio::test]
async fn ids_increase() {
    let t = TestDb::migrated().await;
    let first = audit::record(&t.pool, &entry()).await.unwrap();
    let second = audit::record(&t.pool, &entry()).await.unwrap();
    assert!(second > first, "{second} <= {first}");
}

#[tokio::test]
async fn record_works_on_a_single_connection() {
    let t = TestDb::migrated().await;
    let mut conn = t.pool.acquire().await.unwrap();
    audit::record(&mut *conn, &entry()).await.unwrap();
    drop(conn);
    assert_eq!(row_count(&t).await, 1);
}

#[tokio::test]
async fn a_rolled_back_transaction_leaves_no_row() {
    let t = TestDb::migrated().await;
    let mut tx = t.pool.begin().await.unwrap();
    audit::record(&mut *tx, &entry()).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(row_count(&t).await, 0);
}

#[tokio::test]
async fn a_committed_transaction_keeps_its_row() {
    let t = TestDb::migrated().await;
    let mut tx = t.pool.begin().await.unwrap();
    audit::record(&mut *tx, &entry()).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(row_count(&t).await, 1);
}

#[tokio::test]
async fn blank_fields_are_rejected_and_write_nothing() {
    let t = TestDb::migrated().await;
    let cases = [
        (
            AuditEntry {
                actor: "",
                ..entry()
            },
            "actor",
        ),
        (
            AuditEntry {
                action: "  ",
                ..entry()
            },
            "action",
        ),
        (
            AuditEntry {
                subject: "\t\n",
                ..entry()
            },
            "subject",
        ),
    ];
    for (e, field) in cases {
        match audit::record(&t.pool, &e).await {
            Err(AuditError::Blank(f)) => assert_eq!(f, field),
            other => panic!("expected Blank({field}), got {other:?}"),
        }
    }
    assert_eq!(row_count(&t).await, 0);
}

#[tokio::test]
async fn non_object_detail_is_rejected_and_writes_nothing() {
    let t = TestDb::migrated().await;
    for detail in [json!([1]), json!("x")] {
        let e = AuditEntry { detail, ..entry() };
        assert!(
            matches!(
                audit::record(&t.pool, &e).await,
                Err(AuditError::DetailNotObject)
            ),
            "non-object detail accepted"
        );
    }
    assert_eq!(row_count(&t).await, 0);
}

#[tokio::test]
async fn database_checks_back_up_the_validation() {
    let t = TestDb::migrated().await;
    for sql in [
        "INSERT INTO audit_log (actor, action, subject) VALUES ('  ', 'a', 's')",
        "INSERT INTO audit_log (actor, action, subject) VALUES ('x', '', 's')",
        "INSERT INTO audit_log (actor, action, subject) VALUES ('x', 'a', ' ')",
        r"INSERT INTO audit_log (actor, action, subject) VALUES (E'\t', 'a', 's')",
        r"INSERT INTO audit_log (actor, action, subject) VALUES ('x', E'\n', 's')",
        r"INSERT INTO audit_log (actor, action, subject) VALUES ('x', 'a', E'\t\n')",
        "INSERT INTO audit_log (actor, action, subject, detail) VALUES ('x', 'a', 's', '[1]')",
    ] {
        let err = sqlx::query(sql)
            .execute(&t.pool)
            .await
            .expect_err(sql)
            .to_string();
        assert!(err.contains("check constraint"), "{sql}: {err}");
    }
    assert_eq!(row_count(&t).await, 0);
}

#[tokio::test]
async fn update_is_rejected() {
    let t = seeded().await;
    let before = rows(&t).await;
    let err = sqlx::query("UPDATE audit_log SET actor = 'x'")
        .execute(&t.pool)
        .await
        .expect_err("UPDATE succeeded");
    assert_append_only(err);
    assert_eq!(rows(&t).await, before);
}

#[tokio::test]
async fn delete_is_rejected() {
    let t = seeded().await;
    let err = sqlx::query("DELETE FROM audit_log")
        .execute(&t.pool)
        .await
        .expect_err("DELETE succeeded");
    assert_append_only(err);
    assert_eq!(row_count(&t).await, 2);
}

#[tokio::test]
async fn truncate_is_rejected() {
    let t = seeded().await;
    let err = sqlx::query("TRUNCATE audit_log")
        .execute(&t.pool)
        .await
        .expect_err("TRUNCATE succeeded");
    assert_append_only(err);
    assert_eq!(row_count(&t).await, 2);
}
