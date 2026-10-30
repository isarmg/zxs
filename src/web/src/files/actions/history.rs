//! Current-root history reads own bounded pages and reject obsolete completions.
use crate::{
    api,
    files::{
        history::{
            self, FileHistory,
            reads::{Resource, ReviewOwner},
        },
        inline,
        merge::selection::{ChosenNode, ReviewInventory, SelectedBranch},
        page::Entry,
        review,
        tags::SelectedNode,
    },
    ui::{ErrorContext, State, accept_read_response, fail_in},
};
use std::{collections::BTreeSet, sync::Arc};
use storage_contract::history_window::{
    HistoryCollection, HistoryCursor, HistoryField, HistoryFileId, HistoryLocation, HistoryMergeId,
    HistoryNodeId, HistoryPageLimit, HistoryPosition, HistoryRepositoryId, HistoryResolvedContext,
    HistoryRoot, HistoryVersionId, HistoryWindowPage, HistoryWindowRequest,
};
use web_interface::{i18n::t, prelude::*, tasks::spawn_local};

#[derive(Clone)]
struct Target {
    epoch: u64,
    slot: String,
    directory: String,
    entry: Entry,
    basis: Option<String>,
    resolved: Option<HistoryResolvedContext>,
}

#[derive(Clone, Copy)]
enum NodeRead {
    Refresh,
    Page,
    Anchor,
    BackupAnchor,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active() {
            return None;
        }
        let entry = state.files.selected.as_ref()?;
        if !super::super::model::file_kind(entry) {
            return None;
        }
        Some(Self {
            epoch: state.session_epoch,
            slot: state.files.slot.clone(),
            directory: state.files.directory.clone(),
            entry: entry.clone(),
            basis: state.files.file_history.root().map(str::to_owned),
            resolved: state
                .files
                .file_history
                .identity()
                .map(|identity| identity.context.clone()),
        })
    }

    fn current(&self, state: &State) -> bool {
        state.session_active()
            && state.session_epoch == self.epoch
            && state.files.slot == self.slot
            && state.files.directory == self.directory
            && state.files.file_history.root() == self.basis.as_deref()
            && state
                .files
                .selected
                .as_ref()
                .is_some_and(|entry| same_target(entry, &self.entry))
    }
}

fn same_target(current: &Entry, requested: &Entry) -> bool {
    current.entry_id() == requested.entry_id()
        && current.file_id() == requested.file_id()
        && current.path() == requested.path()
        && current.context() == requested.context()
        && current.backup() == requested.backup()
        && current.node_id() == requested.node_id()
        && current.version_id() == requested.version_id()
        && current.selected_branch_id() == requested.selected_branch_id()
}

fn invalid_request(message: &str) -> api::Error {
    local_error(storage_contract::Error::new(
        storage_contract::ErrorCode::InvalidRequest,
        message,
    ))
}

fn local_error(source: storage_contract::Error) -> api::Error {
    api::Error {
        status: 400,
        code: source.code(),
        message: source.message().into_owned(),
        retryable: false,
        request_id: None,
        source: Some(Arc::new(api::ErrorSource::FileHistory(
            history::DecodeError::Name(source),
        ))),
    }
}

