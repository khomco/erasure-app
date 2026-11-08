//! A local mock of the Stripe API (offerings-and-payments story 3): an axum
//! server on `127.0.0.1:0` that records every request's method, path,
//! headers and form, and answers with scripted replies, or by default:
//!
//! - `POST /v1/checkout/sessions` → `{id: "cs_test_<uuid>", url:
//!   "https://checkout.stripe.com/c/pay/<id>"}`;
//! - `POST /v1/billing_portal/sessions` → `{url: BILLING_URL}`;
//! - anything else → 404.
//!
//! [`StripeMock::hold_next`] holds the next request until the test releases
//! it, so a test can act while the hub waits on Stripe.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::hub::{spawn_hub_args, Hub};

pub const STRIPE_SECRET_KEY: &str = "sk_test_wipehub_mock_key";
pub const STRIPE_WEBHOOK_SECRET: &str = "whsec_wipehub_mock_secret";
pub const BILLING_URL: &str = "https://billing.stripe.com/p/session/test_wipehub";

/// One request the mock received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: HeaderMap,
    pub form: Vec<(String, String)>,
}

impl Recorded {
    /// The single value of form field `name`, if present.
    pub fn field(&self, name: &str) -> Option<&str> {
        let mut found = self.form.iter().filter(|(k, _)| k == name);
        let first = found.next().map(|(_, v)| v.as_str());
        assert!(found.next().is_none(), "form field {name} repeated");
        first
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

struct Hold {
    seen: oneshot::Sender<Recorded>,
    release: oneshot::Receiver<()>,
}

#[derive(Default)]
struct Inner {
    requests: Vec<Recorded>,
    replies: VecDeque<(u16, Value)>,
    hold: Option<Hold>,
}

#[derive(Clone)]
pub struct StripeMock {
    pub addr: SocketAddr,
    inner: Arc<Mutex<Inner>>,
}

impl StripeMock {
    pub async fn start() -> StripeMock {
        let inner = Arc::new(Mutex::new(Inner::default()));
        let app = Router::new().fallback(handle).with_state(inner.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        StripeMock { addr, inner }
    }

    /// The base URL to pass as `--stripe-api-base`.
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Answer the next request with `status` and `body` instead of the
    /// default.
    pub fn reply_next(&self, status: u16, body: Value) {
        self.inner.lock().unwrap().replies.push_back((status, body));
    }

    /// Every request so far, oldest first.
    pub fn requests(&self) -> Vec<Recorded> {
        self.inner.lock().unwrap().requests.clone()
    }

    /// Hold the next request: the first receiver gets it when it arrives,
    /// and the mock answers only once the sender fires (or is dropped).
    pub fn hold_next(&self) -> (oneshot::Receiver<Recorded>, oneshot::Sender<()>) {
        let (seen_tx, seen_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        self.inner.lock().unwrap().hold = Some(Hold {
            seen: seen_tx,
            release: release_rx,
        });
        (seen_rx, release_tx)
    }
}

async fn handle(
    State(inner): State<Arc<Mutex<Inner>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let recorded = Recorded {
        method: method.to_string(),
        path: uri.path().to_string(),
        headers,
        form: url::form_urlencoded::parse(&body).into_owned().collect(),
    };
    let (scripted, hold) = {
        let mut inner = inner.lock().unwrap();
        inner.requests.push(recorded.clone());
        (inner.replies.pop_front(), inner.hold.take())
    };
    if let Some(hold) = hold {
        let _ = hold.seen.send(recorded.clone());
        let _ = hold.release.await;
    }
    let (status, body) = match scripted {
        Some(reply) => reply,
        None => match (recorded.method.as_str(), recorded.path.as_str()) {
            ("POST", "/v1/checkout/sessions") => {
                let id = format!("cs_test_{}", Uuid::new_v4().simple());
                let url = format!("https://checkout.stripe.com/c/pay/{id}");
                (
                    200,
                    json!({"id": id, "object": "checkout.session", "url": url}),
                )
            }
            ("POST", "/v1/billing_portal/sessions") => (
                200,
                json!({"id": "bps_test_1", "object": "billing_portal.session", "url": BILLING_URL}),
            ),
            _ => (
                404,
                json!({"error": {"type": "invalid_request_error", "message": "unknown path"}}),
            ),
        },
    };
    (StatusCode::from_u16(status).unwrap(), Json(body)).into_response()
}

/// A hub on the Stripe provider, pointed at `mock`.
pub async fn spawn_stripe_hub(mock: &StripeMock) -> Hub {
    let base = mock.base();
    spawn_hub_args(move |a| {
        a.payment_provider = wipe_hub::PaymentProvider::Stripe;
        a.stripe_secret_key = Some(STRIPE_SECRET_KEY.into());
        a.stripe_webhook_secret = Some(STRIPE_WEBHOOK_SECRET.into());
        a.stripe_api_base = base;
    })
    .await
}
