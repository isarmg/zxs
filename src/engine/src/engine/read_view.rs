use crate::engine::Engine;
use storage_contract::{ContentState, FileVersion, VersionId};

/// Counters borrowed from the current owner for the lifetime of the view.
///
/// The owner remains borrowed until the last use of the view:
///
/// ```compile_fail,E0505
/// use storage_engine::Engine;
///
/// fn release_owner(engine: Engine) {
///     let read = engine.read();
///     drop(engine);
///     let _sequence = read.sequence();
/// }
/// ```
///
/// Mutating the owner requires the shared borrow to end first:
///
/// ```compile_fail,E0502
/// use storage_engine::Engine;
///
/// fn change_owner(_: &mut Engine) {}
/// fn hold_read(engine: &mut Engine) {
///     let read = engine.read();
///     change_owner(engine);
///     let _sequence = read.sequence();
/// }
/// ```
pub struct ReadView<'a> {
    sequence: &'a u64,
    topology_generation: &'a u64,
}

impl<'a> ReadView<'a> {
    pub(in crate::engine) fn new(sequence: &'a u64, topology_generation: &'a u64) -> Self {
        Self {
            sequence,
            topology_generation,
        }
    }

    /// Sequence of the owner's current published state.
    pub fn sequence(&self) -> u64 {
        *self.sequence
    }

    /// Topology generation of the owner's current published state.
    pub fn topology_generation(&self) -> u64 {
        *self.topology_generation
    }
}

impl Engine {
    /// Borrow the current published counters in constant time.
    pub fn read(&self) -> ReadView<'_> {
        ReadView::new(&self.state.sequence, &self.state.topology_generation)
    }

    /// Borrow one version and its optional stored disposition from this owner.
    /// This is metadata, not an authorization grant. Callers must retain current
    /// admission and check the version's history and repository under the same lock.
    /// Sparse disposition absence is preserved for the caller's existing wire policy.
    ///
    /// The returned record cannot outlive or cross a mutation of its owner:
    ///
    /// ```compile_fail,E0505
    /// use storage_engine::Engine;
    /// use storage_contract::VersionId;
    ///
    /// fn release_owner(engine: Engine, id: VersionId) {
    ///     let (version, _) = engine.version_metadata(id).unwrap();
    ///     drop(engine);
    ///     let _size = version.logical_size;
    /// }
    /// ```
    ///
    /// ```compile_fail,E0502
    /// use storage_engine::Engine;
    /// use storage_contract::VersionId;
    ///
    /// fn change_owner(_: &mut Engine) {}
    /// fn hold_version(engine: &mut Engine, id: VersionId) {
    ///     let (version, _) = engine.version_metadata(id).unwrap();
    ///     change_owner(engine);
    ///     let _size = version.logical_size;
    /// }
    /// ```
    pub fn version_metadata(&self, id: VersionId) -> Option<(&FileVersion, Option<ContentState>)> {
        let key = id.to_string();
        let version = self.state.versions.get(&key)?;
        let disposition = self.state.content_states.get(&version.version_id).copied();
        Some((version, disposition))
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/read_view_tests.rs"]
mod tests;