fn make_request(
    target: &Target,
    collection: HistoryCollection,
    cursor: Option<&str>,
    anchor: Option<&str>,
    rooted: bool,
) -> Result<HistoryWindowRequest, api::Error> {
    if cursor.is_some() && anchor.is_some() {
        return Err(invalid_request(
            "A history anchor and cursor cannot be combined.",
        ));
    }
    let file_id = HistoryFileId::parse(target.entry.file_id()).map_err(local_error)?;
    let location = super::super::model::file_context(&target.entry)
        .map(|(repository, path)| {
            let repository = HistoryRepositoryId::parse(&repository).map_err(local_error)?;
            let path = name_policy::LookupPath::parse(&path)
                .map_err(|source| api::Error::from(history::DecodeError::Path(source)))?;
            Ok::<HistoryLocation, api::Error>(HistoryLocation::new(repository, path))
        })
        .transpose()?;
    let history_root = if rooted {
        Some(
            HistoryRoot::parse(target.basis.as_deref().ok_or_else(|| {
                invalid_request("Load a current history root before continuing.")
            })?)
            .map_err(local_error)?,
        )
    } else {
        None
    };
    if anchor.is_some() && history_root.is_none() {
        return Err(invalid_request(
            "An exact history anchor requires a current root.",
        ));
    }
    let cursor = cursor
        .map(HistoryCursor::parse)
        .transpose()
        .map_err(local_error)?;
    let limit = cursor
        .as_ref()
        .map(|cursor| cursor.payload().limit)
        .unwrap_or_else(|| HistoryPageLimit::default_for(&collection));
    let anchor_node_id = anchor
        .map(HistoryNodeId::parse)
        .transpose()
        .map_err(local_error)?;
    let request = HistoryWindowRequest {
        file_id,
        collection,
        location,
        history_root,
        cursor,
        anchor_node_id,
        limit,
    };
    if let Some(cursor) = &request.cursor {
        let payload = cursor.payload();
        let resolved = target
            .resolved
            .as_ref()
            .ok_or_else(|| invalid_request("The cursor has no current history context."))?;
        if payload.file_id != request.file_id
            || payload.collection != request.collection
            || Some(payload.history_root) != request.history_root
            || payload.history_tree_id != resolved.history_tree_id
            || payload.query_hash != request.query_hash(resolved, payload.anchor_node_id.as_ref())
        {
            return Err(invalid_request(
                "The history cursor belongs to another query.",
            ));
        }
    }
    Ok(request)
}

fn field_token(field: HistoryField) -> &'static str {
    match field {
        HistoryField::Message => "message",
        HistoryField::Author => "author",
        HistoryField::CreatedAt => "created_at",
    }
}

fn endpoint(request: &HistoryWindowRequest) -> String {
    let file = api::segment(request.file_id.as_str());
    let base = format!("/api/file-manager/files/{file}/history");
    let mut endpoint = match &request.collection {
        HistoryCollection::Nodes => base,
        HistoryCollection::Versions => format!("/api/files/{file}/history"),
        HistoryCollection::Branches => format!("{base}/branches"),
        HistoryCollection::Merges => format!("{base}/merges"),
        HistoryCollection::NodeParents { node_id } => {
            format!("{base}/nodes/{}/parents", api::segment(node_id.as_str()))
        }
        HistoryCollection::VersionSources { version_id } => format!(
            "/api/files/{file}/history/versions/{}/sources",
            api::segment(version_id.as_str())
        ),
        HistoryCollection::MergeInputs { merge_id } => {
            format!("{base}/merges/{}/inputs", api::segment(merge_id.as_str()))
        }
        HistoryCollection::MergeParents { merge_id } => {
            format!("{base}/merges/{}/parents", api::segment(merge_id.as_str()))
        }
        HistoryCollection::NodeField { node_id, field } => format!(
            "{base}/nodes/{}/fields/{}",
            api::segment(node_id.as_str()),
            field_token(*field)
        ),
    };
    endpoint.push_str(&format!("?limit={}", request.limit.get()));
    if let Some(location) = &request.location {
        endpoint.push_str(&format!(
            "&repository_id={}&path={}",
            api::segment(location.repository_id.as_str()),
            api::segment(location.path.as_str())
        ));
    }
    if let Some(root) = request.history_root {
        endpoint.push_str(&format!(
            "&history_root={}",
            api::segment(&root.to_string())
        ));
    }
    if let Some(cursor) = &request.cursor {
        endpoint.push_str(&format!("&cursor={}", api::segment(cursor.as_str())));
    }
    if let Some(anchor) = &request.anchor_node_id {
        endpoint.push_str(&format!(
            "&anchor_node_id={}",
            api::segment(anchor.as_str())
        ));
    }
    endpoint
}

