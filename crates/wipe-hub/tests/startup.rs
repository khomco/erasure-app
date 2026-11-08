//! The binary applies migrations before it listens, and refuses to start when
//! Postgres is unreachable. Needs `docker compose up -d` for the first test.
//! Every `WIPE_HUB_*` var is removed from the child.

mod support;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use support::TestDb;
use wipe_hub::db;

fn hub() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wipe-hub"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("WIPE_HUB_") {
            cmd.env_remove(&key);
        }
    }
    cmd.env("RUST_LOG", "info");
    // Never `.wipe-hub/artifacts` under the crate: one shared scratch
    // directory (these runs store nothing in it).
    cmd.env(
        "WIPE_HUB_ARTIFACT_DIR",
        std::env::temp_dir().join("wipe-hub-test-artifacts"),
    );
    cmd
}

/// Drop ANSI colour escapes so log fields read as `key=value`.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// Start the hub against `database_url`, wait (10 s limit) for it to log
/// that it is listening, kill it, and return the stdout lines seen.
fn start_until_listening(database_url: &str) -> Vec<String> {
    let mut child = hub()
        .args(["serve", "--addr", "127.0.0.1:0", "--smtp-port"])
        .arg(closed_port().to_string())
        .arg("--database-url")
        .arg(database_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if tx.send(strip_ansi(&line)).is_err() {
                return;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    let listening = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => {
                let done = line.contains("wipe-hub listening");
                seen.push(line);
                if done {
                    break true;
                }
            }
            Err(_) => break false,
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        listening,
        "hub never logged that it is listening; stdout was:\n{}",
        seen.join("\n")
    );
    seen
}

#[tokio::test]
async fn migrates_a_fresh_database_before_listening_and_restarts_cleanly() {
    let t = TestDb::empty().await;

    let first = start_until_listening(&t.url);
    let migrated_at = first
        .iter()
        .position(|l| l.contains("database migrations up to date"))
        .unwrap_or_else(|| panic!("no migration log line:\n{}", first.join("\n")));
    assert!(migrated_at < first.len() - 1, "{first:?}");
    assert!(
        first[migrated_at].contains(&format!("applied={}", db::embedded_count())),
        "{}",
        first[migrated_at]
    );
    assert!(
        first[migrated_at].contains(&format!("database={}", db::redacted(&t.url))),
        "{}",
        first[migrated_at]
    );
    assert!(
        !first[migrated_at].contains(":wipehub@"),
        "{}",
        first[migrated_at]
    );

    let rows: Vec<(i64, bool)> =
        sqlx::query_as("SELECT version, success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    let embedded: Vec<i64> = db::MIGRATOR
        .iter()
        .filter(|m| m.migration_type.is_up_migration())
        .map(|m| m.version)
        .collect();
    assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), embedded);
    assert!(rows.iter().all(|r| r.1), "{rows:?}");

    // Second start against the same database: nothing to apply.
    let second = start_until_listening(&t.url);
    assert!(
        second
            .iter()
            .any(|l| l.contains("database migrations up to date") && l.contains("applied=0")),
        "{}",
        second.join("\n")
    );
}

#[test]
fn unreachable_database_exits_fast_without_leaking_the_password() {
    let password = "Tr0ub4dor-and-3-horse";
    let port = closed_port();
    let url = format!("postgres://wipehub:{password}@127.0.0.1:{port}/wipehub");
    let started = Instant::now();
    let out = hub()
        .args(["serve", "--addr", "127.0.0.1:0", "--database-url", &url])
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{all}");
    assert!(elapsed < Duration::from_secs(8), "took {elapsed:?}");
    assert!(all.contains("run docker compose up -d"), "{all}");
    assert!(
        all.contains(&format!("wipehub@127.0.0.1:{port}/wipehub")),
        "{all}"
    );
    assert!(!all.contains(password), "password leaked:\n{all}");
    assert!(!all.contains("wipe-hub listening"), "{all}");
}

#[test]
fn silent_database_times_out_within_the_connect_bound() {
    // Accepts TCP but never speaks, so only the 5 s connect bound ends it.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = silent.local_addr().unwrap().port();
    let started = Instant::now();
    let out = hub()
        .args(["serve", "--addr", "127.0.0.1:0", "--database-url"])
        .arg(format!(
            "postgres://wipehub:wipehub@127.0.0.1:{port}/wipehub"
        ))
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    drop(silent);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(elapsed < Duration::from_secs(8), "took {elapsed:?}");
    assert!(stderr.contains("run docker compose up -d"), "{stderr}");
}
