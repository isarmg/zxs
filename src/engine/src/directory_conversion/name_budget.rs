//! Import plans bind every prospective path to one target budget observation.
use super::source::Source;
pub(super) use crate::name_budget::NameBudget;
use crate::{Error, Result};
use name_policy::PortablePath;
use storage_contract::{Entry, NamespaceEntryKind};

/// Check imported source and newly introduced destination components.
/// Only unchanged ancestor spellings reuse their logical names. This is not
/// physical-presence proof; the caller still owns disk admission.
pub(super) struct ImportNames<'a> {
    pub(super) source: &'a Source,
    destination: Option<&'a PortablePath>,
}

impl<'a> ImportNames<'a> {
    pub(super) fn plan(
        source: &'a Source,
        destination: Option<&'a PortablePath>,
        existing: &[Entry],
        budget: NameBudget,
    ) -> Result<Self> {
        if let Some(destination) = destination {
            budget.component(destination.file_name(), destination.as_str())
                .map_err(Error::ConversionNameBudget)?;
            let mut parent = destination.parent();
            while let Some(path) = parent {
                if !existing.iter().any(|entry| {
                    entry.kind == NamespaceEntryKind::Directory
                        && entry.lookup_key() == path.key()
                        && entry.name() == path.file_name()
                }) {
                    budget.component(path.file_name(), destination.as_str())
                        .map_err(Error::ConversionNameBudget)?;
                }
                parent = path.parent();
            }
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
