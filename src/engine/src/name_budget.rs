//! One UTF-8 component reserve shared by prospective name admissions.
//! The observed budget is not a disk identity, a write grant or a replay rule.
use std::path::PathBuf;

#[derive(Debug, error_definition::Error)]
pub enum NameBudgetError {
    #[error("conversion name budget belongs to another disk registration")]
    TargetRegistrationChanged,
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
pub(crate) struct NameBudget {
    allowed: usize,
}

impl NameBudget {
    pub(crate) fn from_maximum(maximum: u64) -> Result<Self, NameBudgetError> {
        // Never replace an unavailable or insufficient limit with 255.
        let allowed = maximum
            .checked_sub(16)
            .filter(|allowed| *allowed > 0)
            .ok_or(NameBudgetError::UnusableMaximum { maximum })?;
        let allowed = usize::try_from(allowed)
            .map_err(|source| NameBudgetError::MaximumRange { maximum, source })?;
        Ok(Self { allowed })
    }

    /// Import introduces every component in each source/destination path.
    pub(crate) fn path<'a>(&self, path: &'a str) -> Result<&'a str, NameBudgetError> {
        for component in path.split('/') {
            self.component(component, path)?;
        }
        Ok(path)
    }

    /// Check only the newly introduced component, keeping its path for diagnostics.
    pub(crate) fn component(&self, component: &str, path: &str) -> Result<(), NameBudgetError> {
        if component.len() > self.allowed {
            return Err(NameBudgetError::ComponentTooLong {
                path: PathBuf::from(path),
                bytes: component.len(),
                allowed: self.allowed,
            });
        }
        Ok(())
    }
}
