//! Import plans bind every prospective path to one target budget observation.
use super::source::Source;
pub(super) use crate::name_budget::NameBudget;
use crate::{Error, Result};
use name_policy::PortablePath;

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
            budget.path(destination.as_str()).map_err(Error::ConversionNameBudget)?;
        }
        for item in source.items() {
            budget.path(item.path.as_str()).map_err(Error::ConversionNameBudget)?;
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
