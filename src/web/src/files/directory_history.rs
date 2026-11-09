//! Bounded directory composition history. Reads confer no write authority;
//! separate branch commands require admitted, writable current-folder status.
//! Displaying a graph never initializes a directory or creates a file version.
use crate::{
    api,
    ui::{State, accept_read_response},
};
use data_serialization::Deserialize;
use std::{collections::BTreeMap, sync::Arc};
use storage_contract::directory_history::{
    DirectoryHistoryCollection as Collection, DirectoryHistoryPage, DirectoryHistoryRequest,
};
use web_interface::{prelude::*, tasks::spawn_local};

mod view;
pub(super) use view::{current_directory_view, view};
const PAGE_SIZE: usize = 64;

#[derive(Debug, error_definition::Error)]
pub enum DecodeError {
    #[error("invalid directory history response: {0}")]
    Wire(#[source] json_codec::Error),
    #[error("inconsistent directory history response: {0}")]
    Contract(#[source] storage_contract::Error),
}
impl From<DecodeError> for api::Error {
    fn from(source: DecodeError) -> Self {
        Self {
            status: 200,
            code: storage_contract::ErrorCode::InvalidResponse,
            message: "The directory history did not match the requested folder and history root."
                .into(),
            retryable: false,
            request_id: None,
            source: Some(Arc::new(api::ErrorSource::DirectoryHistory(source))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Scope {
    Selected,
    CurrentDirectory {
        initialized: bool,
        generation: u64,
        repository_path: String,
        namespace_head: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Target {
    epoch: u64,
    repository: String,
    directory: String,
    slot: String,
    path: String,
    preview: bool,
    scope: Scope,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        let entry = state.files.selected.as_ref()?;
        if !state.session_active()
            || entry.kind() != storage_contract::NamespaceEntryKind::Directory
        {
            return None;
        }
        Some(Self {
            epoch: state.session_epoch,
            repository: entry
                .repository_id()
                .or_else(|| {
                    // The disk-root listing emits repository folders with the
                    // repository identity itself, and no namespace location.
                    (!entry.backup() && state.files.directory.is_empty())
                        .then_some(entry.entry_id())
                })?
                .into(),
            directory: entry.entry_id().into(),
            slot: state.files.slot.clone(),
            path: entry.path().into(),
            preview: entry.preview(),
            scope: Scope::Selected,
        })
    }
    /// The caller has matched the decoded Git status to this exact listing.
    /// Paths select the view; only that response supplies the stable directory ID.
    pub(super) fn current_directory(
        state: &State,
        status: &storage_contract::directory_git::DirectoryGitStatus,
    ) -> Self {
        Self {
            epoch: state.session_epoch,
            repository: status.repository_id.clone(),
            directory: status.directory_id.clone(),
            slot: state.files.slot.clone(),
            path: state.files.directory.clone(),
            preview: false,
            scope: Scope::CurrentDirectory {
                initialized: status.initialized,
                generation: state.files.generation,
                repository_path: status.path.clone(),
                namespace_head: state.files.namespace_head.clone(),
            },
        }
    }

    fn can_manage_branches(&self, state: &State) -> bool {
        matches!(
            &self.scope,
            Scope::CurrentDirectory {
                initialized: true,
                ..
            }
        ) && self.current(state)
            && state.files.directory_branches_supported
            && super::actions::write_allowed(state)
    }

    fn current(&self, state: &State) -> bool {
        match &self.scope {
            Scope::Selected => Self::capture(state).as_ref() == Some(self),
            Scope::CurrentDirectory {
                generation,
                repository_path,
                namespace_head,
                ..
            } => {
                state.session_active()
                    && state.session_epoch == self.epoch
                    && state.files.loaded
                    && !state.files.loading
                    && state.files.selected.is_none()
                    && state.files.directory_preview.is_none()
                    && state.files.file_preview.is_none()
                    && state.files.generation == *generation
                    && state.files.slot == self.slot
                    && state.files.directory == self.path
                    && state.files.repository_id == self.repository
                    && state.files.repository_path == *repository_path
                    && state.files.namespace_head == *namespace_head
            }
        }
    }
}

#[derive(Clone, Default)]
struct Model {
    nodes: Option<DirectoryHistoryPage>,
    branches: Option<DirectoryHistoryPage>,
    generation: u64,
    loading: bool,
    stale: bool,
    error: Option<api::Error>,
    cancellation: Option<api::ReadCancellation>,
}
impl Model {
    fn cancel(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.abort();
        }
        self.generation += 1;
        self.loading = false;
    }
    fn install(&mut self, page: DirectoryHistoryPage, collection: Collection) {
        match collection {
            Collection::Nodes => {
                if self
                    .branches
                    .as_ref()
                    .is_some_and(|old| old.identity.history_root != page.identity.history_root)
                {
                    self.branches = None;
                }
                self.nodes = Some(page);
            }
            Collection::Branches => self.branches = Some(page),
        }
        self.stale = false;
        self.error = None;
    }
    fn failed(&mut self, error: api::Error) {
        self.stale |= error.code == storage_contract::ErrorCode::HistoryChanged;
        self.error = Some(error);
    }
}

#[derive(Clone)]
enum Read {
    Refresh,
    Nodes {
        cursor: Option<String>,
        anchor: Option<String>,
    },
    Branches(String),
}
impl Read {
    fn collection(&self) -> Collection {
        match self {
            Self::Refresh | Self::Nodes { .. } => Collection::Nodes,
            Self::Branches(_) => Collection::Branches,
        }
    }
}
fn request(
    target: &Target,
    model: &Model,
    read: &Read,
) -> Result<DirectoryHistoryRequest, api::Error> {
    let mut query = BTreeMap::from([("limit".into(), PAGE_SIZE.to_string())]);
    if !matches!(read, Read::Refresh) {
        let page = model.nodes.as_ref().ok_or_else(|| {
            api::error(
                storage_contract::ErrorCode::InvalidRequest,
                "Read this folder's history before following a history link.",
            )
        })?;
        if model.stale {
            return Err(api::error(
                storage_contract::ErrorCode::HistoryChanged,
                "Refresh the folder history before following a history link.",
            ));
        }
        query.insert(
            "history_root".into(),
            page.identity.history_root.to_string(),
        );
    }
    match read {
        Read::Nodes { cursor, anchor } => {
            if let Some(cursor) = cursor {
                query.insert("cursor".into(), cursor.clone());
            }
            if let Some(anchor) = anchor {
                query.insert("anchor_tree_id".into(), anchor.clone());
            }
        }
        Read::Branches(cursor) => {
            query.insert("cursor".into(), cursor.clone());
        }
        Read::Refresh => {}
    }
    DirectoryHistoryRequest::parse(
        &target.repository,
        &target.directory,
        read.collection(),
        query,
    )
    .map_err(|source| DecodeError::Contract(source).into())
}
fn endpoint(request: &DirectoryHistoryRequest) -> String {
    let query = request
        .query_fields()
        .into_iter()
        .map(|(key, value)| format!("{}={}", api::segment(&key), api::segment(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{query}", request.route())
}
async fn fetch(
    request: &DirectoryHistoryRequest,
    cancellation: api::ReadCancellation,
) -> Result<DirectoryHistoryPage, api::Error> {
    let raw = api::cancellable_get(&endpoint(request), cancellation).await?;
    let page = DirectoryHistoryPage::deserialize(raw).map_err(DecodeError::Wire)?;
    page.validate(request).map_err(DecodeError::Contract)?;
    Ok(page)
}
fn owned(state: RwSignal<State>, model: RwSignal<Model>, target: &Target, generation: u64) -> bool {
    state
        .try_with_untracked(|state| target.current(state))
        .unwrap_or(false)
        && model
            .try_with_untracked(|model| model.generation == generation)
            .unwrap_or(false)
}
fn load(state: RwSignal<State>, model: RwSignal<Model>, target: StoredValue<Target>, read: Read) {
    let target = target.get_value();
    if !state.with_untracked(|state| target.current(state)) {
        return;
    }
    let request = model.with_untracked(|model| request(&target, model, &read));
    let (request, cancellation) = match request.and_then(|request| {
        api::ReadCancellation::new().map(|cancellation| (request, cancellation))
    }) {
        Ok(values) => values,
        Err(error) => {
            model.update(|model| model.failed(error));
            return;
        }
    };
    let generation = model
        .try_update(|model| {
            model.cancel();
            model.loading = true;
            model.error = None;
            model.cancellation = Some(cancellation.clone());
            model.generation
        })
        .expect("invariant: mounted directory history owns its read state");
    spawn_local(async move {
        let result = fetch(&request, cancellation).await;
        if !accept_read_response(state, target.epoch, &result)
            || !owned(state, model, &target, generation)
        {
            return;
        }
        let mut branch_request = None;
        match result {
            Ok(page) => {
                if matches!(read, Read::Refresh) {
                    branch_request = Some(DirectoryHistoryRequest::parse(
                        &target.repository,
                        &target.directory,
                        Collection::Branches,
                        BTreeMap::from([
                            ("limit".into(), PAGE_SIZE.to_string()),
                            (
                                "history_root".into(),
                                page.identity.history_root.to_string(),
                            ),
                        ]),
                    ));
                }
                model.update(|model| model.install(page, read.collection()));
            }
            Err(error) => model.update(|model| model.failed(error)),
        }
        if let Some(request) = branch_request {
            // A completed fetch aborts its controller when its body guard is
            // dropped. The next page owns a fresh controller, while retaining
            // the same logical refresh generation.
            let next = request
                .map_err(|source| api::Error::from(DecodeError::Contract(source)))
                .and_then(|request| {
                    api::ReadCancellation::new().map(|cancellation| (request, cancellation))
                });
            let result = match next {
                Ok((request, cancellation)) => {
                    model.update(|model| {
                        if let Some(previous) = model.cancellation.replace(cancellation.clone()) {
                            previous.abort();
                        }
                    });
                    fetch(&request, cancellation).await
                }
                Err(error) => Err(error),
            };
            if !accept_read_response(state, target.epoch, &result)
                || !owned(state, model, &target, generation)
            {
                return;
            }
            model.update(|model| match result {
                Ok(page) => model.install(page, Collection::Branches),
                Err(error) => model.failed(error),
            });
        }
        model.update(|model| {
            model.loading = false;
            model.cancellation = None;
        });
    });
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/directory_history/tests.rs"]
mod tests;
