//! Shared publication state for inline review panels. Unknown outcomes remain
//! tied to their original operation, and definitive conflicts preserve drafts.
use super::{
    actions,
    model::{Model, SaveStatus, publication_notice},
};
use crate::{
    api,
    ui::{State, current_session, fail},
};
use json_codec::{Value, json};
use std::collections::BTreeMap;
use web_interface::prelude::*;
use web_interface::tasks::spawn_local;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    Merge,
    Tags,
    Backup,
}
#[derive(Clone, Default)]
pub(crate) struct Publication {
    pub operation_id: Option<String>,
    pub error: String,
}
pub(crate) type Publications = BTreeMap<Kind, Publication>;

pub(crate) fn error(state: RwSignal<State>, kind: Kind, message: String) {
    state.update(|s| s.files.publications.entry(kind).or_default().error = message);
}
pub(crate) fn reset(state: RwSignal<State>, kind: Kind) {
    state.update(|s| {
        s.files.publications.remove(&kind);
    });
}
pub(crate) fn error_text(state: &State, kind: Kind) -> String {
    state
        .files
        .publications
        .get(&kind)
        .map(|publication| publication.error.clone())
        .unwrap_or_default()
}
pub(crate) fn settle(model: &mut Model, id: &str, status: SaveStatus) {
    let kinds = model
        .publications
        .iter()
        .filter(|(_, publication)| publication.operation_id.as_deref() == Some(id))
        .map(|(kind, _)| *kind)
        .collect::<Vec<_>>();
    for kind in kinds {
        match status {
            SaveStatus::Published | SaveStatus::Degraded => {
                match kind {
                    Kind::Merge => model.merge = None,
                    Kind::Backup => model.backup = None,
                    Kind::Tags => {
                        model.tag_edit = None;
                        model.checked_nodes.clear();
                    }
                }
                model.publications.remove(&kind);
            }
            SaveStatus::Rejected => {
                if let Some(publication) = model.publications.get_mut(&kind) {
                    publication.operation_id = None;
                    publication.error = publication_notice(status);
                }
            }
            SaveStatus::Unknown => {}
        }
    }
}
pub(crate) fn submit(
    state: RwSignal<State>,
    kind: Kind,
    method: &'static str,
    endpoint: String,
    mut body: Value,
) {
    if state.with_untracked(|s| {
        !actions::write_allowed(s)
            || kind != Kind::Backup && super::model::selected_entry_read_only(&s.files)
            || s.files
                .publications
                .get(&kind)
                .is_some_and(|publication| publication.operation_id.is_some())
    }) {
        return;
    }
    let Some(id) = crate::ui::operation_id(state) else {
        return;
    };
    body["operation_id"] = id.clone().into();
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| {
        let publication = s.files.publications.entry(kind).or_default();
        publication.operation_id = Some(id.clone());
        publication.error.clear();
        s.files.mutation_pending = true;
    });
    spawn_local(async move {
        let result = api::request(method, &endpoint, Some(body)).await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => {
                actions::record_receipt(state, &id, value);
            }
            Err(error) => {
                let unknown = error.mutation_outcome_unknown();
                if unknown {
                    actions::record_operation(state, json!({"operation_id":id,"state":"in_doubt"}));
                }
                state.update(|s| {
                    if let Some(publication) = s
                        .files
                        .publications
                        .get_mut(&kind)
                        .filter(|publication| publication.operation_id.as_deref() == Some(&id))
                    {
                        publication.error = error.to_string();
                        if !unknown {
                            publication.operation_id = None;
                        }
                    }
                });
                fail(state, error);
            }
        }
        state.update(|s| s.files.mutation_pending = false);
    });
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/review/tests.rs"]
mod tests;
