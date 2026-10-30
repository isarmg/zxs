//! Bounded history projection over one admitted immutable metadata observation.
use super::{Engine, MetadataView, State};
use crate::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use storage_contract::history_window::*;
mod nodes;
mod projection;
mod query;
mod read;
mod selection;
pub use read::HistoryWindowRead;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/history_window_oracle_tests.rs"]
mod oracle_tests;

/// Create this signal before queuing a managed history read. All clones cancel
/// the same request, including initial capture and long single-record scans.
#[derive(Clone, Debug, Default)]
pub struct HistoryWindowCancellation(Arc<AtomicBool>);

impl HistoryWindowCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn checkpoint(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(Error::new(
                storage_contract::ErrorCode::OperationAborted,
                "history query cancelled",
            ));
        }
        Ok(())
    }
}

/// Logical root identity only. This retains neither disk access nor a GC lease.
pub(super) struct HistoryRootBindings {
    state: Weak<State>,
    roots: RootCatalog,
}

#[derive(Clone)]
enum RootCatalog {
    Published(crate::metadata_map::MapRoot),
    Recovered(Arc<BTreeMap<String, String>>),
}

impl HistoryRootBindings {
    pub(super) fn published(state: &Arc<State>, roots: crate::metadata_map::MapRoot) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::downgrade(state),
            roots: RootCatalog::Published(roots),
        })
    }

    pub(super) fn recovered(state: &Arc<State>, roots: BTreeMap<String, String>) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::downgrade(state),
            roots: RootCatalog::Recovered(Arc::new(roots)),
        })
    }

    fn get<'a>(&'a self, state: &Arc<State>, history: &str) -> Result<&'a str> {
        if !self.state.ptr_eq(&Arc::downgrade(state)) {
            return Err(missing_binding());
        }
        let root = match &self.roots {
            RootCatalog::Published(roots) => roots
                .catalog_binding(history)
                .map(|binding| binding.hash.as_str()),
            RootCatalog::Recovered(roots) => roots.get(history).map(String::as_str),
        };
        root.ok_or_else(missing_binding)
    }
}

fn missing_binding() -> Error {
    Error::new(
        storage_contract::ErrorCode::RecoveryRequired,
        "admitted metadata observation has no exact history-manifest root binding",
    )
}

impl MetadataView {
    /// Return the exact published history-manifest object hash for this State.
    /// The caller must authorize the target on the current request. This identity
    /// grants no disk-health authority, retained-root service or read lease.
    /// Missing publication/recovery evidence returns `RecoveryRequired`.
    pub fn history_root(&self, history: &str) -> Result<&str> {
        if !self.state.histories.contains_key(history) {
            return Err(Error::new(
                storage_contract::ErrorCode::HistoryNotFound,
                "history is absent from metadata observation",
            ));
        }
        self.history_window_roots
            .as_ref()
            .ok_or_else(missing_binding)?
            .get(&self.state, history)
    }
}

impl Engine {
    pub(crate) fn bind_recovered_history_roots(&mut self, roots: BTreeMap<String, String>) {
        self.history_window_roots = Some(HistoryRootBindings::recovered(&self.state, roots));
    }

    /// Local plans are excluded by the exhaustive history projection. Preserve
    /// that proof across this one known mutation instead of treating arbitrary
    /// new State allocations as if they had publication evidence.
    pub(crate) fn retain_local_plan(&mut self, plan: crate::Plan) {
        let roots = self
            .history_window_roots
            .as_ref()
            .filter(|binding| binding.state.ptr_eq(&Arc::downgrade(&self.state)))
            .map(|binding| binding.roots.clone());
        Arc::make_mut(&mut self.state)
            .plans
            .insert(plan.plan_id.clone(), plan);
        self.history_window_roots = roots.map(|roots| {
            Arc::new(HistoryRootBindings {
                state: Arc::downgrade(&self.state),
                roots,
            })
        });
    }
}
