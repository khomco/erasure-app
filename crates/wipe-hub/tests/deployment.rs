//! `GET /api/deployment` over real HTTP (commerce-end-to-end story 2): what
//! kind of hub this is, with no session. The portal's development banner
//! renders this body.
//!
//! Needs `docker compose up -d`; fails, never skips, when it is down.

mod support;

use reqwest::StatusCode;
use serde_json::{json, Value};
use support::hub::{spawn_hub, spawn_hub_with_root, Hub};
use support::stripe_mock::{spawn_stripe_hub, StripeMock};

/// The status and body of an anonymous `GET /api/deployment`.
async fn deployment(hub: &Hub) -> (StatusCode, Value) {
    let resp = hub.get("/api/deployment", None).await;
    let code = resp.status();
    (code, resp.json().await.unwrap())
}

#[tokio::test]
async fn default_hub_is_development_for_the_fake_provider() {
    let hub = spawn_hub().await;
    assert_eq!(
        deployment(&hub).await,
        (
            StatusCode::OK,
            json!({"development": true, "payment_provider": "fake", "vendor_root": "none",
                   "reasons": ["fake_payment_provider"]})
        )
    );
}

#[tokio::test]
async fn file_held_root_adds_a_reason() {
    let (hub, _root) = spawn_hub_with_root().await;
    assert_eq!(
        deployment(&hub).await,
        (
            StatusCode::OK,
            json!({"development": true, "payment_provider": "fake", "vendor_root": "file",
                   "reasons": ["fake_payment_provider", "file_vendor_root"]})
        )
    );
}

#[tokio::test]
async fn stripe_without_a_root_is_not_development() {
    let mock = StripeMock::start().await;
    let hub = spawn_stripe_hub(&mock).await;
    assert_eq!(
        deployment(&hub).await,
        (
            StatusCode::OK,
            json!({"development": false, "payment_provider": "stripe", "vendor_root": "none",
                   "reasons": []})
        )
    );
}
