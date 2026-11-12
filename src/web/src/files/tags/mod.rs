//! Current file-container tags. Neither history nodes nor branch heads own them.
use crate::{
    api,
    files::{
        actions,
        model::{self, Model},
        review,
    },
    ui::{State, accept_read_response},
};
use json_codec::{Value, json};
use storage_contract::file_container_tags::{
    Context, ReadRequest, ReplaceCommand, Snapshot, Tag, TagSet,
};
use web_interface::{i18n::t, prelude::*, tasks::spawn_local};
mod view;
pub(crate) use crate::files::tags::view::view;

#[derive(Debug, error_definition::Error)]
pub enum DecodeError {
    #[error("invalid container tag response: {0}")]
    Wire(#[source] json_codec::Error),
    #[error("container tag authority mismatch: {0}")]
    Authority(#[source] storage_contract::file_container_tags::TagsError),
}
impl From<DecodeError> for api::Error {
    fn from(source: DecodeError) -> Self {
        Self::invalid_response(
            "The container tag response did not match the requested file context.",
            api::ErrorSource::ContainerTags(source),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    epoch: u64,
    navigation: u64,
    request: ReadRequest,
    entry: String,
    read_only: bool,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active() {
            return None;
        }
        let entry = state.files.selected.as_ref()?;
        if entry.kind() != storage_contract::NamespaceEntryKind::File {
            return None;
        }
        let (repository, path) = entry.context()?;
        // Listing admission has already validated these exact typed identities.
        Some(Self {
            epoch: state.session_epoch,
            navigation: state.files.generation,
            entry: entry.entry_id().to_owned(),
            read_only: entry.read_only(),
            request: ReadRequest {
                file_id: entry.file_id().parse().ok()?,
                context: Context {
                    repository_id: repository.parse().ok()?,
                    path: name_policy::PortablePath::parse(path).ok()?,
                },
            },
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct ReadModel {
    mount: u64,
    generation: u64,
    target: Option<Target>,
    snapshot: Option<Snapshot>,
    cancellation: Option<api::ReadCancellation>,
    loading: bool,
    error: Option<api::Error>,
}
impl ReadModel {
    pub(crate) fn cancel(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.abort();
        }
        self.generation += 1;
        self.loading = false;
    }
    pub(crate) fn clear(&mut self) {
        self.cancel();
        self.target = None;
        self.snapshot = None;
        self.error = None;
    }
    fn set_target(&mut self, target: Option<Target>) {
        if self.target != target {
            self.clear();
            self.target = target;
        }
    }
    fn begin(&mut self) -> u64 {
        self.cancel();
        self.loading = true;
        self.snapshot = None;
        self.error = None;
        self.generation
    }
    fn complete(
        &mut self,
        target: &Target,
        generation: u64,
        result: Result<Snapshot, api::Error>,
    ) {
        if self.target.as_ref() != Some(target) || self.generation != generation {
            return;
        }
        self.cancellation = None;
        self.loading = false;
        match result {
            Ok(snapshot) => self.snapshot = Some(snapshot),
            Err(error) => self.error = Some(error),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TagOwner {
    instance: u64,
    target: Target,
}
impl TagOwner {
    pub(crate) fn current(&self, files: &Model) -> bool {
        files.tag_edit.as_ref().is_some_and(|draft| draft.owned_by(self))
            && files.generation == self.target.navigation
            && files.selected.as_ref().is_some_and(|entry| {
                entry.entry_id() == self.target.entry
                    && entry.read_only() == self.target.read_only
                    && entry.file_id() == self.target.request.file_id.to_string()
                    && entry.context().is_some_and(|(repository, path)| {
                        repository == self.target.request.context.repository_id.to_string()
                            && path == self.target.request.context.path.as_str()
                    })
            })
    }
}

#[derive(Clone)]
pub(crate) struct Draft {
    owner: TagOwner,
    command: ReplaceCommand,
    input: String,
    needs_review: bool,
}
impl Draft {
    pub(crate) fn owner(&self) -> TagOwner {
        self.owner.clone()
    }
    pub(crate) fn owned_by(&self, owner: &TagOwner) -> bool {
        &self.owner == owner
    }
    pub(crate) fn instance(&self) -> u64 {
        self.owner.instance
    }
    pub(crate) fn requires_review(&self) -> bool {
        self.needs_review
    }
    pub(crate) fn require_review(&mut self) {
        self.needs_review = true;
    }
    pub(crate) fn command(&self) -> &ReplaceCommand {
        &self.command
    }
}

fn endpoint(request: &ReadRequest) -> String {
    format!(
        "/api/file-manager/files/{}/tags",
        api::segment(&request.file_id.to_string()),
    )
}
fn read_endpoint(request: &ReadRequest) -> String {
    format!(
        "{}?repository_id={}&path={}",
        endpoint(request),
        api::segment(&request.context.repository_id.to_string()),
        api::segment(request.context.path.as_str()),
    )
}
fn decode(value: Value, request: &ReadRequest) -> Result<Snapshot, api::Error> {
    let snapshot: Snapshot = json_codec::from_value(value).map_err(DecodeError::Wire)?;
    snapshot.validate(request).map_err(DecodeError::Authority)?;
    Ok(snapshot)
}
pub(crate) fn reload(state: RwSignal<State>) {
    let Some(target) = state.with_untracked(Target::capture) else {
        return;
    };
    if state.with_untracked(actions::pending) {
        return;
    }
    let cancellation = match api::ReadCancellation::new() {
        Ok(value) => value,
        Err(error) => {
            state.update(|s| s.files.container_tags.error = Some(error));
            return;
        }
    };
    // The live root signal was read above; no await or owner transition occurs
    // before this synchronous update. Async completion uses try_update instead.
    let generation = state
        .try_update(|state| {
            let model = &mut state.files.container_tags;
            model.set_target(Some(target.clone()));
            let generation = model.begin();
            model.cancellation = Some(cancellation.clone());
            generation
        })
        .expect("invariant: mounted tag panel owns its read");
    spawn_local(async move {
        let result = api::cancellable_get(&read_endpoint(&target.request), cancellation)
            .await
            .and_then(|value| decode(value, &target.request));
        if !accept_read_response(state, target.epoch, &result)
            || state.try_with_untracked(Target::capture).flatten().as_ref() != Some(&target)
        {
            return;
        }
        let _ = state.try_update(|s| s.files.container_tags.complete(&target, generation, result));
    });
}

pub(crate) fn start(state: RwSignal<State>) {
    state.update(|s| {
        if !actions::write_allowed(s)
            || model::selected_entry_read_only(&s.files)
            || s.files.tag_edit.is_some()
        {
            return;
        }
        let Some(target) = Target::capture(s) else {
            return;
        };
        let read = &s.files.container_tags;
        if read.loading || read.target.as_ref() != Some(&target) {
            return;
        }
        let Some(snapshot) = read.snapshot.as_ref() else {
            return;
        };
        if s.files.publications
            .get(&review::Kind::Tags)
            .is_some_and(|publication| publication.operation_id.is_some())
        {
            return;
        }
        s.files.history_next_draft += 1;
        s.files.tag_edit = Some(Draft {
            owner: TagOwner {
                instance: s.files.history_next_draft,
                target,
            },
            command: ReplaceCommand {
                file_id: snapshot.file_id.clone(),
                context: snapshot.context.clone(),
                expected_generation: snapshot.generation,
                tags: snapshot.tags.clone(),
            },
            input: String::new(),
            needs_review: false,
        });
        s.files.publications.remove(&review::Kind::Tags);
    });
}
fn edit(state: RwSignal<State>, instance: u64, update: impl FnOnce(&mut Draft)) {
    state.update(|s| {
        if !actions::write_allowed(s) || model::selected_entry_read_only(&s.files) {
            return;
        }
        let target = Target::capture(s);
        let Some(draft) = s.files.tag_edit.as_mut().filter(|draft| {
            draft.instance() == instance && target.as_ref() == Some(&draft.owner.target)
        }) else {
            return;
        };
        update(draft);
    });
}
fn add_input(state: RwSignal<State>, instance: u64) {
    let result = state.try_update(|s| {
        if !actions::write_allowed(s) || model::selected_entry_read_only(&s.files) {
            return Ok(());
        }
        let target = Target::capture(s);
        let Some(draft) = s.files.tag_edit.as_mut().filter(|draft| {
            draft.instance() == instance && target.as_ref() == Some(&draft.owner.target)
        }) else {
            return Ok(());
        };
        let tag = Tag::parse(draft.input.clone())?;
        let tags = TagSet::new(draft.command.tags.iter().cloned().chain([tag]))?;
        draft.command.tags = tags;
        draft.input.clear();
        Ok::<(), storage_contract::file_container_tags::TagsError>(())
    });
    if let Some(Err(error)) = result {
        review::error(state, review::Kind::Tags, error.to_string());
    }
}
fn remove(state: RwSignal<State>, instance: u64, tag: &str) {
    edit(state, instance, |draft| {
        draft.command.tags = TagSet::new(
            draft.command.tags.iter().filter(|value| value.as_str() != tag).cloned(),
        )
        .expect("invariant: removing a tag preserves its admitted set");
    });
}
fn accept_current(state: RwSignal<State>, instance: u64) {
    state.update(|s| {
        if !actions::write_allowed(s) || model::selected_entry_read_only(&s.files) {
            return;
        }
        let Some(target) = Target::capture(s) else {
            return;
        };
        let read = &s.files.container_tags;
        if read.loading || read.target.as_ref() != Some(&target) {
            return;
        }
        let Some(snapshot) = read.snapshot.as_ref() else {
            return;
        };
        let Some(draft) = s.files.tag_edit.as_mut().filter(|draft| {
            draft.instance() == instance && draft.owner.target == target && draft.needs_review
        }) else {
            return;
        };
        // Explicit confirmation changes only the CAS base. Keep the user's exact
        // replacement set; never silently merge it with concurrent changes.
        draft.command.expected_generation = snapshot.generation;
        draft.needs_review = false;
        s.files.publications.remove(&review::Kind::Tags);
    });
}
fn cancel(state: RwSignal<State>, instance: u64) {
    state.update(|s| {
        if actions::pending(s)
            || s.files.publications
                .get(&review::Kind::Tags)
                .is_some_and(|publication| publication.operation_id.is_some())
        {
            return;
        }
        if s.files.tag_edit.as_ref().is_some_and(|d| d.instance() == instance) {
            s.files.tag_edit = None;
            s.files.publications.remove(&review::Kind::Tags);
        }
    });
}
fn submit(state: RwSignal<State>, instance: u64) {
    let command = state.with_untracked(|s| {
        let draft = s.files.tag_edit.as_ref().filter(|d| d.instance() == instance)?;
        if draft.requires_review() || Target::capture(s).as_ref() != Some(&draft.owner.target) {
            return None;
        }
        if !draft.input.is_empty() {
            return Some(Err(t(
                "请先添加或清除未提交的标签输入。",
                "Add or clear the pending tag input before saving.",
            )));
        }
        Some(Ok(draft.command.clone()))
    });
    match command {
        Some(Ok(command)) => {
            let request = ReadRequest {
                file_id: command.file_id.clone(),
                context: command.context.clone(),
            };
            let body = json!({
                "repository_id": command.context.repository_id,
                "path": command.context.path,
                "expected_generation": command.expected_generation,
                "tags": command.tags,
            });
            review::submit(state, review::Kind::Tags, "POST", endpoint(&request), body);
        }
        Some(Err(error)) => review::error(state, review::Kind::Tags, error),
        None => {}
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/tags/tests.rs"]
pub(crate) mod tests;
