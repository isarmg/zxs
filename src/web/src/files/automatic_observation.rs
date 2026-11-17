//! Private, bounded observations of immutable automatic-publication decisions.
use crate::{
    api,
    ui::{State, accept_read_response},
};
use name_policy::PortablePath;
use storage_contract::{
    FileId, Principal, RepositoryId, Timestamp,
    automatic_observation::{Binding, Position, Reply, Request},
    snapshot_worktree::NamespaceDirectoryId,
};
use web_interface::{i18n::t, prelude::*, tasks::spawn_local};

mod calendar;
mod view;
pub(super) use view::panel;

#[derive(Debug, error_definition::Error)]
pub enum DecodeError {
    #[error("invalid automatic observation JSON: {0}")]
    Wire(#[source] json_codec::Error),
    #[error("invalid automatic observation page: {0}")]
    Page(#[source] storage_contract::automatic_observation::ObservationError),
    #[error("invalid automatic observation relation: {0}")]
    Relation(&'static str),
}
impl From<DecodeError> for api::Error {
    fn from(source: DecodeError) -> Self {
        api::Error::invalid_response(
            "The automatic publication observation could not be verified.",
            api::ErrorSource::AutomaticObservation(source),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Target {
    principal: Principal,
    epoch: u64,
    navigation: u64,
    slot: String,
    repository: RepositoryId,
    path: Option<PortablePath>,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        let files = &state.files;
        if !state.session_active()
            || !files.loaded
            || files.loading
            || files.listing_failed
            || files.directory_preview.is_some()
            || files.file_preview.is_some()
            || !files
                .disks
                .iter()
                .any(|d| d.slot_id == files.slot && d.level.get() == 1 && d.online)
        {
            return None;
        }
        Some(Self {
            principal: state.principal.clone()?,
            epoch: state.session_epoch,
            navigation: files.generation,
            slot: files.slot.clone(),
            repository: files.repository_id.parse().ok()?,
            path: if files.repository_path.is_empty() {
                None
            } else {
                Some(PortablePath::parse(&files.repository_path).ok()?)
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Window {
    start: Timestamp,
    end: Timestamp,
    next: Option<(Request, Binding)>,
}
impl Window {
    fn endpoint(&self, target: &Target) -> String {
        let mut query = format!("start={}&end={}", self.start, self.end);
        if let Some(path) = &target.path {
            query.push_str(&format!("&path={}", api::segment(path.as_str())));
        }
        if let Some((request, binding)) = self.next
            && let Position::After { observed, last } = request.position
        {
            query.push_str(&format!(
                "&observed={observed}&last_file={}&last_node={}&binding={binding}",
                last.file, last.node
            ));
        }
        format!(
            "/api/file-manager/repositories/{}/automatic-nodes?{query}",
            target.repository
        )
    }
    fn admit(&self, target: &Target, reply: &Reply) -> Result<(), DecodeError> {
        let request = reply.page.request;
        if reply.path != target.path
            || request.scope.repository != target.repository
            || request.scope.start != self.start
            || request.scope.end != self.end
            || !matches!((&target.path,request.scope.directory),(None,NamespaceDirectoryId::Repository(id)) if id==target.repository)
                && !matches!(
                    (&target.path, request.scope.directory),
                    (Some(_), NamespaceDirectoryId::Entry(_))
                )
        {
            return Err(DecodeError::Relation("different directory or interval"));
        }
        let expected = match self.next {
            None => Request {
                scope: request.scope,
                position: Position::First,
            },
            Some((next, _)) => next,
        };
        reply.page.validate(&expected).map_err(DecodeError::Page)?;
        if reply.next_binding.is_some() != reply.page.next.is_some() {
            return Err(DecodeError::Relation("incomplete server continuation"));
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
struct Totals {
    files: u64,
    nodes: u64,
    last_file: Option<FileId>,
    last_file_nodes: u64,
}
impl Totals {
    fn advance(&self, reply: &Reply) -> Result<Self, DecodeError> {
        let mut after = self.clone();
        for row in &reply.page.rows {
            after.nodes = after
                .nodes
                .checked_add(1)
                .ok_or(DecodeError::Relation("node count overflow"))?;
            if after.last_file != Some(row.key.file) {
                after.files = after
                    .files
                    .checked_add(1)
                    .ok_or(DecodeError::Relation("file count overflow"))?;
                after.last_file = Some(row.key.file);
                after.last_file_nodes = 0;
            }
            after.last_file_nodes += 1; // bounded by checked total above
        }
        Ok(after)
    }
}
#[derive(Clone, Default)]
struct Model {
    target: Option<Target>,
    generation: u64,
    read: Option<api::ReadCancellation>,
    start: String,
    end: String,
    totals: Totals,
    carried: u64,
    page: Option<Reply>,
    error: Option<api::Error>,
}
impl Model {
    fn clear(&mut self) {
        if let Some(read) = self.read.take() {
            read.abort();
        }
        self.generation += 1;
        self.page = None;
        self.totals = Totals::default();
        self.carried = 0;
        self.error = None;
    }
    fn target(&mut self, target: Option<Target>) {
        if self.target != target {
            self.clear();
            self.target = target;
        }
    }
    fn window(&self, next: bool) -> Result<Option<Window>, api::Error> {
        if self.target.is_none() || self.read.is_some() {
            return Ok(None);
        }
        if next {
            return Ok(self.page.as_ref().and_then(|r| {
                Some(Window {
                    start: r.page.request.scope.start,
                    end: r.page.request.scope.end,
                    next: Some((r.page.continuation()?, r.next_binding?)),
                })
            }));
        }
        let invalid = || {
            api::error(
                storage_contract::ErrorCode::InvalidRequest,
                t(
                    "请输入 UTC 开始和结束时间，结束时间必须较晚。",
                    "Enter a UTC start and a later end time.",
                ),
            )
        };
        let start = calendar::parse(&self.start).ok_or_else(invalid)?;
        let end = calendar::parse(&self.end).ok_or_else(invalid)?;
        if start >= end {
            return Err(invalid());
        }
        Ok(Some(Window {
            start,
            end,
            next: None,
        }))
    }
    fn apply(&mut self, reply: Reply) -> Result<(), api::Error> {
        let advanced = self.totals.advance(&reply)?;
        self.carried = if reply
            .page
            .rows
            .first()
            .is_some_and(|r| Some(r.key.file) == self.totals.last_file)
        {
            self.totals.last_file_nodes
        } else {
            0
        };
        self.totals = advanced;
        self.page = Some(reply);
        Ok(())
    }
}
fn load(state: RwSignal<State>, model: RwSignal<Model>, next: bool) {
    let Some(target) = state.with_untracked(Target::capture) else {
        return;
    };
    let window = model.with_untracked(|m| {
        if m.target.as_ref() == Some(&target) {
            m.window(next)
        } else {
            Ok(None)
        }
    });
    let window = match window {
        Ok(Some(w)) => w,
        Ok(None) => return,
        Err(e) => {
            model.update(|m| m.error = Some(e));
            return;
        }
    };
    let cancel = match api::ReadCancellation::new() {
        Ok(c) => c,
        Err(e) => {
            model.update(|m| m.error = Some(e));
            return;
        }
    };
    let Some(generation) = model.try_update(|m| {
        if !next {
            m.clear();
        } else {
            m.generation += 1;
            m.error = None;
        }
        m.read = Some(cancel.clone());
        m.generation
    }) else {
        return;
    };
    spawn_local(async move {
        let result = api::cancellable_get(&window.endpoint(&target), cancel)
            .await
            .and_then(|value| {
                let reply: Reply = json_codec::from_value(value).map_err(DecodeError::Wire)?;
                window.admit(&target, &reply)?;
                Ok(reply)
            });
        if !accept_read_response(state, target.epoch, &result)
            || state.try_with_untracked(Target::capture).flatten().as_ref() != Some(&target)
        {
            return;
        }
        let _ = model.try_update(|m| {
            if m.generation != generation || m.target.as_ref() != Some(&target) {
                return;
            }
            m.read = None;
            if let Err(error) = result.and_then(|reply| m.apply(reply)) {
                // Denied, stale or malformed observations are not a completed count.
                // Drop previous private IDs as well, rather than displaying data after revocation.
                m.clear();
                m.error = Some(error);
            }
        });
    });
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/automatic_observation.rs"]
mod tests;
