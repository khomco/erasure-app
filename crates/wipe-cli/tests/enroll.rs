//! `wipestation enroll` refuses what it can before contacting the hub
//! (station-enrollment story 3). The full flow against a real hub is
//! `scripts/commerce-demo.sh`; the client is covered by
//! `crates/wipe-server/tests/hub_client.rs`.

use std::{
    io::Read,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};

/// A vendor root public key (any 32-byte Ed25519 point).
const ROOT_B64: &str = "GX9rI+FshTLGq8g4+s1ep4m+DHaykgM0A5v6iz02jWE";

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wipestation-enroll-cli-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `wipestation enroll <args>` with an emptied environment whose
/// configuration directory is `config`.
fn enroll(config: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wipestation"))
        .arg("enroll")
        .args(args)
        .env_clear()
        .env("WIPESTATION_CONFIG_DIR", config)
        .env("HOME", config)
        .output()
        .expect("running wipestation enroll")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A local "hub" that records whether anything connected.
struct Witness(TcpListener);

impl Witness {
    fn new() -> Self {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        Self(l)
    }
    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.0.local_addr().unwrap().port())
    }
    /// The first bytes of a connection, if one arrives within `wait`.
    fn first_request(&self, wait: Duration) -> Option<String> {
        let until = Instant::now() + wait;
        while Instant::now() < until {
            if let Ok((mut sock, _)) = self.0.accept() {
                sock.set_nonblocking(false).unwrap();
                sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut buf = [0u8; 512];
                let n = sock.read(&mut buf).unwrap_or(0);
                return Some(String::from_utf8_lossy(&buf[..n]).into_owned());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

fn key_file(dir: &Path) -> PathBuf {
    let path = dir.join("station.key");
    std::fs::write(&path, [7u8; 32]).unwrap();
    path
}

#[test]
fn an_http_hub_off_loopback_is_refused_with_the_rule() {
    let dir = tmpdir("http-hub");
    let key = key_file(&dir);
    let out = enroll(
        &dir,
        &[
            "--hub",
            "http://hub.example",
            "--key-path",
            key.to_str().unwrap(),
            "--vendor-root-b64",
            ROOT_B64,
        ],
    );
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(
        err.contains("http is allowed only on a loopback host"),
        "{err}"
    );
    assert!(err.contains("https origin"), "{err}");
    assert!(!dir.join("enrolled-licence.json").exists());
}

#[test]
fn no_vendor_root_is_refused_before_any_request() {
    let dir = tmpdir("no-root");
    let key = key_file(&dir);
    let hub = Witness::new();
    let out = enroll(
        &dir,
        &["--hub", &hub.origin(), "--key-path", key.to_str().unwrap()],
    );
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("at least one --vendor-root-b64"), "{err}");
    assert!(hub.first_request(Duration::from_millis(200)).is_none());
}

#[test]
fn a_missing_key_file_is_refused_before_any_request_and_not_created() {
    let dir = tmpdir("no-key");
    let key = dir.join("missing.key");
    let hub = Witness::new();
    let out = enroll(
        &dir,
        &[
            "--hub",
            &hub.origin(),
            "--key-path",
            key.to_str().unwrap(),
            "--vendor-root-b64",
            ROOT_B64,
        ],
    );
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("does not exist"), "{err}");
    assert!(err.contains("never creates one"), "{err}");
    assert!(!key.exists(), "enroll created a key");
    assert!(hub.first_request(Duration::from_millis(200)).is_none());
}

#[test]
fn no_hub_given_or_saved_is_refused() {
    let dir = tmpdir("no-hub");
    let key = key_file(&dir);
    let out = enroll(
        &dir,
        &[
            "--key-path",
            key.to_str().unwrap(),
            "--vendor-root-b64",
            ROOT_B64,
        ],
    );
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("pass --hub"), "{err}");
    assert!(err.contains("station-settings.json"), "{err}");
}

#[test]
fn the_saved_hub_address_is_used_when_no_hub_is_given() {
    let dir = tmpdir("saved-hub");
    let key = key_file(&dir);
    let hub = Witness::new();
    std::fs::write(
        dir.join("station-settings.json"),
        format!(r#"{{"hub_origin":"{}"}}"#, hub.origin()),
    )
    .unwrap();
    let child = std::thread::spawn({
        let dir = dir.clone();
        move || {
            enroll(
                &dir,
                &[
                    "--key-path",
                    key.to_str().unwrap(),
                    "--vendor-root-b64",
                    ROOT_B64,
                ],
            )
        }
    });
    let request = hub
        .first_request(Duration::from_secs(20))
        .expect("enroll contacted the saved hub");
    assert!(
        request.starts_with("POST /api/enrollments HTTP/1.1"),
        "{request}"
    );
    // The witness closes the connection: enroll fails and names the hub.
    let out = child.join().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains(&hub.origin()), "{}", stderr(&out));
    assert!(!dir.join("enrolled-licence.json").exists());
}

