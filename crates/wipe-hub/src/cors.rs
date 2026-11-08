//! Credentialed CORS for the marketing site (lead-capture story 1).
//!
//! The site is a static page on another origin. To post a sales lead through
//! the double-submit CSRF check (ADR-0007 §3, no exemption) it fetches
//! `GET /api/auth/csrf` and then `POST /api/leads`, both with
//! `credentials: "include"`. Only those two paths, and their `OPTIONS`
//! preflights, get CORS headers, and only when `Origin` equals the
//! configured `--site-origin` exactly. Any other origin gets no
//! `Access-Control-*` header at all, so the browser keeps the response from
//! it.
//!
//! This layer wraps [`crate::csrf::require`], so a 403 for a missing token
//! (and every 400 and 429) carries the headers and the site can read it.
//! The cookie is `SameSite=Lax`, so the site and the hub must be the same
//! *site* (registrable domain) for the browser to send it (CONTEXT §10).

use axum::extract::{Request, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE, ORIGIN,
    VARY,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::AppState;

/// The only paths the marketing site calls.
pub const SITE_PATHS: [&str; 2] = ["/api/auth/csrf", "/api/leads"];

/// The request's single `Origin`, if it equals `site_origin` exactly.
fn site_origin_of(headers: &HeaderMap, site_origin: &str) -> Option<HeaderValue> {
    let mut values = headers.get_all(ORIGIN).iter();
    match (values.next(), values.next()) {
        (Some(v), None) if v.as_bytes() == site_origin.as_bytes() => Some(v.clone()),
        _ => None,
    }
}

fn add_headers(headers: &mut HeaderMap, origin: HeaderValue) {
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    headers.insert(
        ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    headers.append(VARY, HeaderValue::from_static("Origin"));
    headers.insert(
        ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("Retry-After"),
    );
}

/// Middleware: answer the site's preflights and add CORS headers to its
/// requests; anything else passes through untouched.
pub async fn site_cors(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let origin = if SITE_PATHS.contains(&req.uri().path()) {
        site_origin_of(req.headers(), &state.config.site_origin)
    } else {
        None
    };
    let Some(origin) = origin else {
        return next.run(req).await;
    };
    if req.method() == Method::OPTIONS {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        let h = resp.headers_mut();
        add_headers(h, origin);
        h.insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST"),
        );
        h.insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type, x-csrf-token"),
        );
        h.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        return resp;
    }
    let mut resp = next.run(req).await;
    add_headers(resp.headers_mut(), origin);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_exactly_equal_origin_matches() {
        let site = "http://localhost:4321";
        let with = |values: &[&str]| {
            let mut h = HeaderMap::new();
            for v in values {
                h.append(ORIGIN, HeaderValue::from_str(v).unwrap());
            }
            site_origin_of(&h, site)
        };
        assert_eq!(with(&[site]).unwrap(), site);
        for other in [
            "http://localhost:4322",
            "http://localhost:4321/",
            "https://localhost:4321",
            "http://LOCALHOST:4321",
            "http://127.0.0.1:4321",
            "null",
            "",
        ] {
            assert!(with(&[other]).is_none(), "{other}");
        }
        assert!(with(&[]).is_none());
        assert!(with(&[site, site]).is_none());
    }
}