async fn fetch_page(
    target: &Target,
    request: &HistoryWindowRequest,
    cancellation: api::ReadCancellation,
) -> Result<HistoryWindowPage, api::Error> {
    api::cancellable_get(&endpoint(request), cancellation)
        .await
        .and_then(|value| {
            history::admit(
                value,
                request,
                request.history_root.and(target.resolved.as_ref()),
            )
        })
}

fn navigation_allowed(state: RwSignal<State>) -> bool {
    inline::history_navigation_allowed(state)
        && state.with_untracked(super::history_navigation_ready)
}

fn report_local(state: RwSignal<State>, target: &Target, error: api::Error) {
    fail_in(
        state,
        target.epoch,
        ErrorContext::Read(format!(
            "/api/file-manager/files/{}/history",
            api::segment(target.entry.file_id())
        )),
        error,
    );
}

pub(crate) fn load_file_history(state: RwSignal<State>) {
    // This also runs after selection/listing refresh. An old editor owner may
    // still be cleaning up, so the completion decides whether a window can move.
    let Some(target) = state.with_untracked(Target::capture) else {
        return;
    };
    begin_nodes(state, target, None, None, NodeRead::Refresh);
}

pub(crate) fn page_history_nodes(state: RwSignal<State>, next: bool) {
    if !navigation_allowed(state) {
        return;
    }
    let cursor = state.with_untracked(|s| {
        let cursor = if next {
            s.files.file_history.next_cursor()
        } else {
            s.files.file_history.previous_cursor()
        };
        cursor.map(str::to_owned)
    });
    let Some(cursor) = cursor else {
        return;
    };
    if let Some(target) = state.with_untracked(Target::capture) {
        begin_nodes(state, target, Some(cursor), None, NodeRead::Page);
    }
}

pub(crate) fn anchor_history_node(state: RwSignal<State>, id: String) {
    if !navigation_allowed(state) {
        return;
    }
    if let Some(target) = state.with_untracked(Target::capture) {
        begin_nodes(state, target, None, Some(id), NodeRead::Anchor);
    }
}

fn begin_nodes(
    state: RwSignal<State>,
    target: Target,
    cursor: Option<String>,
    anchor: Option<String>,
    kind: NodeRead,
) {
    let fresh = matches!(kind, NodeRead::Refresh);
    let request = match make_request(
        &target,
        HistoryCollection::Nodes,
        cursor.as_deref(),
        anchor.as_deref(),
        !fresh,
    ) {
        Ok(request) => request,
        Err(error) => {
            report_local(state, &target, error);
            return;
        }
    };
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => {
            report_local(state, &target, error);
            return;
        }
    };
    let generation = state
        .try_update(|s| {
            if !target.current(s) {
                return None;
            }
            s.files.cancel_history();
            s.files.history_loading = true;
            s.files.history_cancellation = Some(cancellation.clone());
            Some(s.files.history_generation)
        })
        .flatten();
    let Some(generation) = generation else {
        return;
    };
    spawn_local(async move {
        let result = fetch_page(&target, &request, cancellation).await;
        if !accept_read_response(state, target.epoch, &result) {
            return;
        }
        let completion =
            state.try_update(|s| complete_nodes(s, &target, generation, result, !fresh));
        match completion {
            Some(Ok(true)) => {
                let backup_anchor = if fresh {
                    state.with_untracked(|s| backup_anchor(s, &target.entry))
                } else {
                    None
                };
                if let Some(id) = backup_anchor {
                    if let Some(next) = state.with_untracked(Target::capture)
                        && same_target(&next.entry, &target.entry)
                    {
                        begin_nodes(state, next, None, Some(id), NodeRead::BackupAnchor);
                    }
                } else if fresh || matches!(kind, NodeRead::BackupAnchor) {
                    begin_auxiliary(state, HistoryCollection::Branches, None, false);
                }
            }
            Some(Err(error)) => fail_in(
                state,
                target.epoch,
                ErrorContext::Read(endpoint(&request)),
                error,
            ),
            Some(Ok(_)) | None => {}
        }
    });
}

