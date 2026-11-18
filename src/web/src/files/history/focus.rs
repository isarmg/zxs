//! A locate action belongs to one installed read, not a DOM id or the next refresh.
use crate::{files::actions, ui::State};
use storage_contract::history_window::{HistoryNodeId, HistoryPageIdentity};
use web_interface::{html, prelude::*};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Anchor {
    node: HistoryNodeId,
    identity: HistoryPageIdentity,
    epoch: u64,
    navigation: u64,
    read: u64,
    entry: String,
}

impl Anchor {
    pub(in crate::files) fn from_page(state: &State, read: u64, node: &HistoryNodeId) -> Option<Self> {
        let identity = state.files.file_history.identity()?;
        let entry = state.files.selected.as_ref()?;
        if state.files.history_generation != read || !actions::history_navigation_ready(state)
            || entry.file_id() != identity.file_id.as_str()
            || !state.files.file_history.nodes().iter().any(|row| row.id() == node.as_str()) {
            return None;
        }
        Some(Self {
            node: node.clone(), identity: identity.clone(), epoch: state.session_epoch,
            navigation: state.files.generation, read, entry: entry.entry_id().into(),
        })
    }

    pub(in crate::files) fn current(&self, state: &State) -> bool {
        state.session_epoch == self.epoch && state.files.generation == self.navigation
            && state.files.history_generation == self.read && actions::history_navigation_ready(state)
            && state.files.file_history.identity() == Some(&self.identity)
            && state.files.selected.as_ref().is_some_and(|entry|
                entry.entry_id() == self.entry && entry.file_id() == self.identity.file_id.as_str())
    }
}

/// Only the mounted original card owns this reference. User text, another FILE
/// or a shared-history card with the same node string cannot receive its focus.
pub(in crate::files) fn card(
    state: RwSignal<State>, request: Memo<Option<Anchor>>, id: StoredValue<String>,
) -> NodeRef<html::Article> {
    let card = NodeRef::<html::Article>::new();
    Effect::new(move |_| {
        let Some(anchor) = request.get() else { return };
        if !id.with_value(|id| id == anchor.node.as_str()) || card.get().is_none() { return; }
        request_animation_frame(move || {
            let Some(element) = card.try_get_untracked().flatten() else { return };
            let permitted = state.try_update(|s| {
                if s.files.history_focus.as_ref() != Some(&anchor) { return false; }
                s.files.history_focus = None;
                anchor.current(s)
            }).unwrap_or(false);
            if permitted && element.is_connected() {
                // Native focus scrolls into view once, after the new card mounted.
                if let Err(error) = element.focus() {
                    web_sys::console::warn_2(&"Could not focus the selected history card".into(), &error);
                }
            }
        });
    });
    card
}
