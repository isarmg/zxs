//! Static asset admission, validators and the reserved API namespace.
use crate::{
    ApiError, AppState,
    http_boundary::{is_api_path, not_found},
};
use http_routing::{
    Router,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) fn fallback(directory: PathBuf) -> Router {
    Router::new()
        .fallback_service(
            tower_http::services::ServeDir::new(directory)
                .append_index_html_on_directories(true)
                .precompressed_gzip()
                .not_found_service(Router::new().fallback(not_found)),
        )
        .layer(middleware::from_fn(reserve_api_namespace))
}

pub(crate) fn unavailable_fallback() -> Router {
    Router::new()
        .fallback(|| async {
            let mut response = ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::WebAssetsUnavailable,
                "Web UI assets are unavailable; management HTTP APIs remain available",
            )
            .into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        })
        .layer(middleware::from_fn(reserve_api_namespace))
}

async fn reserve_api_namespace(request: Request, next: Next) -> Response {
    if is_api_path(request.uri().path()) {
        return not_found().await.into_response();
    }
    next.run(request).await
}

pub(crate) fn static_etags(root: &Path) -> Result<BTreeMap<String, String>, ApiError> {
    fn walk(
        base: &Path,
        directory: &Path,
        result: &mut BTreeMap<String, String>,
    ) -> Result<(), ApiError> {
        for entry in std::fs::read_dir(directory).map_err(ApiError::io)? {
            let entry = entry.map_err(ApiError::io)?;
            let metadata = entry.path().symlink_metadata().map_err(ApiError::io)?;
            if metadata.file_type().is_symlink() {
                return Err(ApiError::invalid(
                    "web assets must not contain symbolic links",
                ));
            }
            if metadata.is_dir() {
                walk(base, &entry.path(), result)?;
            } else if metadata.is_file() {
                if result.len() >= 4096 || metadata.len() > 64 * 1024 * 1024 {
                    return Err(ApiError::invalid("web assets exceed admission limits"));
                }
                let relative = entry
                    .path()
                    .strip_prefix(base)
                    .expect("invariant: directory traversal yields descendants of its root")
                    .to_string_lossy()
                    .replace('\\', "/");
                if relative.ends_with(".gz") {
                    continue;
                }
                let bytes = std::fs::read(entry.path()).map_err(ApiError::io)?;
                let etag = format!("W/\"{}\"", blake3::hash(&bytes).to_hex());
                result.insert(format!("/{relative}"), etag.clone());
                if relative == "index.html" {
                    result.insert("/".into(), etag);
                }
            } else {
                return Err(ApiError::invalid(
                    "web assets must contain only regular files and directories",
                ));
            }
        }
        Ok(())
    }
    // Check each normalized component, not a pathname ending in `/` or `/.`:
    // lstat otherwise follows the final directory link. Root ancestors must not
    // introduce links either. This is admission, not race-safe file opening.
    let mut checked_root = PathBuf::new();
    for component in root.components() {
        checked_root.push(component);
        if !checked_root
            .symlink_metadata()
            .map_err(ApiError::io)?
            .is_dir()
        {
            return Err(ApiError::invalid(
                "web asset root must contain only directory components, not links or files",
            ));
        }
    }
    let root = checked_root.as_path();
    let mut result = BTreeMap::new();
    walk(root, root, &mut result)?;
    if !result.contains_key("/index.html") {
        return Err(ApiError::invalid("web asset root must contain index.html"));
    }
    Ok(result)
}

pub(crate) async fn static_validation(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    // Reserved API paths never use static validators, even when an accidental
    // same-named file exists below the configured asset directory.
    let etag = if is_api_path(request.uri().path()) {
        None
    } else {
        state.static_etags.get(request.uri().path()).cloned()
    };
    if matches!(*request.method(), Method::GET | Method::HEAD)
        && let Some(etag) = &etag
        && request
            .headers()
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|candidate| candidate.trim() == etag || candidate.trim() == "*")
            })
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response
            .headers_mut()
            .insert(header::ETAG, HeaderValue::from_str(etag).unwrap());
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("accept-encoding"));
        return response;
    }
    if matches!(*request.method(), Method::GET | Method::HEAD)
        && etag.is_some()
        && request.headers().contains_key(header::IF_NONE_MATCH)
    {
        // This layer has evaluated the entity tag. ServeDir's date validator
        // must not override that result (RFC 9110 section 13.1.3).
        request.headers_mut().remove(header::IF_MODIFIED_SINCE);
    }
    let mut response = next.run(request).await;
    if response.status().is_success()
        && let Some(etag) = etag
    {
        response
            .headers_mut()
            .insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    }
    response
}