/// A one-shot "hub" on a std listener: answers `POST /api/enrollments` with
/// codes (interval 1 s), then every poll with an approval carrying `licence`.
fn approving_hub(licence: serde_json::Value) -> String {
    use std::io::{BufRead, BufReader, Write};
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    std::thread::spawn(move || {
        for sock in l.incoming() {
            let Ok(mut sock) = sock else { return };
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; len];
            std::io::Read::read_exact(&mut reader, &mut body).unwrap();
            let answer = if first.starts_with("POST /api/enrollments/poll ") {
                serde_json::json!({"station": {"station_id": "bench-9"}, "licence": licence})
            } else {
                serde_json::json!({
                    "device_code": "dc", "user_code": "BCDF-GHJK",
                    "verification_uri": "http://127.0.0.1/activate",
                    "verification_uri_complete": "http://127.0.0.1/activate?code=BCDF-GHJK",
                    "expires_in": 600, "interval": 1
                })
            };
            let text = answer.to_string();
            write!(
                sock,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                text.len()
            )
            .unwrap();
        }
    });
    origin
}

#[test]
fn with_no_station_running_the_verified_licence_is_saved_and_the_serve_command_printed() {
    let dir = tmpdir("no-serve");
    let key_path = key_file(&dir);
    let root = wipe_license::VendorRoot::generate();
    let licence = licence_for_key_file(&root);
    let hub = approving_hub(licence);
    // A closed port: nothing is serving there.
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let station = format!("http://127.0.0.1:{closed}");
    let out = enroll(
        &dir,
        &[
            "--hub",
            &hub,
            "--key-path",
            key_path.to_str().unwrap(),
            "--vendor-root-b64",
            &root.verifying_key().to_base64(),
            "--station",
            &station,
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", stderr(&out));
    assert!(stdout.contains("user code: BCDF-GHJK"), "{stdout}");
    let saved = dir.join("enrolled-licence.json");
    let body: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&saved).unwrap()).unwrap();
    assert_eq!(body["body"]["license_id"], "lic-cli");
    assert!(stdout.contains("No station answered"), "{stdout}");
    assert!(
        stdout.contains(&format!("wipestation serve --license {}", saved.display())),
        "{stdout}"
    );
    let settings = std::fs::read_to_string(dir.join("station-settings.json")).unwrap();
    assert!(settings.contains(&hub), "{settings}");
}

/// A licence from `root` for the key `key_file` writes (seed `[7; 32]`).
fn licence_for_key_file(root: &wipe_license::VendorRoot) -> serde_json::Value {
    let key = wipe_cert::SigningKey::from_seed([7u8; 32]);
    let now = time::OffsetDateTime::now_utc();
    let ent = wipe_license::Entitlements {
        customer_id: "org-1".into(),
        customer_name: "Acme Recycling".into(),
        quota: wipe_license::Quota::Unlimited,
        scope: wipe_license::Scope::Site {
            site_id: "s".into(),
        },
        not_before: now - time::Duration::days(1),
        not_after: now + time::Duration::days(365),
        features: vec![],
        allowed_methods: wipe_license::AllowedMethods::All,
        machine_binding: None,
    };
    let licence = root
        .issue("lic-cli", key.public_key_id(), ent, now)
        .unwrap();
    serde_json::to_value(licence).unwrap()
}

/// A one-shot "station" answering any request with 409 and `error`.
fn refusing_station(error: &'static str) -> String {
    use std::io::{BufRead, BufReader, Write};
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    std::thread::spawn(move || {
        for sock in l.incoming() {
            let Ok(mut sock) = sock else { return };
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; len];
            std::io::Read::read_exact(&mut reader, &mut body).unwrap();
            let text = serde_json::json!({ "error": error }).to_string();
            write!(
                sock,
                "HTTP/1.1 409 Conflict\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                text.len()
            )
            .unwrap();
        }
    });
    base
}

#[test]
fn a_station_refusing_the_install_fails_with_its_error_and_the_saved_path() {
    let dir = tmpdir("station-refuses");
    let key_path = key_file(&dir);
    let root = wipe_license::VendorRoot::generate();
    let hub = approving_hub(licence_for_key_file(&root));
    let station = refusing_station("no vendor root configured");
    let out = enroll(
        &dir,
        &[
            "--hub",
            &hub,
            "--key-path",
            key_path.to_str().unwrap(),
            "--vendor-root-b64",
            &root.verifying_key().to_base64(),
            "--station",
            &station,
        ],
    );
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("refused the licence"), "{err}");
    assert!(err.contains("no vendor root configured"), "{err}");
    let saved = dir.join("enrolled-licence.json");
    assert!(saved.exists());
    assert!(err.contains(&saved.display().to_string()), "{err}");
}

#[test]
fn an_untrusted_licence_is_neither_saved_nor_installed() {
    let dir = tmpdir("untrusted");
    let key_path = key_file(&dir);
    let trusted = wipe_license::VendorRoot::generate();
    let other = wipe_license::VendorRoot::generate();
    let hub = approving_hub(licence_for_key_file(&other));
    let station = Witness::new();
    let out = enroll(
        &dir,
        &[
            "--hub",
            &hub,
            "--key-path",
            key_path.to_str().unwrap(),
            "--vendor-root-b64",
            &trusted.verifying_key().to_base64(),
            "--station",
            &station.origin(),
        ],
    );
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("does not trust"), "{err}");
    assert!(!dir.join("enrolled-licence.json").exists());
    assert!(station.first_request(Duration::from_millis(300)).is_none());
}
