//! Bounded resident history slots owned by the file model: one main window,
//! one branch page, one merge page and one auxiliary panel. Replacing a slot
//! drops the previous page; nothing accumulates across pages.
use super::{BranchPage, HistoryRoot, HistoryWindow, MergeInputsPage, MergePage, NodeField};
use super::{FieldSlice, ParentsPage};
use crate::{api, snapshot::Snapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StaleReason {
    /// The server reported `HISTORY_CHANGED`: the resident root is no longer
    /// the current root of this history.
    RootChanged,
}

/// The branch demand is consumed by its first attempt. Failures are retried
/// only by an explicit user action, never by the detail view's load effect.
#[derive(Clone, Debug, Default)]
pub(crate) enum BranchReadState {
    #[default]
    Unloaded,
    Loading,
    Loaded,
    Failed(api::Error),
}

/// Lifecycle of the main window. While a replacement is loading the previous
/// page stays readable (one old page plus one decode staging, never more);
/// installing the response drops it. A stale window stays readable but
/// closes every write entry that depended on it.
#[derive(Clone, Debug, Default)]
pub(crate) enum HistoryState {
    #[default]
    Unloaded,
    Loading {
        generation: u64,
        previous: Option<Snapshot<HistoryWindow>>,
    },
    Loaded(Snapshot<HistoryWindow>),
    Stale {
        window: Snapshot<HistoryWindow>,
        reason: StaleReason,
    },
    Failed(api::Error),
}
impl HistoryState {
    /// The resident page for display: current, stale, or the previous page
    /// while its replacement loads.
    pub(crate) fn window(&self) -> Option<&Snapshot<HistoryWindow>> {
        match self {
            Self::Loaded(window) | Self::Stale { window, .. } => Some(window),
            Self::Loading { previous, .. } => previous.as_ref(),
            Self::Unloaded | Self::Failed(_) => None,
        }
    }
    /// A current page: the only basis for write entry points.
    pub(crate) fn loaded(&self) -> Option<&Snapshot<HistoryWindow>> {
        match self {
            Self::Loaded(window) => Some(window),
            Self::Unloaded | Self::Loading { .. } | Self::Stale { .. } | Self::Failed(_) => None,
        }
    }
    pub(crate) fn root(&self) -> Option<&HistoryRoot> {
        self.loaded().map(|window| window.root())
    }
    pub(crate) fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. })
    }
    /// The generation of the read whose response will replace this state.
    pub(crate) fn loading_generation(&self) -> Option<u64> {
        match self {
            Self::Loading { generation, .. } => Some(*generation),
            Self::Unloaded | Self::Loaded(_) | Self::Stale { .. } | Self::Failed(_) => None,
        }
    }
    pub(crate) fn stale_reason(&self) -> Option<StaleReason> {
        match self {
            Self::Stale { reason, .. } => Some(*reason),
            Self::Unloaded | Self::Loading { .. } | Self::Loaded(_) | Self::Failed(_) => None,
        }
    }
    pub(crate) fn is_stale(&self) -> bool {
        self.stale_reason().is_some()
    }
}

