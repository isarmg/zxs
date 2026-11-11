//! A component budget is planning evidence, never disk admission or write authority.
use super::source::Source;
use crate::{Error, Result};
use name_policy::PortablePath;
use std::path::PathBuf;

#[derive(Debug, error_definition::Error)]
pub enum NameBudgetError {
    #[error("target component maximum {maximum} cannot provide a 16-byte reserve")]
    UnusableMaximum { maximum: u64 },
    #[error("target component maximum {maximum} does not fit this host: {source}")]
    MaximumRange {
        maximum: u64,
        #[source]
        source: std::num::TryFromIntError,
    },
    #[error(
        "source or destination path {path:?} has a component of {bytes} UTF-8 bytes; \
         target permits {allowed}; rename it explicitly and request a new preview"
    )]
    ComponentTooLong {
        path: PathBuf,
        bytes: usize,
        allowed: usize,
    },
}

#[derive(Clone, Copy)]
pub(super) struct NameBudget {
    allowed: usize,
}

impl NameBudget {
    pub(super) fn from_maximum(maximum: u64) -> Result<Self> {
        // Never replace an unavailable or insufficient limit with 255.
        let allowed = maximum
            .checked_sub(16)
            .filter(|allowed| *allowed > 0)
            .ok_or(Error::ConversionNameBudget(NameBudgetError::UnusableMaximum {
                maximum,
            }))?;
        let allowed = usize::try_from(allowed).map_err(|source| {
            Error::ConversionNameBudget(NameBudgetError::MaximumRange { maximum, source })
        })?;
        Ok(Self { allowed })
    }

    pub(super) fn path<'a>(&self, path: &'a str) -> Result<&'a str> {
        for component in path.split('/') {
            if component.len() > self.allowed {
                return Err(Error::ConversionNameBudget(NameBudgetError::ComponentTooLong {
                    path: PathBuf::from(path),
                    bytes: component.len(),
                    allowed: self.allowed,
                }));
            }
        }
        Ok(path)
    }
}

/// All prospective names in this immutable source and destination have been
/// planned against one observation. The caller still owns disk admission.
pub(super) struct ImportNames<'a> {
    pub(super) source: &'a Source,
    destination: Option<&'a PortablePath>,
}

impl<'a> ImportNames<'a> {
    pub(super) fn plan(
        source: &'a Source,
        destination: Option<&'a PortablePath>,
        budget: NameBudget,
    ) -> Result<Self> {
        if let Some(destination) = destination {
            budget.path(destination.as_str())?;
        }
        for item in source.items() {
            budget.path(item.path.as_str())?;
        }
        Ok(Self { source, destination })
    }

    pub(super) fn target_path(&self, path: &PortablePath) -> Result<PortablePath> {
        super::import::target_path(path, self.destination)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/directory_conversion/name_budget_tests.rs"]
mod tests;