fn backup_anchor(state: &State, entry: &Entry) -> Option<String> {
    if !entry.backup() {
        return None;
    }
    let stored = entry.node_id()?;
    (!state
        .files
        .file_history
        .nodes()
        .iter()
        .any(|node| node.id() == stored))
    .then(|| stored.to_owned())
}

fn complete_nodes(
    state: &mut State,
    target: &Target,
    generation: u64,
    result: Result<HistoryWindowPage, api::Error>,
    replace: bool,
) -> Result<bool, api::Error> {
    if !target.current(state) || state.files.history_generation != generation {
        return Ok(false);
    }
    if result.is_ok() && state.files.history_editors > 0 && state.files.file_history.loaded() {
        state.files.history_loading = false;
        state.files.history_cancellation = None;
        state.files.file_history.invalidate();
        state.notice = t(
            "请完成或取消当前编辑，然后刷新历史。",
            "Finish or cancel the current edit, then refresh history.",
        );
        return Ok(false);
    }
    let result = result.and_then(|page| {
        let mut history = (*state.files.file_history).clone();
        history
            .install_nodes(page, &target.entry, replace)
            .map_err(api::Error::from)?;
        Ok(history)
    });
    complete_history_load(
        state,
        target.epoch,
        generation,
        target.entry.file_id(),
        result,
    )?;
    Ok(true)
}

pub(crate) fn complete_history_load(
    state: &mut State,
    epoch: u64,
    generation: u64,
    file: &str,
    result: Result<FileHistory, api::Error>,
) -> Result<(), api::Error> {
    if !state.session_active()
        || state.session_epoch != epoch
        || state.files.history_generation != generation
        || !state
            .files
            .selected
            .as_ref()
            .is_some_and(|entry| entry.file_id() == file)
    {
        return Ok(());
    }
    state.files.history_loading = false;
    state.files.history_cancellation = None;
    match result {
        Ok(history) => {
            state.files.file_history = history.into();
            Ok(())
        }
        Err(error) => {
            state.files.file_history.invalidate();
            state.files.history_aux.cancel_all();
            Err(error)
        }
    }
}

fn resource(collection: &HistoryCollection) -> Resource {
    match collection {
        HistoryCollection::Branches => Resource::Branches,
        HistoryCollection::Merges => Resource::Merges,
        HistoryCollection::NodeField { .. } => Resource::Field,
        HistoryCollection::NodeParents { .. }
        | HistoryCollection::VersionSources { .. }
        | HistoryCollection::MergeInputs { .. }
        | HistoryCollection::MergeParents { .. } => Resource::Relation,
        HistoryCollection::Nodes | HistoryCollection::Versions => {
            unreachable!("invariant: primary histories use their own loader")
        }
    }
}

fn begin_auxiliary(
    state: RwSignal<State>,
    collection: HistoryCollection,
    cursor: Option<String>,
    explicit: bool,
) {
    if explicit && !navigation_allowed(state) {
        return;
    }
    let Some(target) = state.with_untracked(|s| {
        s.files
            .file_history
            .ready()
            .then(|| Target::capture(s))
            .flatten()
    }) else {
        return;
    };
    let request = match make_request(&target, collection, cursor.as_deref(), None, true) {
        Ok(request) => request,
        Err(error) => {
            report_local(state, &target, error);
            return;
        }
    };
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => {
            report_local(state, &target, error);
            return;
        }
    };
    let resource = resource(&request.collection);
    let read = state
        .try_update(|s| {
            if !target.current(s) || !s.files.file_history.ready() {
                return None;
            }
            let expected_total = expected_field_total(&s.files.file_history, &request.collection);
            let generation = s
                .files
                .history_aux
                .begin(resource, Some(cancellation.clone()));
            Some((generation, expected_total))
        })
        .flatten();
    let Some((generation, expected_total)) = read else {
        return;
    };
    spawn_local(async move {
        let result = fetch_page(&target, &request, cancellation).await;
        if !accept_read_response(state, target.epoch, &result) {
            return;
        }
        let completion = state.try_update(|s| {
            complete_auxiliary(s, &target, resource, generation, expected_total, result)
        });
        if let Some(Err(error)) = completion {
            fail_in(
                state,
                target.epoch,
                ErrorContext::Read(endpoint(&request)),
                error,
            );
        }
    });
}

