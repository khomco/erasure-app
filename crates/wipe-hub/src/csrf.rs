//! Double-submit CSRF protection (ADR-0007 §3, portal-accounts story 2).
//!
//! Every `/api/*` request whose method is not GET, HEAD or OPTIONS, including
//! unknown `/api` paths, must carry an `X-CSRF-Token` header equal to the
//! `wipehub_csrf` cookie, and the value must be a well-formed token (32
//! OS-RNG bytes, base64url without padding: 43 characters). [`require`]
//! answers anything else with 403 [`MSG_BAD_CSRF`] before any handler runs.
//!
//! One rule covers anonymous and signed-in routes alike, so sign-in and
//! sign-up (login CSRF) are protected too. A cross-site page cannot set the
//! custom header: the hub sends CORS headers only to the marketing site's
//! configured origin, and only on this token endpoint and `POST /api/leads`
//! ([`crate::cors`]); that origin can read a token and post a sales lead, but
//! it is the vendor's own site. Subdomain cookie-tossing is the known
//! weakness, accepted until hosting is decided (CONTEXT §10).
//!
//! Three routes are exempt, all called by machines that have no cookie and
//! carry their own credential: `POST /api/payments/webhook`, called by the
//! payment provider, whose signature over the raw body is its credential and
//! which answers only `{"received":true}`; and a station's enrollment start
//! and poll, `POST /api/enrollments` and `POST /api/enrollments/poll`
//! (station-enrollment story 1), where the station's instance-key signature
//! is the credential. None of them sets or reads a cookie.
//!
//! `GET /api/auth/csrf` hands the token to the portal; it is the only place a
//! raw CSRF token is ever returned, and none is ever logged.

use axum::extract::{Request, State};
use axum::http::header::{CACHE_CONTROL, SET_COOKIE};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use serde_json::json;

use crate::accounts;
use crate::auth::{self, ApiError};
use crate::enrollment_routes::{ENROLLMENT_POLL_PATH, ENROLLMENT_START_PATH};
use crate::order_routes::PAYMENT_WEBHOOK_PATH;
use crate::AppState;

/// Name of the CSRF cookie.
pub const CSRF_COOKIE: &str = "wipehub_csrf";
/// Header that must echo the cookie on every mutating `/api` request.
pub const CSRF_HEADER: &str = "x-csrf-token";
pub const MSG_BAD_CSRF: &str = "missing or invalid CSRF token";

/// A fresh token: 32 bytes from the OS RNG, base64url without padding.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Whether `t` is exactly what [`generate_token`] produces in shape.
pub fn is_well_formed(t: &str) -> bool {
    t.len() == 43
        && URL_SAFE_NO_PAD
            .decode(t)
            .is_ok_and(|bytes| bytes.len() == 32)
}

/// Equality whose time depends only on the lengths, not the contents.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `Set-Cookie` value for the CSRF cookie; it lasts as long as a session.
pub fn csrf_cookie(token: &str, secure: bool) -> HeaderValue {
    auth::cookie_header(CSRF_COOKIE, token, accounts::SESSION_TTL.as_secs(), secure)
}

fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Whether a request needs no token: `POST` to the payment provider's
/// webhook (its credential is the provider's signature over the body) or to
/// a station's enrollment start or poll (its credential is the instance-key
/// signature). Exact paths only.
fn is_exempt(method: &Method, path: &str) -> bool {
    *method == Method::POST
        && [
            PAYMENT_WEBHOOK_PATH,
            ENROLLMENT_START_PATH,
            ENROLLMENT_POLL_PATH,
        ]
        .contains(&path)
}

/// Middleware: reject a mutating `/api` request without a valid token,
/// except the machine-called routes in [`is_exempt`].
pub async fn require(req: Request, next: Next) -> Response {
    if is_safe(req.method())
        || !is_api_path(req.uri().path())
        || is_exempt(req.method(), req.uri().path())
    {
        return next.run(req).await;
    }
    let headers = req.headers();
    let cookie = auth::cookie_value(headers, CSRF_COOKIE);
    let mut sent = headers.get_all(CSRF_HEADER).iter();
    // Exactly one header value; a repeated header is never "equal".
    let header = match (sent.next(), sent.next()) {
        (Some(v), None) => v.to_str().ok(),
        _ => None,
    };
    let ok = match (cookie.as_deref(), header) {
        (Some(c), Some(h)) => {
            is_well_formed(c) && is_well_formed(h) && constant_time_eq(c.as_bytes(), h.as_bytes())
        }
        _ => false,
    };
    if !ok {
        return ApiError::new(StatusCode::FORBIDDEN, MSG_BAD_CSRF).into_response();
    }
    next.run(req).await
}

