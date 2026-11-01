//! Browser-origin policy shared by ordinary HTTP and setup adapters.
use crate::{
    Error, HOST_HEADER, ORIGIN_HEADER, SEC_FETCH_SITE_HEADER, require_single_security_header_value,
};
use url_encoding::address::Url;

/// A configured HTTPS origin, normalized once to browser serialization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalOrigin(String);

impl ExternalOrigin {
    /// Parse deployment configuration, rejecting paths, credentials and non-HTTPS origins.
    pub fn parse(value: &str) -> Result<Self, Error> {
        let parsed = Url::parse(value).map_err(Error::OriginUrl)?;
        if parsed.scheme() != "https" || !is_origin_url(&parsed) || value.ends_with('/') {
            return Err(Error::InvalidExternalOrigin);
        }
        Ok(Self(parsed.origin().ascii_serialization()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OriginRequirement {
    Required,
    Optional,
}

/// Proof that the supplied Origin exactly matches the selected deployment origin.
/// This is not authentication or a proof of trusted proxy/transport identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedSameOrigin(String);

impl VerifiedSameOrigin {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_origin_url(parsed: &Url) -> bool {
    parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.path() == "/"
        && parsed.query().is_none()
        && parsed.fragment().is_none()
}

fn direct_http_origin(host: &str) -> Result<String, Error> {
    let parsed = Url::parse(&format!("http://{host}")).map_err(Error::OriginUrl)?;
    if !is_origin_url(&parsed) || host.contains('/') || host.contains('\\') {
        return Err(Error::InvalidSecurityHeaderValue { name: HOST_HEADER });
    }
    Ok(parsed.origin().ascii_serialization())
}

/// Verify complete wire header lists, without trusting forwarded headers.
///
/// Direct HTTP permits any host, including non-loopback addresses. With a configured
/// external HTTPS origin the proxy's internal Host need not equal the external host;
/// any supplied Host must still be a single valid authority. Fetch Metadata is optional,
/// and must be a singleton. With Origin present it must be `same-origin`. Missing
/// Origin is allowed only when the adapter selects Optional (e.g. Bearer); supplied
/// Origin is always checked.
/// URL parse errors retain their source. Header multiplicity/shape and mismatches are
/// explicit errors. Neither this function nor its result authorizes a session.
pub fn verify_same_origin<O: AsRef<[u8]>, H: AsRef<[u8]>, S: AsRef<[u8]>>(
    external: Option<&ExternalOrigin>,
    requirement: OriginRequirement,
    origins: &[O],
    hosts: &[H],
    sites: &[S],
) -> Result<Option<VerifiedSameOrigin>, Error> {
    // Even origin-less native requests cannot supply ambiguous field lines.
    let site = optional_fetch_site(sites)?;
    let direct = if hosts.is_empty() {
        None
    } else {
        Some(direct_http_origin(require_single_security_header_value(
            HOST_HEADER,
            hosts,
        )?)?)
    };
    if origins.is_empty() && requirement == OriginRequirement::Optional {
        return Ok(None);
    }
    if let Some(site) = site {
        require_same_site_value(site)?;
    }
    let origin = require_single_security_header_value(ORIGIN_HEADER, origins)?;
    let expected = match external {
        Some(external) => external.as_str(),
        None => direct
            .as_deref()
            .ok_or(Error::MissingSecurityHeader { name: HOST_HEADER })?,
    };
    if origin != expected {
        return Err(Error::OriginMismatch);
    }
    Ok(Some(VerifiedSameOrigin(origin.to_owned())))
}

fn optional_fetch_site<S: AsRef<[u8]>>(sites: &[S]) -> Result<Option<&str>, Error> {
    if sites.is_empty() {
        Ok(None)
    } else {
        require_single_security_header_value(SEC_FETCH_SITE_HEADER, sites).map(Some)
    }
}

fn require_same_site_value(site: &str) -> Result<(), Error> {
    if site != "same-origin" {
        return Err(Error::InvalidSecurityHeaderValue {
            name: SEC_FETCH_SITE_HEADER,
        });
    }
    Ok(())
}

/// Bootstrap endpoints require any supplied Fetch Metadata to be same-origin,
/// including read-only status requests without Origin. Missing metadata is legal.
pub fn require_same_origin_fetch_metadata<S: AsRef<[u8]>>(sites: &[S]) -> Result<(), Error> {
    if let Some(site) = optional_fetch_site(sites)? {
        require_same_site_value(site)?;
    }
    Ok(())
}
