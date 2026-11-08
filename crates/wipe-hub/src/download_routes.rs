//! Customer download routes (software-downloads story 2, CAP-2, CAP-3); the
//! rules are [`crate::downloads`]'s:
//!
//! - `GET /api/downloads` — any role of an entitled organization:
//!   `{releases}`, published only, newest publication first, each with its
//!   files (`id, platform, filename, size_bytes, sha256`).
//! - `POST /api/downloads/:artifact_id/link` — any role of an entitled
//!   organization: `{url, expires_at}`, a signed link to that file good for
//!   five minutes. 404 [`MSG_NO_DOWNLOAD`] for a draft, withdrawn, unknown
//!   or non-UUID artifact.
//! - `GET /api/downloads/:artifact_id/file?user=&expires=&sig=` — no
//!   session: the link is the credential. The stored file, streamed as
//!   `application/octet-stream` with `Content-Disposition: attachment`,
//!   `Content-Length`, `Cache-Control: no-store`, `X-Content-Type-Options:
//!   nosniff` and `Referrer-Policy: no-referrer`, after recording the
//!   download. A missing, malformed, tampered or expired link, or one whose
//!   user is gone, is 403 [`MSG_LINK_INVALID`]; an organization no longer
//!   entitled 403 [`MSG_NOT_ENTITLED`]; a release no longer published 404
//!   [`MSG_NO_DOWNLOAD`]; a stored file the store cannot open 503
//!   [`MSG_STORAGE_UNAVAILABLE`]. None of these writes anything. `GET`
//!   only: `HEAD` (which axum would otherwise route to the GET handler) and
//!   every other method are 405 and write nothing, so a link-preview bot's
//!   probe never records a download.
//!
//! The first two need a session (401 without one) and refuse an unentitled
//! organization, the vendor's included, with 403 [`MSG_NOT_ENTITLED`] before
//! the artifact id is looked at. Neither is audited; the fetch is.

use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio_util::io::ReaderStream;

use crate::artifacts::StoreError;
use crate::auth::{ApiError, CurrentUser};
use crate::downloads::{
    self, DownloadError, DownloadLink, DownloadList, MSG_LINK_INVALID, MSG_NOT_ENTITLED,
    MSG_NO_DOWNLOAD,
};
use crate::release_admin::MSG_STORAGE_UNAVAILABLE;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/downloads", get(list))
        .route("/api/downloads/:artifact_id/link", post(link))
        // axum's `get` also answers `HEAD` with the GET handler, which would
        // record a download for a link-preview bot's probe that received no
        // bytes. Refuse it explicitly; every other method is 405 already.
        .route(
            "/api/downloads/:artifact_id/file",
            get(file).head(head_refused),
        )
}

/// `HEAD` on a download link: 405, nothing verified or written.
async fn head_refused() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, HeaderValue::from_static("GET"))],
    )
        .into_response()
}

impl From<DownloadError> for ApiError {
    fn from(e: DownloadError) -> Self {
        match e {
            DownloadError::NotEntitled => ApiError::new(StatusCode::FORBIDDEN, MSG_NOT_ENTITLED),
            DownloadError::NoDownload => ApiError::new(StatusCode::NOT_FOUND, MSG_NO_DOWNLOAD),
            DownloadError::LinkInvalid => ApiError::new(StatusCode::FORBIDDEN, MSG_LINK_INVALID),
            DownloadError::Db(e) => e.into(),
            DownloadError::Audit(e) => e.into(),
        }
    }
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<DownloadList>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(
        downloads::list_for(&mut conn, user.organization_id).await?,
    ))
}

async fn link(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(artifact): Path<String>,
) -> Result<Json<DownloadLink>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let link = downloads::issue_link(
        &mut conn,
        &state.download_signer,
        state.config.download_link_ttl,
        user.organization_id,
        user.user_id,
        &artifact,
    )
    .await?;
    Ok(Json(link))
}

/// The link's `user`, `expires` and `sig`; `None` for a field that is
/// missing or given more than once.
fn link_fields(query: Option<&str>) -> [Option<String>; 3] {
    let mut out: [Option<String>; 3] = Default::default();
    let mut seen = [0u8; 3];
    for (k, v) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
        let i = match k.as_ref() {
            "user" => 0,
            "expires" => 1,
            "sig" => 2,
            _ => continue,
        };
        seen[i] = seen[i].saturating_add(1);
        out[i] = Some(v.into_owned());
    }
    for i in 0..3 {
        if seen[i] != 1 {
            out[i] = None;
        }
    }
    out
}

async fn file(
    State(state): State<AppState>,
    Path(artifact): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let [user, expires, sig] = link_fields(query.as_deref());
    let (artifact_id, user_id) = state.download_signer.verify(
        &artifact,
        user.as_deref(),
        expires.as_deref(),
        sig.as_deref(),
        downloads::unix_now(),
    )?;

    // Open first: a store failure must record nothing.
    let reader = match state.artifacts.open(&artifact_id.to_string()).await {
        Ok(r) => r,
        Err(cause) => {
            let missing = matches!(cause, StoreError::NotFound);
            tracing::error!(
                %artifact_id,
                error = %cause,
                missing,
                "a release file could not be opened for download"
            );
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                MSG_STORAGE_UNAVAILABLE,
            ));
        }
    };

    let mut tx = state.pool.begin().await?;
    let fetch = downloads::resolve_fetch(&mut tx, artifact_id, user_id).await?;
    downloads::record(&mut tx, &fetch).await?;
    tx.commit().await?;

    let disposition = HeaderValue::from_str(&format!(
        "attachment; filename=\"{}\"",
        fetch.artifact.filename
    ))
    .map_err(ApiError::internal)?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            ),
            (header::CONTENT_DISPOSITION, disposition),
            (
                header::CONTENT_LENGTH,
                HeaderValue::from(fetch.artifact.size_bytes),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            ),
        ],
        Body::from_stream(ReaderStream::new(reader)),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_fields_need_each_exactly_once() {
        assert_eq!(
            link_fields(Some("user=u&expires=1&sig=s&other=x")),
            [Some("u".into()), Some("1".into()), Some("s".into())]
        );
        assert_eq!(link_fields(None), [None, None, None]);
        assert_eq!(
            link_fields(Some("user=u&user=v&expires=1&sig=s")),
            [None, Some("1".into()), Some("s".into())]
        );
    }
}