/// At most one auxiliary page is open; opening another replaces it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AuxPanel {
    Parents {
        node_id: String,
        page: ParentsPage,
    },
    MergeSources {
        merge_id: String,
        page: MergeInputsPage,
    },
    Field {
        node_id: String,
        field: NodeField,
        slice: FieldSlice,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadSlot {
    Main,
    Branches,
    Panel,
    Merges,
}
impl ReadSlot {
    const ALL: [Self; 4] = [Self::Main, Self::Branches, Self::Panel, Self::Merges];
    fn index(self) -> usize {
        match self {
            Self::Main => 0,
            Self::Branches => 1,
            Self::Panel => 2,
            Self::Merges => 3,
        }
    }
}

#[derive(Clone, Default)]
struct Slot {
    cancellation: Option<api::ReadCancellation>,
    /// Incremented for every read started in this slot; a late response must
    /// still hold the ticket it was started with.
    ticket: u64,
}

/// One in-flight read per slot. Starting a new read aborts the previous.
#[derive(Clone, Default)]
pub(crate) struct HistoryReads {
    slots: [Slot; 4],
}
impl HistoryReads {
    /// Host tests cannot construct a browser `AbortController`; the fence
    /// logic is exercised with a read that has nothing to abort.
    pub(crate) fn begin_with(
        &mut self,
        slot: ReadSlot,
        cancellation: Option<api::ReadCancellation>,
    ) -> u64 {
        self.abort(slot);
        let entry = &mut self.slots[slot.index()];
        entry.cancellation = cancellation;
        entry.ticket += 1;
        entry.ticket
    }
    pub(crate) fn abort(&mut self, slot: ReadSlot) {
        let entry = &mut self.slots[slot.index()];
        if let Some(previous) = entry.cancellation.take() {
            previous.abort();
        }
        // A read whose owner cancelled it never settles, even if its
        // response races the abort.
        entry.ticket += 1;
    }
    pub(crate) fn finish(&mut self, slot: ReadSlot) {
        self.slots[slot.index()].cancellation = None;
    }
    pub(crate) fn abort_all(&mut self) {
        for slot in ReadSlot::ALL {
            self.abort(slot);
        }
    }
    pub(crate) fn ticket(&self, slot: ReadSlot) -> u64 {
        self.slots[slot.index()].ticket
    }
}

/// Everything the file model retains about the selected file's history.
#[derive(Clone, Default)]
pub(crate) struct History {
    pub(crate) state: HistoryState,
    pub(crate) branches: Option<BranchPage>,
    pub(crate) branch_read: BranchReadState,
    /// Position of the last branch request, retained for an exact retry.
    pub(crate) branch_cursor: Option<String>,
    pub(crate) panel: Option<AuxPanel>,
    pub(crate) merges: Option<MergePage>,
    pub(crate) reads: HistoryReads,
    /// Bumped whenever the resident window changes identity (new main read,
    /// context change, stale root, clear); late responses must still match.
    pub(crate) generation: u64,
}
impl History {
    pub(crate) fn window(&self) -> Option<&Snapshot<HistoryWindow>> {
        self.state.window()
    }
    pub(crate) fn loaded(&self) -> Option<&Snapshot<HistoryWindow>> {
        self.state.loaded()
    }
    pub(crate) fn root(&self) -> Option<&HistoryRoot> {
        self.state.root()
    }
    pub(crate) fn loading(&self) -> bool {
        self.state.is_loading()
    }
    pub(crate) fn stale(&self) -> bool {
        self.state.is_stale()
    }
    /// Branches of the resident root. A page captured under another root is
    /// not consulted even if it is still allocated.
    pub(crate) fn branch_page(&self) -> Option<&BranchPage> {
        let root = self.root()?;
        self.branches.as_ref().filter(|page| page.root() == root)
    }
    pub(crate) fn needs_branches(&self) -> bool {
        self.root().is_some()
            && self.branch_page().is_none()
            && matches!(self.branch_read, BranchReadState::Unloaded)
    }
    pub(crate) fn merge_page(&self) -> Option<&MergePage> {
        let root = self.root()?;
        self.merges.as_ref().filter(|page| page.root() == root)
    }
    /// The main window owns the requested alias/backup location. Branch
    /// candidates are file-wide and cannot override that contextual choice.
    pub(crate) fn selected_branch(&self) -> Option<&str> {
        self.window()
            .and_then(|window| window.context.selected_branch_id.as_deref())
    }

    fn drop_pages(&mut self) {
        self.branches = None;
        self.branch_read = BranchReadState::Unloaded;
        self.branch_cursor = None;
        self.panel = None;
        self.merges = None;
    }
    /// Forget every page and abort in-flight reads; the selection no longer
    /// describes this history (logout, other file, alias or backup change).
    pub(crate) fn clear(&mut self) {
        self.reads.abort_all();
        self.state = HistoryState::Unloaded;
        self.drop_pages();
        self.generation += 1;
    }
    /// The logical path changed but the content identity did not: the page
    /// stays readable, in-flight reads of the previous context are rejected.
    pub(crate) fn invalidate(&mut self) {
        self.reads.abort_all();
        self.generation += 1;
        if matches!(self.branch_read, BranchReadState::Loading) {
            self.branch_read = if self.branches.is_some() {
                BranchReadState::Loaded
            } else {
                BranchReadState::Unloaded
            };
        }
        if let HistoryState::Loading { .. } = self.state {
            let HistoryState::Loading { previous, .. } = std::mem::take(&mut self.state) else {
                unreachable!("invariant: matched the loading state above");
            };
            self.state = previous.map_or(HistoryState::Unloaded, HistoryState::Loaded);
        }
    }
    /// Keep the window readable but close every page that depended on the
    /// root being current. Drafts are not touched here; see the callers.
    pub(crate) fn mark_stale(&mut self, reason: StaleReason) {
        self.reads.abort_all();
        self.generation += 1;
        self.drop_pages();
        self.state = match std::mem::take(&mut self.state) {
            HistoryState::Loaded(window)
            | HistoryState::Stale { window, .. }
            | HistoryState::Loading {
                previous: Some(window),
                ..
            } => HistoryState::Stale { window, reason },
            // Without a resident page there is nothing to keep readable.
            HistoryState::Unloaded
            | HistoryState::Loading { previous: None, .. }
            | HistoryState::Failed(_) => HistoryState::Unloaded,
        };
    }
    /// Start a main read: every in-flight read is aborted, the previous page
    /// stays readable until the response arrives and the returned generation
    /// fences that response.
    pub(crate) fn begin_main(&mut self, cancellation: Option<api::ReadCancellation>) -> u64 {
        self.reads.abort_all();
        self.generation += 1;
        self.reads.begin_with(ReadSlot::Main, cancellation);
        let previous = self.state.window().cloned();
        self.state = HistoryState::Loading {
            generation: self.generation,
            previous,
        };
        self.drop_pages();
        self.generation
    }
    /// Install a new main page.
    pub(crate) fn install(&mut self, window: HistoryWindow) {
        self.reads.finish(ReadSlot::Main);
        self.state = HistoryState::Loaded(window.into());
        self.drop_pages();
    }
    pub(crate) fn fail(&mut self, error: api::Error) {
        self.reads.finish(ReadSlot::Main);
        self.state = HistoryState::Failed(error);
        self.drop_pages();
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/history/state_tests.rs"]
mod tests;
