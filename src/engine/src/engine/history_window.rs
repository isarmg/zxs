//! Bounded history projection over one admitted immutable metadata observation.
//!
//! Request-owned space is O(L + B + E + F): selected records, one content batch,
//! preview edges and one UTF-8 fragment, all bounded by shared policy. State and its
//! admitted root catalog are shared, not projected or copied for each request.
//! CPU is deliberately not O(L). With S total nodes, N scoped content records,
//! B the shared maximum node-page budget, H commit headers and C scoped commit
//! entries, cold node selection is O(S + ceil(N/B)*(H + C*log(B))), plus bounded
//! selector comparisons. Selected content reuses its first commit for fields
//! and path. Revision path/tag facts can walk ancestor chains; inherited path,
//! standalone fields and head lookup can scan merge/branch/commit records. Parent/source/input
//! reads scan their borrowed member arrays. Every such scan has cancellation
//! checkpoints. Publication, resident State and cold recovery remain unbounded
//! in total retained history, as before this read protocol.
use super::{Engine, MetadataView, State};
use crate::{Error, Result};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
pub use storage_contract::history_window::HistoryWindowCancellation;
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

/// Keep engine errors local while every transport/worker uses the shared token.
trait HistoryCancellationCheckpoint {
    fn checkpoint(&self) -> Result<()>;
}

impl HistoryCancellationCheckpoint for HistoryWindowCancellation {
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
