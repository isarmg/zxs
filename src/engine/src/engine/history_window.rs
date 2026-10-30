//! Exact per-history root identity and the derived owner index for bounded
//! file-history reads.
//!
//! A `HistoryCatalog` binds the published history catalog of one admitted
//! `State` allocation. The root of a file's history is the exact
//! `history-manifest` object hash recorded in that catalog; it is neither a
//! sequence number nor a graph hash recomputed per request.
use super::Engine;
use crate::{Error, ErrorCode, Result, State};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock, Weak},
};

/// Resolved roots memoized for one catalog; cleared whole when full.
const MEMO_CAPACITY: usize = 256;

/// Exactly 64 lowercase hexadecimal digits naming one `history-manifest` object.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct HistoryRoot(String);

impl HistoryRoot {
    /// # Errors
    /// Returns `InvalidRequest` unless the text is exactly 64 lowercase hex digits.
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "history root must be 64 lowercase hexadecimal digits",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_catalog(hash: &str) -> Self {
        // Catalog digests were validated by the encoder or by `catalog_entry`.
        Self(hash.to_owned())
    }
}

impl std::fmt::Display for HistoryRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub(crate) enum CatalogSource {
    /// The catalog map root installed by this process's own publication.
    Published(crate::metadata_map::MapRoot),
    /// The catalog reference admitted by recovery; entries are read on demand.
    Recovered(crate::metadata_map::Reference),
}

/// The admitted history catalog of exactly one `State` allocation.
pub(crate) struct HistoryCatalog {
    state: Weak<State>,
    source: CatalogSource,
    memo: Mutex<BTreeMap<String, HistoryRoot>>,
    owners: Arc<OnceLock<crate::state_projection::DerivedOwners>>,
}

impl HistoryCatalog {
    pub(crate) fn published(state: &Arc<State>, roots: crate::metadata_map::MapRoot) -> Self {
        Self::new(state, CatalogSource::Published(roots))
    }

    pub(crate) fn recovered(state: &Arc<State>, reference: crate::metadata_map::Reference) -> Self {
        Self::new(state, CatalogSource::Recovered(reference))
    }

    fn new(state: &Arc<State>, source: CatalogSource) -> Self {
        Self {
            state: Arc::downgrade(state),
            source,
            memo: Mutex::new(BTreeMap::new()),
            owners: Arc::new(OnceLock::new()),
        }
    }

    fn binds(&self, state: &Arc<State>) -> bool {
        self.state.ptr_eq(&Arc::downgrade(state))
    }
}

fn no_catalog() -> Error {
    Error::new(
        ErrorCode::RecoveryRequired,
        "no admitted history catalog binds the current state",
    )
}

fn history_not_found() -> Error {
    Error::new(
        ErrorCode::HistoryNotFound,
        "history is not present in the admitted catalog",
    )
}

impl Engine {
    fn bound_history_catalog(&self) -> Result<&HistoryCatalog> {
        self.history_catalog
            .as_ref()
            .filter(|catalog| catalog.binds(&self.state))
            .ok_or_else(no_catalog)
    }

    /// Retain the existing index cell only after proving its exact State binding.
    /// The caller must keep the captured State with this cell; plans-only
    /// rebinding is safe because plans do not contribute to history projection.
    pub(super) fn history_owner_cell(
        &self,
    ) -> Result<Arc<OnceLock<crate::state_projection::DerivedOwners>>> {
        Ok(Arc::clone(&self.bound_history_catalog()?.owners))
    }

    /// The exact published `history-manifest` hash of one history in the
    /// current admitted State's catalog.
    ///
    /// # Errors
    /// `RecoveryRequired` when no admitted catalog binds the current State
    /// (fresh pool, interrupted recovery); `HistoryNotFound` when the catalog
    /// has no entry; member read or object integrity errors from cold reads.
    pub fn history_root(&self, history_id: &str) -> Result<HistoryRoot> {
        let catalog = self.bound_history_catalog()?;
        match &catalog.source {
            CatalogSource::Published(root) => root
                .catalog_binding(history_id)
                .map(|binding| HistoryRoot::from_catalog(&binding.hash))
                .ok_or_else(history_not_found),
            CatalogSource::Recovered(reference) => {
                if let Some(root) = catalog
                    .memo
                    .lock()
                    .expect("invariant: history root memo lock poisoned")
                    .get(history_id)
                {
                    return Ok(root.clone());
                }
                let root = self.read_recovered_root(reference, history_id)?;
                let mut memo = catalog
                    .memo
                    .lock()
                    .expect("invariant: history root memo lock poisoned");
                if memo.len() >= MEMO_CAPACITY {
                    memo.clear();
                }
                memo.insert(history_id.to_owned(), root.clone());
                Ok(root)
            }
        }
    }