/// `GET /api/auth/csrf`: `{"csrf_token": …}`, reusing a well-formed incoming
/// cookie and otherwise minting and setting a new one.
pub async fn get_token(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    let incoming = auth::cookie_value(&headers, CSRF_COOKIE).filter(|t| is_well_formed(t));
    let (token, fresh) = match incoming {
        Some(t) => (t, false),
        None => (generate_token(), true),
    };
    let mut resp = Json(json!({ "csrf_token": token })).into_response();
    let h = resp.headers_mut();
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if fresh {
        h.insert(
            SET_COOKIE,
            csrf_cookie(&token, auth::secure_cookies(&state.config.public_base_url)),
        );
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_well_formed_and_distinct() {
        let t = generate_token();
        assert_eq!(t.len(), 43);
        assert!(is_well_formed(&t));
        assert_ne!(generate_token(), t);
    }

    #[test]
    fn malformed_tokens() {
        let short = URL_SAFE_NO_PAD.encode([0u8; 31]);
        let long = URL_SAFE_NO_PAD.encode([0u8; 33]);
        let padded = format!("{}=", generate_token());
        let bad_char = format!("{}!", &generate_token()[..42]);
        let std_alphabet = format!("{}+", &generate_token()[..42]);
        for t in [
            "",
            "abc",
            short.as_str(),
            long.as_str(),
            padded.as_str(),
            bad_char.as_str(),
            std_alphabet.as_str(),
        ] {
            assert!(!is_well_formed(t), "{t}");
        }
        // 43 characters whose last one carries non-zero trailing bits.
        assert!(!is_well_formed(&format!("{}B", "A".repeat(42))));
        assert!(is_well_formed(&"A".repeat(43)));
    }

    #[test]
    fn compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn cookie_attributes() {
        assert_eq!(
            csrf_cookie("tok", false).to_str().unwrap(),
            "wipehub_csrf=tok; HttpOnly; SameSite=Lax; Path=/; Max-Age=1209600"
        );
        assert_eq!(
            csrf_cookie("tok", true).to_str().unwrap(),
            "wipehub_csrf=tok; HttpOnly; SameSite=Lax; Path=/; Max-Age=1209600; Secure"
        );
    }

    #[test]
    fn scope() {
        assert!(is_api_path("/api"));
        assert!(is_api_path("/api/"));
        assert!(is_api_path("/api/auth/sign-in"));
        assert!(!is_api_path("/apis"));
        assert!(!is_api_path("/"));
        for m in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(is_safe(&m));
        }
        for m in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(!is_safe(&m));
        }
    }

    #[test]
    fn the_payment_webhook_post_is_exempt() {
        // Built from the constant: `tests/isolation.rs` treats every
        // `"/api…"` literal in `src/` as a route.
        let webhook = PAYMENT_WEBHOOK_PATH;
        assert_eq!(webhook, concat!("/api", "/payments/webhook"));
        assert!(is_exempt(&Method::POST, webhook));
        for (m, p) in [
            (Method::GET, webhook.to_string()),
            (Method::PUT, webhook.to_string()),
            (Method::DELETE, webhook.to_string()),
            (Method::POST, format!("{webhook}/")),
            (Method::POST, format!("{webhook}/x")),
            (Method::POST, webhook.to_uppercase()),
            (Method::POST, webhook.replace("webhook", "fake")),
            (
                Method::POST,
                webhook.replace("/payments/webhook", "/checkout"),
            ),
        ] {
            assert!(!is_exempt(&m, &p), "{m} {p}");
        }
    }

    #[test]
    fn only_the_enrollment_posts_are_exempt() {
        // Built from the constants, as above.
        assert_eq!(ENROLLMENT_START_PATH, concat!("/api", "/enrollments"));
        assert_eq!(ENROLLMENT_POLL_PATH, concat!("/api", "/enrollments/poll"));
        for path in [ENROLLMENT_START_PATH, ENROLLMENT_POLL_PATH] {
            assert!(is_exempt(&Method::POST, path));
            for (m, p) in [
                (Method::GET, path.to_string()),
                (Method::PUT, path.to_string()),
                (Method::DELETE, path.to_string()),
                (Method::POST, format!("{path}/")),
                (Method::POST, format!("{path}/x")),
                (Method::POST, path.to_uppercase()),
            ] {
                assert!(!is_exempt(&m, &p), "{m} {p}");
            }
        }
        // Neighbouring paths are not exempt.
        let approve = ENROLLMENT_START_PATH.replace("enrollments", "enrollments/approve");
        assert!(!is_exempt(&Method::POST, &approve));
        let stations = ENROLLMENT_START_PATH.replace("enrollments", "stations");
        assert!(!is_exempt(&Method::POST, &stations));
    }

    #[test]
    fn portal_csrf_error_matches_the_hub_message() {
        // The portal retries once on exactly this 403 text; pin the copy so
        // a change on either side fails here, not in a browser.
        let portal = include_str!("../../../apps/portal/src/api/auth.ts");
        let pinned = format!("export const CSRF_ERROR = \"{MSG_BAD_CSRF}\";");
        assert!(portal.contains(&pinned), "portal must declare {pinned}");
    }
}