fn expected_field_total(history: &FileHistory, collection: &HistoryCollection) -> Option<u64> {
    let HistoryCollection::NodeField { node_id, field } = collection else {
        return None;
    };
    history
        .node(node_id.as_str())
        .map(|node| node.field_bytes(*field))
        .or_else(|| {
            history
                .fragment()
                .filter(|fragment| fragment.node == node_id.as_str() && fragment.field == *field)
                .map(|fragment| fragment.total)
        })
}

fn complete_auxiliary(
    state: &mut State,
    target: &Target,
    resource: Resource,
    generation: u64,
    expected_total: Option<u64>,
    result: Result<HistoryWindowPage, api::Error>,
) -> Result<(), api::Error> {
    if !target.current(state)
        || !state.files.file_history.ready()
        || !state.files.history_aux.current(resource, generation)
    {
        return Ok(());
    }
    let result = result.and_then(|page| {
        if let HistoryWindowPage::NodeField { total_bytes, .. } = &page
            && expected_total.is_some_and(|expected| expected != *total_bytes)
        {
            return Err(history::DecodeError::Relation(
                "history field total differs from its admitted metadata or previous fragment",
            )
            .into());
        }
        Ok(page)
    });
    state.files.history_aux.complete(resource, generation);
    match result.and_then(|page| {
        state
            .files
            .file_history
            .install_auxiliary(page)
            .map_err(api::Error::from)
    }) {
        Ok(()) => Ok(()),
        Err(error) => {
            state.files.file_history.invalidate();
            state.files.history_aux.cancel_all();
            Err(error)
        }
    }
}

pub(crate) fn page_history_branches(state: RwSignal<State>, next: bool) {
    if !navigation_allowed(state) {
        return;
    }
    let cursor = state.with_untracked(|s| {
        let cursor = if next {
            s.files.file_history.next_branch_cursor()
        } else {
            s.files.file_history.previous_branch_cursor()
        };
        cursor.map(str::to_owned)
    });
    if let Some(cursor) = cursor {
        begin_auxiliary(state, HistoryCollection::Branches, Some(cursor), false);
    }
}

pub(crate) fn load_history_merges(state: RwSignal<State>, cursor: Option<String>) {
    begin_auxiliary(state, HistoryCollection::Merges, cursor, true);
}

pub(crate) fn open_history_parents(state: RwSignal<State>, node: String, cursor: Option<String>) {
    if !navigation_allowed(state) {
        return;
    }
    match HistoryNodeId::parse(&node) {
        Ok(node_id) => begin_auxiliary(
            state,
            HistoryCollection::NodeParents { node_id },
            cursor,
            false,
        ),
        Err(error) => {
            if let Some(target) = state.with_untracked(Target::capture) {
                report_local(state, &target, local_error(error));
            }
        }
    }
}

