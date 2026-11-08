use super::{State, http::Failure};
use http_routing::{
    extract::Request,
    http::{HeaderMap, StatusCode},
};
use std::net::{IpAddr, SocketAddr};

fn values<'a>(headers: &'a HeaderMap, name: &'static str) -> Vec<&'a [u8]> {
    headers
        .get_all(name)
        .iter()
        .map(|value| value.as_bytes())
        .collect()
}
fn single(headers: &HeaderMap, name: &'static str) -> Result<String, Failure> {
    let raw = values(headers, name);
    authentication::require_single_security_header_value(name, &raw)
        .map(str::to_owned)
        .map_err(|error| Failure::new(StatusCode::FORBIDDEN, "request_headers", error))
}

pub(super) fn host(state: &State, request: &Request) -> Result<(), Failure> {
    if request.uri().query().is_some() {
        return Err(Failure::message(
            StatusCode::BAD_REQUEST,
            "request_query",
            "Helper routes do not accept URL query parameters.",
        ));
    }
    let host = single(request.headers(), "host")?;
    let address = if let Some(port) = host.strip_prefix("localhost:") {
        SocketAddr::new(
            IpAddr::from([127, 0, 0, 1]),
            port.parse()
                .map_err(|error| Failure::new(StatusCode::FORBIDDEN, "request_host", error))?,
        )
    } else if host == "localhost" {
        SocketAddr::from(([127, 0, 0, 1], 80))
    } else {
        host.parse::<SocketAddr>()
            .or_else(|_| host.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, 80)))
            .map_err(|error| Failure::new(StatusCode::FORBIDDEN, "request_host", error))?
    };
    if address.port() != state.listen.port()
        || address.ip().is_unspecified()
        || (!state.listen.ip().is_unspecified() && address.ip() != state.listen.ip())
    {
        return Err(Failure::message(
            StatusCode::FORBIDDEN,
            "request_host",
            "Use the listener's literal IP and port; DNS names and forwarded hosts are not accepted.",
        ));
    }
    Ok(())
}

pub(super) fn post(state: &State, request: &Request) -> Result<(), Failure> {
    host(state, request)?;
    authentication::verify_same_origin(
        None,
        authentication::OriginRequirement::Required,
        &values(request.headers(), "origin"),
        &values(request.headers(), "host"),
        &values(request.headers(), "sec-fetch-site"),
    )
    .map_err(|error| Failure::new(StatusCode::FORBIDDEN, "request_origin", error))?;
    if single(request.headers(), "x-zxs-helper-csrf")? != state.csrf.as_ref() {
        return Err(Failure::message(
            StatusCode::FORBIDDEN,
            "request_csrf",
            "Reload this helper console before submitting.",
        ));
    }
    let content_type = single(request.headers(), "content-type")?;
    if content_type != "application/json" {
        return Err(Failure::message(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "request_type",
            "Use application/json; no form, script or URL submissions are accepted.",
        ));
    }
    Ok(())
}
