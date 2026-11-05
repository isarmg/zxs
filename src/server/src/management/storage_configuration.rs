//! Read-only setup projection admission across recorded Add/Exit successors.
//! This graph never grants storage authority. Engine startup still authenticates
//! the fixed membership lineage, bootstrap stops and publication receipts.
use super::storage_addition::{configuration_digest, latest_name, read, storage_error};
use crate::ApiError;
use platform_io::fs_safety::{
    AtomicFile, PrivateDirectory,
    linux::{MountPolicy, OpenAt2Root},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use storage_contract::{Config, ErrorCode, OperationId};

#[derive(Default)]
pub(super) struct ConfigurationChain {
    predecessors: BTreeMap<String, BTreeSet<String>>,
    tips: BTreeSet<String>,
}
impl ConfigurationChain {
    pub(super) fn edge(&mut self, original: String, target: String) {
        if original != target {
            self.predecessors
                .entry(target)
                .or_default()
                .insert(original);
        }
    }
    pub(super) fn tip(&mut self, digest: String) {
        self.tips.insert(digest);
    }
    fn connects(&self, current: &str, initial: &str) -> bool {
        if !self.tips.contains(current) {
            return false;
        }
        let mut pending = vec![current];
        let mut visited = BTreeSet::new();
        while let Some(next) = pending.pop() {
            if next == initial {
                return true;
            }
            if visited.insert(next)
                && let Some(predecessors) = self.predecessors.get(next)
            {
                pending.extend(predecessors.iter().map(String::as_str));
            }
        }
        false
    }
}

/// Stream names from one held private directory; only the two digests per
/// validated record remain in memory. Per-record decoding retains its existing
/// byte bound. There is no full-record clone or artificial lifecycle-count cap.
pub(super) fn records<T>(
    path: &Path,
    mut load: impl FnMut(&PrivateDirectory, OperationId) -> Result<Option<T>, ApiError>,
    mut consume: impl FnMut(T, bool) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let root = match OpenAt2Root::open(path, MountPolicy::SameMount) {
        Ok(root) => root,
        Err(platform_io::fs_safety::Error::Io(source))
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(());
        }
        Err(source) => return Err(storage_error(source)),
    };
    let entries = root.entries().map_err(storage_error)?;
    let directory = PrivateDirectory::from_directory(root.into_directory(), path.to_owned())
        .map_err(storage_error)?;
    let latest: Option<OperationId> = read(&directory, &latest_name())?;
    let mut found_latest = latest.is_none();
    for name in entries {
        let name = name.map_err(storage_error)?;
        if name == latest_name() || AtomicFile::is_temporary_name(&name) {
            continue;
        }
        let operation = name
            .as_os_str()
            .to_str()
            .and_then(|value| value.strip_suffix(".json"))
            .ok_or_else(|| {
                ApiError::conflict(
                    ErrorCode::ObjectCorrupt,
                    "storage intent directory contains an unrecognized record name",
                )
            })?
            .parse::<OperationId>()
            .map_err(ApiError::invalid_source)?;
        let record = load(&directory, operation)?.ok_or_else(|| {
            ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "storage intent disappeared during setup admission",
            )
        })?;
        let is_latest = latest == Some(operation);
        found_latest |= is_latest;
        consume(record, is_latest)?;
    }
    if !found_latest {
        return Err(ApiError::conflict(
            ErrorCode::RecoveryRequired,
            "latest storage intent is missing",
        ));
    }
    Ok(())
}

pub(crate) fn admits_configuration(
    root: &Path,
    current: &Config,
    initial: &Config,
) -> Result<bool, ApiError> {
    let mut chain = ConfigurationChain::default();
    super::storage_addition::configuration_edges(root, &mut chain)?;
    super::storage_lifecycle::configuration_edges(root, &mut chain)?;
    super::storage_readmission::configuration_edges(root, &mut chain)?;
    Ok(chain.connects(
        &configuration_digest(current),
        &configuration_digest(initial),
    ))
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/storage_configuration.rs"]
mod tests;