pub(crate) fn open_history_merge_inputs(
    state: RwSignal<State>,
    merge: String,
    cursor: Option<String>,
) {
    open_merge_relation(state, merge, cursor, true);
}
pub(crate) fn open_history_merge_parents(
    state: RwSignal<State>,
    merge: String,
    cursor: Option<String>,
) {
    open_merge_relation(state, merge, cursor, false);
}
fn open_merge_relation(
    state: RwSignal<State>,
    merge: String,
    cursor: Option<String>,
    inputs: bool,
) {
    if !navigation_allowed(state) {
        return;
    }
    match HistoryMergeId::parse(&merge) {
        Ok(merge_id) => begin_auxiliary(
            state,
            if inputs {
                HistoryCollection::MergeInputs { merge_id }
            } else {
                HistoryCollection::MergeParents { merge_id }
            },
            cursor,
            false,
        ),
        Err(error) => {
            if let Some(target) = state.with_untracked(Target::capture) {
                report_local(state, &target, local_error(error));
            }
        }
    }
}
pub(crate) fn open_history_version_sources(
    state: RwSignal<State>,
    version: String,
    cursor: Option<String>,
) {
    if !navigation_allowed(state) {
        return;
    }
    match HistoryVersionId::parse(&version) {
        Ok(version_id) => begin_auxiliary(
            state,
            HistoryCollection::VersionSources { version_id },
            cursor,
            false,
        ),
        Err(error) => {
            if let Some(target) = state.with_untracked(Target::capture) {
                report_local(state, &target, local_error(error));
            }
        }
    }
}
pub(crate) fn load_history_field(
    state: RwSignal<State>,
    node: String,
    field: HistoryField,
    cursor: Option<String>,
) {
    if !navigation_allowed(state) {
        return;
    }
    match HistoryNodeId::parse(&node) {
        Ok(node_id) => begin_auxiliary(
            state,
            HistoryCollection::NodeField { node_id, field },
            cursor,
            false,
        ),
        Err(error) => {
            if let Some(target) = state.with_untracked(Target::capture) {
                report_local(state, &target, local_error(error));
            }
        }
    }
}
pub(crate) fn close_history_relation(state: RwSignal<State>) {
    state.update(|s| {
        s.files.history_aux.cancel(Resource::Relation);
        s.files.file_history.close_relation();
    });
}
pub(crate) fn close_history_field(state: RwSignal<State>) {
    state.update(|s| {
        s.files.history_aux.cancel(Resource::Field);
        s.files.file_history.close_fragment();
    });
}
pub(crate) fn history_aux_loading(state: &State) -> bool {
    state.files.history_aux.loading()
}

enum ReviewNodes {
    Merge(Vec<ChosenNode>),
    Tags(Vec<SelectedNode>),
}
struct ReviewFacts {
    nodes: ReviewNodes,
    branches: Vec<SelectedBranch>,
    max_entries: usize,
}

pub(crate) fn review_history_merge(state: RwSignal<State>, instance: u64) {
    begin_review(
        state,
        ReviewOwner {
            kind: review::Kind::Merge,
            instance,
        },
    );
}
pub(crate) fn review_history_tags(state: RwSignal<State>, instance: u64) {
    begin_review(
        state,
        ReviewOwner {
            kind: review::Kind::Tags,
            instance,
        },
    );
}

fn inventory(state: &State, kind: review::Kind) -> Option<ReviewInventory> {
    match kind {
        review::Kind::Merge => Some(state.files.merge.as_ref()?.review_inventory()),
        review::Kind::Tags => Some(state.files.tag_edit.as_ref()?.review_inventory()),
        review::Kind::Backup => {
            unreachable!("invariant: backup drafts have no history selection review")
        }
    }
}

fn review_current(state: &State, target: &Target, generation: u64, owner: ReviewOwner) -> bool {
    let instance = match owner.kind {
        review::Kind::Merge => state.files.merge.as_ref().map(|draft| draft.instance()),
        review::Kind::Tags => state.files.tag_edit.as_ref().map(|draft| draft.instance()),
        review::Kind::Backup => None,
    };
    target.current(state)
        && state.files.file_history.ready()
        && state.files.history_aux.current_review(owner, generation)
        && instance == Some(owner.instance)
}

fn begin_review(state: RwSignal<State>, owner: ReviewOwner) {
    if !navigation_allowed(state) {
        return;
    }
    let kind = owner.kind;
    let start = state
        .try_update(|s| {
            let target = Target::capture(s)?;
            let max_entries = s.files.file_history.action_max_entries()?;
            let inventory = inventory(s, kind)?;
            if inventory.instance != owner.instance {
                return None;
            }
            let generation = s.files.history_aux.begin_review(owner);
            match kind {
                review::Kind::Merge => s.files.merge.as_mut()?.discard_review(),
                review::Kind::Tags => s.files.tag_edit.as_mut()?.discard_review(),
                review::Kind::Backup => {
                    unreachable!("invariant: selected history review is merge or tags")
                }
            }
            Some((target, inventory, generation, max_entries))
        })
        .flatten();
    let Some((target, requested, generation, max_entries)) = start else {
        return;
    };
    spawn_local(async move {
        let result =
            collect_review(state, &target, &requested, generation, owner, max_entries).await;
        let completion = state
            .try_update(|s| complete_review(s, &target, generation, owner, &requested, result));
        match completion {
            Some(ReviewCompletion::Prepared(Err(error))) => review::error(state, kind, error),
            Some(ReviewCompletion::Failed(error)) => report_local(state, &target, error),
            Some(ReviewCompletion::Obsolete | ReviewCompletion::Prepared(Ok(()))) | None => {}
        }
    });
}

