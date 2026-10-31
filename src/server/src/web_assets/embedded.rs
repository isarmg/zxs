//! Immutable executable-owned Web bytes. No extraction or runtime filesystem dependency.
use super::*;
use bytes::Bytes;
use http_routing::body::{Body, HttpBody};
use std::time::{Duration, SystemTime};

struct Asset {
    path: &'static str,
    bytes: &'static [u8],
}
struct Bundle {
    source_commit: &'static str,
    source_tree: &'static str,
    manifest_sha256: &'static str,
    modified_seconds: u64,
    files: &'static [Asset],
}
include!(concat!(env!("OUT_DIR"), "/embedded_web.rs"));

pub(crate) fn etags() -> Option<BTreeMap<String, String>> {
    (!BUNDLE.files.is_empty()).then(|| validators(BUNDLE.files))
}
fn validators(files: &[Asset]) -> BTreeMap<String, String> {
    let mut tags = BTreeMap::new();
    for file in files.iter().filter(|file| !file.path.ends_with(".gz")) {
        let tag = format!("W/\"{}\"", blake3::hash(file.bytes).to_hex());
        tags.insert(format!("/{}", file.path), tag.clone());
        if file.path == "index.html" {
            tags.insert("/".into(), tag);
        }
    }
    tags
}
pub(crate) fn fallback() -> Router {
    tracing::debug!(
        commit = BUNDLE.source_commit,
        tree = BUNDLE.source_tree,
        manifest_sha256 = BUNDLE.manifest_sha256,
        "using embedded Web assets"
    );
    route(&BUNDLE)
}
fn route(bundle: &'static Bundle) -> Router {
    Router::new()
        .fallback_service(service_stack::service_fn(
            move |request: Request| async move {
                Ok::<_, std::convert::Infallible>(serve(bundle, request).await)
            },
        ))
        .layer(middleware::from_fn(super::reserve_api_namespace))
}
fn find<'a>(files: &'a [Asset], path: &str) -> Option<&'a Asset> {
    files
        .binary_search_by_key(&path, |file| file.path)
        .ok()
        .map(|index| &files[index])
}
fn normalized_path(path: &str) -> Option<String> {
    let decoded = url_encoding::percent::percent_decode(path.trim_start_matches('/').as_bytes())
        .decode_utf8()
        .ok()?;
    let mut parts = Vec::new();
    for part in Path::new(decoded.as_ref()).components() {
        match part {
            std::path::Component::Normal(value) => parts.push(value.to_str()?),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}
// Same supported representations as ServeDir::precompressed_gzip: explicit gzip
// and identity tokens, RFC q-values, no wildcard extension or hidden decompression.
fn gzip_requested(headers: &http::HeaderMap) -> bool {
    let mut best = (0u16, false);
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else { continue };
        for item in value.split(',') {
            let mut parts = item.trim().splitn(2, ';');
            let gzip = match parts.next().unwrap().trim() {
                "gzip" => true,
                "identity" => false,
                _ => continue,
            };
            let quality = match parts.next() {
                None => 1000,
                Some(value) => {
                    let Some(q) = quality(value.trim()) else {
                        continue;
                    };
                    q
                }
            };
            if quality > 0 {
                best = best.max((quality, gzip));
            }
        }
    }
    best.1
}
fn quality(value: &str) -> Option<u16> {
    let value = value.strip_prefix("q=")?;
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if !matches!(whole, "0" | "1")
        || fraction.len() > 3
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u16>().ok()? * 10u16.pow(3 - fraction.len() as u32)
    };
    if whole == "1" {
        (fraction == 0).then_some(1000)
    } else {
        Some(fraction)
    }
}
fn date(headers: &http::HeaderMap, name: http::HeaderName) -> Option<SystemTime> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| httpdate::parse_http_date(value).ok())
}
async fn absent(method: &Method) -> Response {
    let mut response = not_found().await.into_response();
    if let Some(length) = response.body().size_hint().exact() {
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    }
    if *method == Method::HEAD {
        *response.body_mut() = Body::empty();
    }
    response
}

async fn serve(bundle: &Bundle, request: Request) -> Response {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET,HEAD"));
        return response;
    }
    let Some(mut path) = normalized_path(request.uri().path()) else {
        return absent(request.method()).await;
    };
    // Reserve the logical API namespace after decoding too; no static alias
    // may manufacture an API response from executable-owned asset bytes.
    if is_api_path(&format!("/{path}")) {
        return absent(request.method()).await;
    }
    let directory = path.is_empty()
        || bundle
            .files
            .iter()
            .any(|asset| asset.path.starts_with(&format!("{path}/")));
    if directory {
        if !request.uri().path().ends_with('/') {
            let location = format!(
                "{}/{}",
                request.uri().path(),
                request
                    .uri()
                    .query()
                    .map(|q| format!("?{q}"))
                    .unwrap_or_default()
            );
            return Response::builder()
                .status(StatusCode::TEMPORARY_REDIRECT)
                .header(header::LOCATION, location)
                .body(Body::empty())
                .unwrap();
        }
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str("index.html");
    }
    let Some(original) = find(bundle.files, &path) else {
        return absent(request.method()).await;
    };
    let compressed = gzip_requested(request.headers())
        .then(|| find(bundle.files, &format!("{path}.gz")))
        .flatten();
    let file = compressed.unwrap_or(original);
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(bundle.modified_seconds);
    if date(request.headers(), header::IF_UNMODIFIED_SINCE).is_some_and(|date| modified > date) {
        return StatusCode::PRECONDITION_FAILED.into_response();
    }
    if date(request.headers(), header::IF_MODIFIED_SINCE).is_some_and(|date| modified <= date) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let size = file.bytes.len();
    let mut response = Response::builder()
        .header(
            header::CONTENT_TYPE,
            mime_guess::from_path(&path)
                .first_raw()
                .unwrap_or("application/octet-stream"),
        )
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::LAST_MODIFIED, httpdate::fmt_http_date(modified));
    if compressed.is_some() {
        response = response.header(header::CONTENT_ENCODING, "gzip");
    }
    let mut selected = file.bytes;
    if *request.method() == Method::GET
        && let Some(range) = request
            .headers()
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok())
    {
        match http_range_header::parse_range_header(range)
            .and_then(|ranges| ranges.validate(size as u64))
        {
            Ok(ranges) if ranges.len() == 1 => {
                let range = &ranges[0];
                selected = &file.bytes[*range.start() as usize..=*range.end() as usize];
                response = response.status(StatusCode::PARTIAL_CONTENT).header(
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{size}", range.start(), range.end()),
                );
            }
            result => {
                let body = if result.as_ref().is_ok_and(|ranges| ranges.len() > 1) {
                    Body::from("Cannot serve multipart range requests")
                } else {
                    Body::empty()
                };
                return response
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                    .body(body)
                    .unwrap();
            }
        }
    }
    response
        .header(header::CONTENT_LENGTH, selected.len())
        .body(if *request.method() == Method::HEAD {
            Body::empty()
        } else {
            Body::from(Bytes::from_static(selected))
        })
        .unwrap()
}

#[cfg(test)]
#[path = "../../../../test/server/unit/embedded_web.rs"]
mod tests;
