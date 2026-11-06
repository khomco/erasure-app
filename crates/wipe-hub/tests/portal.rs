//! Portal serving over real HTTP (hub-foundation story 4).
//!
//! No compose needed: the hub gets a lazy pool on a closed port and a portal
//! built into a tempdir.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use tokio::net::TcpListener;
use wipe_hub::{AppState, Args, Config};

const INDEX: &str = "<!doctype html><title>Wipestation portal</title><div id=root></div>";
const APP_JS: &str = "console.log('portal');";

/// A port with nothing listening on it.
async fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

fn built_portal(root: &Path) -> PathBuf {
    let dir = root.join("dist");
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), INDEX).unwrap();
    std::fs::write(dir.join("assets/app.js"), APP_JS).unwrap();
    dir
}

async fn spawn_hub(portal_dir: Option<PathBuf>) -> SocketAddr {
    let args = Args {
        addr: "127.0.0.1:0".parse().unwrap(),
        database_url: format!(
            "postgres://wipehub:wipehub@127.0.0.1:{}/wipehub",
            closed_port().await
        ),
        smtp_host: "127.0.0.1".into(),
        smtp_port: closed_port().await,
        mail_from: wipe_hub::config::DEFAULT_MAIL_FROM.into(),
        public_base_url: wipe_hub::config::DEFAULT_PUBLIC_BASE_URL.into(),
        payment_provider: wipe_hub::PaymentProvider::Fake,
        stripe_webhook_secret: None,
        vendor_root_seed: None,
        portal_dir: portal_dir.clone(),
    };
    let mut config = Config::from_args(args).unwrap();
    // `from_args` may pick up a built `apps/portal/dist` under the cwd when
    // the flag is absent; pin the case under test.
    config.portal_dir = portal_dir;
    let state = AppState::new(config).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(wipe_hub::serve_on(listener, state));
    addr
}

async fn get(addr: SocketAddr, path: &str) -> (u16, Option<String>, String) {
    let resp = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let code = resp.status().as_u16();
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string());
    (code, ctype, resp.text().await.unwrap())
}

#[tokio::test]
async fn index_is_served_at_root() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_hub(Some(built_portal(tmp.path()))).await;
    let (code, ctype, body) = get(addr, "/").await;
    assert_eq!(code, 200);
    assert!(ctype.unwrap().starts_with("text/html"));
    assert_eq!(body, INDEX);
}

#[tokio::test]
async fn existing_asset_is_served_as_javascript() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_hub(Some(built_portal(tmp.path()))).await;
    let (code, ctype, body) = get(addr, "/assets/app.js").await;
    assert_eq!(code, 200);
    let ctype = ctype.unwrap();
    assert!(ctype.contains("javascript"), "{ctype}");
    assert_eq!(body, APP_JS);
}

#[tokio::test]
async fn missing_asset_is_404_not_index() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_hub(Some(built_portal(tmp.path()))).await;
    let (code, _, body) = get(addr, "/assets/nope.js").await;
    assert_eq!(code, 404);
    assert_ne!(body, INDEX);
    assert!(!body.contains("<!doctype html>"), "{body}");
}

#[tokio::test]
async fn deep_link_gets_index_with_200() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_hub(Some(built_portal(tmp.path()))).await;
    for path in ["/account/orders", "/x/y", "/nope.html"] {
        let (code, _, body) = get(addr, path).await;
        assert_eq!(code, 200, "{path}");
        assert_eq!(body, INDEX, "{path}");
    }
}

#[tokio::test]
async fn api_health_is_unaffected_by_the_portal() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_hub(Some(built_portal(tmp.path()))).await;
    let resp = reqwest::get(format!("http://{addr}/api/health"))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 503);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body,
        serde_json::json!({"status": "degraded", "database": "down", "mail": "down"})
    );
}

async fn assert_unknown_api_is_json_404(addr: SocketAddr) {
    for path in [
        "/api",
        "/api/",
        "/api/nope",
        "/api/health/extra",
        "/api/a/b/c",
    ] {
        let resp = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
        assert_eq!(resp.status().as_u16(), 404, "{path}");
        let body: serde_json::Value = resp.json().await.unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(body, serde_json::json!({"error": "not found"}), "{path}");
    }
}

#[tokio::test]
async fn unknown_api_is_json_404_with_portal() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_hub(Some(built_portal(tmp.path()))).await;
    assert_unknown_api_is_json_404(addr).await;
}

#[tokio::test]
async fn unknown_api_is_json_404_without_portal() {
    let addr = spawn_hub(None).await;
    assert_unknown_api_is_json_404(addr).await;
}

#[tokio::test]
async fn no_portal_is_404_text_naming_the_build() {
    let addr = spawn_hub(None).await;
    for path in ["/", "/account/orders", "/assets/app.js"] {
        let (code, ctype, body) = get(addr, path).await;
        assert_eq!(code, 404, "{path}");
        assert!(ctype.unwrap().starts_with("text/plain"), "{path}");
        assert!(body.contains("cd apps/portal && pnpm build"), "{body}");
    }
}
