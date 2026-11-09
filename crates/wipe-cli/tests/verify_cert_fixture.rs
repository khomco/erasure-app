//! The shipped `wipestation verify-cert`, given the published station public
//! key, still accepts the v2 certificate an earlier station signed
//! (drift-tests story 3), and both v2 fixtures pass through it. This is the surface a third-party verifier uses;
//! the keys are the literals published in
//! `crates/wipe-cert/tests/fixtures/README.md`.

use std::{path::PathBuf, process::Output};

const STATION_KEY_B64: &str = "GX9rI+FshTLGq8g4+s1ep4m+DHaykgM0A5v6iz02jWE";
const SUPERVISOR_KEY_B64: &str = "RQigeqlBcH8+stuUyIl6gLLBGXR2tt4hOsJz332GxP8";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../wipe-cert/tests/fixtures")
        .join(name)
}

/// Runs `wipestation verify-cert --public-key-b64 <key> <fixture>` with an
/// emptied environment.
fn verify_cert(key: &str, name: &str) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_wipestation"))
        .arg("verify-cert")
        .arg("--public-key-b64")
        .arg(key)
        .arg(fixture(name))
        .env_clear()
        .output()
        .expect("running wipestation verify-cert")
}

#[test]
fn v2_fixtures_verify_with_the_published_station_key() {
    for name in ["v2-erased.json", "v2-destroyed-cosigned.json"] {
        let out = verify_cert(STATION_KEY_B64, name);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{name}: verify-cert exited {}\nstdout: {stdout}\nstderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.contains("format: v2"), "{name}: {stdout}");
    }
}

#[test]
fn v2_erased_fixture_fails_with_the_supervisor_key() {
    let out = verify_cert(SUPERVISOR_KEY_B64, "v2-erased.json");
    assert!(
        !out.status.success(),
        "verify-cert accepted the wrong key\nstdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    // Refused for the signature key, not a missing file or bad arguments.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no trusted key"), "{stderr}");
}