enum ReviewCompletion {
    Obsolete,
    Prepared(Result<(), String>),
    Failed(api::Error),
}

fn complete_review(
    state: &mut State,
    target: &Target,
    generation: u64,
    owner: ReviewOwner,
    requested: &ReviewInventory,
    result: Result<Option<ReviewFacts>, api::Error>,
) -> ReviewCompletion {
    if !review_current(state, target, generation, owner) {
        return ReviewCompletion::Obsolete;
    }
    state
        .files
        .history_aux
        .complete(Resource::Review, generation);
    if inventory(state, owner.kind).as_ref() != Some(requested) {
        return ReviewCompletion::Obsolete;
    }
    match result {
        Ok(Some(facts)) => {
            let root = target
                .basis
                .as_ref()
                .expect("invariant: review begins on an admitted root")
                .clone();
            let result = match (owner.kind, facts.nodes) {
                (review::Kind::Merge, ReviewNodes::Merge(nodes)) => state
                    .files
                    .merge
                    .as_mut()
                    .expect("invariant: inventory still owns its merge draft")
                    .resolve_review(root, &nodes, &facts.branches, facts.max_entries),
                (review::Kind::Tags, ReviewNodes::Tags(nodes)) => state
                    .files
                    .tag_edit
                    .as_mut()
                    .expect("invariant: inventory still owns its tag draft")
                    .resolve_review(root, &nodes, &facts.branches, facts.max_entries),
                _ => unreachable!("invariant: review facts retain their requested action kind"),
            };
            ReviewCompletion::Prepared(result)
        }
        Ok(None) => ReviewCompletion::Obsolete,
        Err(error) => {
            state.files.file_history.invalidate();
            state.files.history_aux.cancel_all();
            ReviewCompletion::Failed(error)
        }
    }
}

async fn review_page(
    state: RwSignal<State>,
    target: &Target,
    generation: u64,
    owner: ReviewOwner,
    request: &HistoryWindowRequest,
) -> Result<Option<HistoryWindowPage>, api::Error> {
    if !state.with_untracked(|s| review_current(s, target, generation, owner)) {
        return Ok(None);
    }
    let cancellation = api::ReadCancellation::new()?;
    let bound = state
        .try_update(|s| {
            review_current(s, target, generation, owner)
                && s.files.history_aux.replace_owner(
                    Resource::Review,
                    generation,
                    cancellation.clone(),
                )
        })
        .unwrap_or(false);
    if !bound {
        return Ok(None);
    }
    let result = fetch_page(target, request, cancellation).await;
    // Revocation belongs to the session, including an obsolete request's 401.
    if !accept_read_response(state, target.epoch, &result)
        || !state.with_untracked(|s| review_current(s, target, generation, owner))
    {
        return Ok(None);
    }
    result.map(Some)
}

