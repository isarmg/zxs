use super::*;
use crate::files::history::{DecodeError, reads::Resource};
use crate::ui::{ErrorContext, accept_read_response, fail_in};
use data_serialization::Deserialize;
use storage_contract::file_history_merge::{FileHistorySource, FileHistorySourceRequest};
use web_interface::tasks::spawn_local;

#[derive(Clone)]
struct Target {
    epoch: u64,
    instance: u64,
    file: String,
    root: String,
    node: ChosenNode,
    label: String,
}
impl Target {
    fn capture(state: &State, instance: u64, id: &str) -> Option<Self> {
        if !actions::history_write_allowed(state) || actions::history_aux_loading(state) {
            return None;
        }
        let draft = state
            .files
            .merge
            .as_ref()
            .filter(|draft| draft.instance == instance && draft.historical())?;
        if state.files.file_history.root() != Some(draft.root()) {
            return None;
        }
        let node = state.files.file_history.node(id)?;
        Some(Self {
            epoch: state.session_epoch,
            instance,
            file: draft.file.clone(),
            root: draft.root.clone(),
            node: ChosenNode::capture(node)?,
            label: node_label(node, 0),
        })
    }
    fn owner(&self) -> ReviewOwner {
        ReviewOwner {
            kind: review::Kind::Merge,
            instance: self.instance,
        }
    }
    fn current(&self, state: &State, generation: u64) -> bool {
        state.session_epoch == self.epoch
            && actions::history_write_allowed(state)
            && state.files.file_history.root() == Some(self.root.as_str())
            && state.files.merge.as_ref().is_some_and(|draft| {
                draft.instance == self.instance
                    && draft.file == self.file
                    && draft.root == self.root
                    && draft.historical()
            })
            && state
                .files
                .history_aux
                .current_review(self.owner(), generation)
    }
}
fn endpoint(request: &FileHistorySourceRequest) -> String {
    let query = request
        .query_fields()
        .into_iter()
        .map(|(key, value)| format!("{}={}", api::segment(&key), api::segment(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{query}", request.route())
}
fn parse(value: Value, request: &FileHistorySourceRequest) -> Result<SelectedBranch, api::Error> {
    let source = FileHistorySource::deserialize(value).map_err(DecodeError::Wire)?;
    source.validate(request).map_err(DecodeError::Name)?;
    let branch = source.branch;
    let head = branch
        .node_id
        .expect("invariant: admitted merge source has a current branch head");
    Ok(SelectedBranch {
        id: branch.branch_id.to_string(),
        name: branch.name,
        expected: head.to_string(),
    })
}
pub(super) fn resolve(state: RwSignal<State>, instance: u64, id: &str) {
    let Some(target) = state.with_untracked(|state| Target::capture(state, instance, id)) else {
        return;
    };
    let request = FileHistorySourceRequest::parse(&target.file, &target.node.id, &target.root)
        .map_err(|source| api::Error::from(DecodeError::Name(source)));
    let (request, cancellation) = match request.and_then(|request| {
        api::ReadCancellation::new().map(|cancellation| (request, cancellation))
    }) {
        Ok(values) => values,
        Err(error) => {
            fail_in(
                state,
                target.epoch,
                ErrorContext::Read("historical merge source".into()),
                error,
            );
            return;
        }
    };
    let path = endpoint(&request);
    let generation = state
        .try_update(|state| {
            let generation = state.files.history_aux.begin_review(target.owner());
            assert!(
                state.files.history_aux.replace_owner(
                    Resource::Review,
                    generation,
                    cancellation.clone()
                ),
                "invariant: new source read owns its review slot"
            );
            generation
        })
        .expect("invariant: a mounted merge owns its state");
    spawn_local(async move {
        let result = api::cancellable_get(&path, cancellation)
            .await
            .and_then(|value| parse(value, &request));
        if !accept_read_response(state, target.epoch, &result) {
            return;
        }
        let epoch = target.epoch;
        let completion = state
            .try_update(|state| {
                if !target.current(state, generation) {
                    return None;
                }
                state
                    .files
                    .history_aux
                    .complete(Resource::Review, generation);
                Some(match result {
                    Ok(branch) => graph::insert_resolved_source(
                        state,
                        target.instance,
                        target.node,
                        target.label,
                        branch,
                    )
                    .map_err(|message| {
                        api::error(storage_contract::ErrorCode::InvalidRequest, message)
                    }),
                    Err(error) => Err(error),
                })
            })
            .flatten();
        if let Some(Err(error)) = completion {
            review::error(
                state,
                review::Kind::Merge,
                if error.code == storage_contract::ErrorCode::FileBranchRequired {
                    t(
                        "此历史节点尚无独立来源分支。请先点击该节点的‘创建分支’，再选为合并输入。",
                        "This historical node has no independent source branch. Use Create branch on that node, then select it as a merge input.",
                    )
                } else {
                    error.to_string()
                },
            );
            fail_in(state, epoch, ErrorContext::Read(path), error);
        }
    });
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/merge/source_tests.rs"]
mod tests;
