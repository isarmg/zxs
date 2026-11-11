use crate::api;
use crate::files::deletion::summary::{
    AuditPage, Model, Read, Target, decode, decode_error, invalid,
};
use crate::files::deletion::{Inspection, wire};
use crate::ui::{State, accept_read_response, fail};
use data_serialization::Deserialize;
use json_codec::Value;
use std::sync::Arc;
use storage_contract::file_deletion_review::{BranchPage, ParentPage, Summary};
use web_interface::{prelude::*, tasks::spawn_local};

pub(in crate::files::deletion) fn load(
    state: RwSignal<State>,
    selected: wire::Summary,
    epoch: u64,
    generation: u64,
) {
    state.update(|s| s.files.deletion.loading = true);
    spawn_local(async move {
        let endpoint = format!(
            "/api/files/{}/deletion/summary?deletion_node_id={}",
            selected.file_id, selected.deletion_node_id,
        );
        let result = api::validated_request("GET", &endpoint, None, |value| {
            decode(value, &selected)
        })
        .await;
        if !accept_read_response(state, epoch, &result)
            || !state.with_untracked(|s| s.files.deletion.accepts(generation))
        {
            return;
        }
        state.update(|s| s.files.deletion.loading = false);
        match result {
            Ok(summary) => show(state, epoch, generation, summary),
            Err(error) => fail(state, error),
        }
    });
}

fn show(state: RwSignal<State>, epoch: u64, generation: u64, summary: Summary) {
    let summary = Arc::new(summary);
    state.update(|s| {
        s.files.deletion.node = Some(Inspection::Paged(Model {
            summary: summary.clone(),
            page: AuditPage::Empty,
        }));
    });
    page(
        state,
        Target {
            epoch,
            generation,
            summary,
        },
        Read::Branches {
            after: None,
            start: 0,
        },
    );
}

impl Read {
    pub(super) fn endpoint(self, summary: &Summary) -> String {
        match self {
            Self::Branches { after, .. } => {
                let mut endpoint = format!(
                    "/api/files/{}/deletion/branches?deletion_node_id={}",
                    summary.file_id, summary.node_id,
                );
                if let Some(after) = after {
                    endpoint.push_str(&format!("&after={after}"));
                }
                endpoint
            }
            Self::Parents { offset } => format!(
                "/api/files/{}/deletion/parents?deletion_node_id={}&offset={offset}",
                summary.file_id, summary.node_id,
            ),
        }
    }
    pub(super) fn decode(self, value: Value, summary: &Summary) -> Result<AuditPage, api::Error> {
        match self {
            Self::Branches { start, .. } => {
                let page = BranchPage::deserialize(value).map_err(decode_error)?;
                if page.file_id != summary.file_id || page.deletion_node_id != summary.node_id {
                    return Err(invalid("the branch page belongs to another deletion"));
                }
                Ok(AuditPage::Branches {
                    start,
                    page: Arc::new(page),
                })
            }
            Self::Parents { offset } => {
                let page = ParentPage::deserialize(value).map_err(decode_error)?;
                if page.file_id != summary.file_id
                    || page.deletion_node_id != summary.node_id
                    || page.offset != offset
                {
                    return Err(invalid("the parent page belongs to another deletion or offset"));
                }
                Ok(AuditPage::Parents(Arc::new(page)))
            }
        }
    }
}

pub(super) fn page(state: RwSignal<State>, target: Target, read: Read) {
    let Some(target) = state
        .try_update(|s| target.begin(s))
        .expect("invariant: mounted audit view owns its state")
    else {
        return;
    };
    spawn_local(async move {
        let endpoint = read.endpoint(&target.summary);
        let result = api::validated_request("GET", &endpoint, None, |value| {
            read.decode(value, &target.summary)
        })
        .await;
        if !accept_read_response(state, target.epoch, &result)
            || !state.with_untracked(|s| target.matches(s))
        {
            return;
        }
        state.update(|s| s.files.deletion.loading = false);
        match result {
            Ok(page) => state.update(|s| {
                let model = s
                    .files
                    .deletion
                    .node
                    .as_mut()
                    .and_then(Inspection::paged_mut)
                    .expect("invariant: matched audit target retains its paged inspection");
                model.page = page;
            }),
            Err(error) => fail(state, error),
        }
    });
}
