//! ADR-0007 §1 dependency rule, checked against `cargo metadata`:
//! `wipe-hub` depends on no sibling crate but common/cert/license, and no
//! other workspace crate depends on `wipe-hub` (in any dependency kind).

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::Value;

const ALLOWED_SIBLINGS: [&str; 3] = ["wipe-common", "wipe-cert", "wipe-license"];

fn metadata() -> Value {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("running cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn workspace_packages(meta: &Value) -> Vec<&Value> {
    let members: BTreeSet<&str> = meta["workspace_members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    meta["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| members.contains(p["id"].as_str().unwrap()))
        .collect()
}

fn dep_names(pkg: &Value) -> Vec<&str> {
    pkg["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect()
}

#[test]
fn no_workspace_crate_depends_on_wipe_hub() {
    let meta = metadata();
    let pkgs = workspace_packages(&meta);
    assert!(pkgs.len() > 1, "expected the whole workspace");
    for pkg in pkgs {
        let name = pkg["name"].as_str().unwrap();
        if name == "wipe-hub" {
            continue;
        }
        assert!(
            !dep_names(pkg).contains(&"wipe-hub"),
            "{name} depends on wipe-hub; ADR-0007 forbids any crate depending on the hub"
        );
    }
}

#[test]
fn wipe_hub_sibling_deps_are_common_cert_license_only() {
    let meta = metadata();
    let pkgs = workspace_packages(&meta);
    let siblings: BTreeSet<&str> = pkgs.iter().map(|p| p["name"].as_str().unwrap()).collect();
    let hub = pkgs
        .iter()
        .find(|p| p["name"] == "wipe-hub")
        .expect("wipe-hub is a workspace member");
    for dep in dep_names(hub) {
        if siblings.contains(dep) {
            assert!(
                ALLOWED_SIBLINGS.contains(&dep),
                "wipe-hub depends on sibling {dep}; only {ALLOWED_SIBLINGS:?} are allowed"
            );
        }
    }
}
