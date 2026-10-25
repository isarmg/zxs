//! One route boundary for operational CLI HTTP and browser HTTP transports.
use crate::{Error, Result};

/// Accept only this service's `/api/` routes before URL parsing can normalize
/// dot segments or strip control characters. Query values are not path parts.
pub fn validate_api_path(value: &str) -> Result<()> {
    let path = value.split('?').next().unwrap_or_default();
    let invalid = !path.starts_with("/api/")
        || value.chars().any(char::is_control)
        || value.contains(['#', '\\'])
        || path.split('/').any(|part| {
            let part = part.to_ascii_lowercase().replace("%2e", ".");
            matches!(part.as_str(), "." | "..") || part.contains("%2f") || part.contains("%5c")
        });
    if invalid {
        Err(Error::new(
            crate::ErrorCode::InvalidApiPath,
            "expected a service-local /api/ API route",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/api_path/tests.rs"]
mod tests;