    fn read_recovered_root(
        &self,
        reference: &crate::metadata_map::Reference,
        history_id: &str,
    ) -> Result<HistoryRoot> {
        let members = self.members();
        let value = crate::metadata_map::lookup(reference, history_id, |kind, hash| {
            crate::publication_evidence::verified_graph_object_observed_at(
                &self.disk_resources,
                &members,
                kind,
                hash,
                &mut |error| self.metadata_integrity.fault(error),
            )
        })?;
        let value = value.ok_or_else(history_not_found)?;
        let hash = crate::history_package::catalog_entry(history_id, &value)?;
        Ok(HistoryRoot::from_catalog(hash))
    }

    /// History owning the file's smallest version id, or `None` for no version.
    ///
    /// Uses an already captured owner index only when it binds this exact State.
    /// Cold lookup keeps the original State scan: it performs no namespace I/O
    /// and does not move history admission ahead of file/repository resolution.
    /// This lookup grants no authorization.
    pub fn file_history_owner(&self, file_id: &str) -> Option<&str> {
        let version = match self.available_history_owners() {
            Some(owners) => self
                .state
                .versions
                .get(owners.first_version(file_id)?)
                .expect("invariant: captured owner membership references an existing version"),
            None => self
                .state
                .versions
                .values()
                .find(|version| version.file_id == file_id)?,
        };
        Some(&version.history_tree_id)
    }

    /// Borrow only an already captured derivation of this exact State; no I/O.
    pub(crate) fn available_history_owners(
        &self,
    ) -> Option<&crate::state_projection::DerivedOwners> {
        self.history_catalog
            .as_ref()
            .filter(|catalog| catalog.binds(&self.state))
            .and_then(|catalog| self.captured_history_owners(catalog))
    }

    fn captured_history_owners<'a>(
        &'a self,
        catalog: &'a HistoryCatalog,
    ) -> Option<&'a crate::state_projection::DerivedOwners> {
        self.published_state_base
            .as_ref()
            .filter(|base| base.names_captured_for(&self.state))
            .map(|base| base.history_owners())
            .or_else(|| catalog.owners.get())
    }

    /// Derived owner membership for the current State.
    ///
    /// Warm reads borrow the publication's captured index. A cold State (after
    /// recovery, or when the base's name capture does not match) builds the
    /// index once over State and its commit namespace objects and keeps it
    /// with this allocation's catalog, so later reads do not rebuild it.
    ///
    /// # Errors
    /// `RecoveryRequired` when no admitted catalog binds the current State.
    pub(crate) fn history_owners(&self) -> Result<&crate::state_projection::DerivedOwners> {
        // Even a warm owner index is usable only with this exact admitted
        // catalog. Plans-only rebinding below is the sole non-publication path.
        let catalog = self.bound_history_catalog()?;
        if let Some(owners) = self.captured_history_owners(catalog) {
            return Ok(owners);
        }
        let owners = self.with_namespace_source(|source| {
            crate::state_projection::DerivedOwners::from_state(&self.state, source)
        })?;
        // Concurrent successful readers may race to install the same
        // immutable derivation. Errors never poison/cache a partial index.
        Ok(catalog.owners.get_or_init(|| owners))
    }

    /// The durable local-plan writers change exactly `State::plans`, which is
    /// outside every history projection. Preserve the catalog/index receipt
    /// across Arc::make_mut's allocation change, but never repair an already
    /// unbound receipt or general State mutation.
    pub(crate) fn install_local_plan(&mut self, plan: storage_contract::Plan) {
        let bound = self
            .history_catalog
            .as_ref()
            .is_some_and(|catalog| catalog.binds(&self.state));
        if bound
            && let Some(base) = self
                .published_state_base
                .as_ref()
                .filter(|base| base.names_captured_for(&self.state))
        {
            let catalog = self
                .history_catalog
                .as_ref()
                .expect("invariant: catalog was bound");
            if catalog.owners.get().is_none() {
                let _ = catalog.owners.set(base.history_owners().clone());
            }
        }
        Arc::make_mut(&mut self.state)
            .plans
            .insert(plan.plan_id.clone(), plan);
        if bound {
            self.history_catalog
                .as_mut()
                .expect("invariant: catalog was bound")
                .state = Arc::downgrade(&self.state);
        }
    }
}