async fn collect_review(
    state: RwSignal<State>,
    target: &Target,
    requested: &ReviewInventory,
    generation: u64,
    owner: ReviewOwner,
    max_entries: usize,
) -> Result<Option<ReviewFacts>, api::Error> {
    let nodes = match owner.kind {
        review::Kind::Merge => ReviewNodes::Merge(Vec::with_capacity(requested.node_ids.len())),
        review::Kind::Tags => ReviewNodes::Tags(Vec::with_capacity(requested.node_ids.len())),
        review::Kind::Backup => unreachable!("invariant: backup selection is not a history review"),
    };
    let mut facts = ReviewFacts {
        nodes,
        branches: Vec::with_capacity(requested.branch_ids.len()),
        max_entries,
    };
    for id in &requested.node_ids {
        let request = make_request(target, HistoryCollection::Nodes, None, Some(id), true)?;
        let Some(page) = review_page(state, target, generation, owner, &request).await? else {
            return Ok(None);
        };
        let HistoryWindowPage::Nodes {
            identity, nodes, ..
        } = page
        else {
            return Err(
                history::DecodeError::Relation("node review received another collection").into(),
            );
        };
        facts.max_entries = identity.action_max_entries.min(usize::MAX as u64) as usize;
        let node = nodes
            .into_iter()
            .find(|node| node.node_id.as_str() == id)
            .expect("invariant: admitted anchored page contains its requested node");
        match &mut facts.nodes {
            ReviewNodes::Merge(nodes) => nodes.push(ChosenNode {
                id: node.node_id.to_string(),
                version: node.version_id.to_string(),
            }),
            ReviewNodes::Tags(nodes) => nodes.push(SelectedNode {
                id: node.node_id.to_string(),
                version: node.version_id.to_string(),
                tags: node.tags.into_iter().collect(),
            }),
        }
        // All unselected node data and parent previews are released here.
    }
    if !collect_review_branches(state, target, requested, generation, owner, &mut facts).await? {
        return Ok(None);
    }
    Ok(Some(facts))
}

async fn collect_review_branches(
    state: RwSignal<State>,
    target: &Target,
    requested: &ReviewInventory,
    generation: u64,
    owner: ReviewOwner,
    facts: &mut ReviewFacts,
) -> Result<bool, api::Error> {
    let mut remaining = requested
        .branch_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut cursor: Option<HistoryCursor> = None;
    let mut last: Option<String> = None;
    while !remaining.is_empty() {
        let request = make_request(
            target,
            HistoryCollection::Branches,
            cursor.as_ref().map(HistoryCursor::as_str),
            None,
            true,
        )?;
        let Some(page) = review_page(state, target, generation, owner, &request).await? else {
            return Ok(false);
        };
        let HistoryWindowPage::Branches {
            identity,
            branches,
            page: cursors,
        } = &page
        else {
            return Err(history::DecodeError::Relation(
                "branch review received another collection",
            )
            .into());
        };
        facts.max_entries = identity.action_max_entries.min(usize::MAX as u64) as usize;
        if branches.first().is_some_and(|branch| {
            last.as_deref()
                .is_some_and(|last| branch.branch_id.as_str() <= last)
        }) {
            return Err(history::DecodeError::Relation(
                "branch review did not advance beyond its previous page",
            )
            .into());
        }
        last = branches.last().map(|branch| branch.branch_id.to_string());
        let next = cursors.next_cursor.clone();
        branch_progress(cursor.as_ref(), next.as_ref())?;
        // Reuse the admitted branch projection; only selected compact facts
        // survive this single local page, never candidate branch catalogues.
        let mut projected = FileHistory::default();
        projected
            .install_auxiliary(page)
            .map_err(api::Error::from)?;
        for branch in projected.branches() {
            if remaining.remove(branch.id())
                && let Some(branch) = SelectedBranch::capture(branch)
            {
                facts.branches.push(branch);
            }
        }
        cursor = next;
        if cursor.is_none() {
            break;
        }
    }
    Ok(true)
}

fn branch_progress(
    previous: Option<&HistoryCursor>,
    next: Option<&HistoryCursor>,
) -> Result<(), api::Error> {
    if let (Some(previous), Some(next)) = (previous, next)
        && !matches!((&previous.payload().position, &next.payload().position),
            (HistoryPosition::Branch { branch_id: previous }, HistoryPosition::Branch { branch_id: next }) if next > previous)
    {
        return Err(history::DecodeError::Relation("branch review cursor does not advance").into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/actions/history_tests.rs"]
mod tests;
