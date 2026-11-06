//! The `wipe-hub` binary itself: exit codes and start-up warning logging.
//! Every `WIPE_HUB_*` var is removed from the child so a developer's
//! environment cannot change the outcome.

use std::io::{BufRead, BufReader};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn hub() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wipe-hub"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("WIPE_HUB_") {
            cmd.env_remove(&key);
        }
    }
    cmd.env("RUST_LOG", "info");
    cmd
}

fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn combined(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn stripe_flag_without_secret_refuses_to_start() {
    let out = hub()
        .args([
            "serve",
            "--addr",
            "127.0.0.1:0",
            "--payment-provider",
            "stripe",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        combined(&out).contains("WIPE_HUB_STRIPE_WEBHOOK_SECRET"),
        "{}",
        combined(&out)
    );
}

#[test]
fn stripe_env_without_secret_refuses_to_start() {
    let out = hub()
        .args(["serve", "--addr", "127.0.0.1:0"])
        .env("WIPE_HUB_PAYMENT_PROVIDER", "stripe")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        combined(&out).contains("WIPE_HUB_STRIPE_WEBHOOK_SECRET"),
        "{}",
        combined(&out)
    );
}

#[test]
fn unknown_payment_provider_exits_non_zero() {
    let out = hub()
        .args([
            "serve",
            "--addr",
            "127.0.0.1:0",
            "--payment-provider",
            "paypal",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", combined(&out));
}

#[test]
fn file_held_vendor_root_is_logged_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let seed = dir.path().join("root.seed");
    std::fs::write(&seed, [9u8; 32]).unwrap();

    let mut child = hub()
        .args(["serve", "--addr", "127.0.0.1:0", "--vendor-root-seed"])
        .arg(&seed)
        // A closed port, so the test touches no database: the warning is
        // logged before `serve` tries to migrate.
        .arg("--database-url")
        .arg(format!(
            "postgres://wipehub:wipehub@127.0.0.1:{}/wipehub",
            closed_port()
        ))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            let found = line.contains("file-held");
            if tx.send(line).is_err() || found {
                return;
            }
        }
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    let found = loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) if line.contains("file-held") => break true,
            Ok(line) => seen.push(line),
            Err(_) => break false,
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        found,
        "no file-held warning; stdout was:\n{}",
        seen.join("\n")
    );
}
