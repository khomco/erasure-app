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

/// Run `serve` in `cwd` against a closed database port and collect stdout
/// until the process exits (start-up warnings are all logged before the
/// failing migration ends it) or a deadline passes.
fn startup_stdout_in(cwd: &std::path::Path) -> String {
    let mut child = hub()
        .args(["serve", "--addr", "127.0.0.1:0", "--database-url"])
        .arg(format!(
            "postgres://wipehub:wipehub@127.0.0.1:{}/wipehub",
            closed_port()
        ))
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if tx.send(line).is_err() {
                return;
            }
        }
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => seen.push(line),
            // Disconnected: stdout closed, the process exited.
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    seen.join("\n")
}

#[test]
fn built_portal_in_cwd_is_used_without_warning() {
    let dir = tempfile::tempdir().unwrap();
    let dist = dir.path().join("apps/portal/dist");
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::write(dist.join("index.html"), "<!doctype html>").unwrap();

    let out = startup_stdout_in(dir.path());
    assert!(
        out.contains("no vendor root seed configured"),
        "start-up warnings were not logged; stdout was:\n{out}"
    );
    assert!(
        !out.contains("portal is not served"),
        "portal warning logged despite a built cwd default:\n{out}"
    );
}

#[test]
fn unbuilt_portal_in_cwd_is_warned() {
    let dir = tempfile::tempdir().unwrap();
    let out = startup_stdout_in(dir.path());
    assert!(
        out.contains("portal is not served"),
        "no portal warning; stdout was:\n{out}"
    );
    assert!(out.contains("cd apps/portal && pnpm build"), "{out}");
}

#[test]
fn bad_portal_dir_refuses_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    let out = hub()
        .args(["serve", "--addr", "127.0.0.1:0", "--database-url"])
        .arg(format!(
            "postgres://wipehub:wipehub@127.0.0.1:{}/wipehub",
            closed_port()
        ))
        .arg("--portal-dir")
        .arg(&missing)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", combined(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&missing.display().to_string()),
        "stderr does not name the path:\n{stderr}"
    );
    assert!(stderr.contains("pnpm build"), "{stderr}");
}

#[test]
fn admin_create_without_an_email_exits_non_zero() {
    let out = hub().args(["admin", "create"]).output().unwrap();
    assert!(!out.status.success());
    assert!(combined(&out).contains("--email"), "{}", combined(&out));
}
