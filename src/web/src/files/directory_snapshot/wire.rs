//! Browser-boundary DTOs and immutable requests for the formal `.zxs` path.
mod worktree;
use crate::api;
use data_serialization::{Deserialize, Serialize};
use json_codec::Value;
use name_policy::SlotId;
use storage_contract::file_draft::{DraftBase, DraftLocation, FileWorkingState};
use storage_contract::snapshot_worktree::creation::AllocateWorktree;
use storage_contract::snapshot_worktree::discovery::{
    DirectorySnapshotHead, SnapshotHeadDiscovery,
};
use storage_contract::snapshot_worktree::draft_commit::{
    CommitDirectoryDrafts, DirectoryDraftExpectation, DirectoryDraftOutcome, DirectoryDraftReceipt,
    SelectedDirectoryDraft,
};
use storage_contract::snapshot_worktree::root::{RegisterSnapshotRoot, RegisteredSnapshotRoot};
use storage_contract::snapshot_worktree::{
    CommitWorktree, CreateWorktree, InitializeSnapshot, SnapshotHandle, WorktreeHandle,
};
use storage_contract::{CommitState, OperationId, Principal, RepositoryId};

pub(super) const MAX_SELECTIONS: usize = crate::files::page::PAGE_SIZE;
const MAX_RECORD_BYTES: usize = 256 * 1024;

#[derive(Debug, error_definition::Error)]
pub enum DecodeError {
    #[error("invalid directory snapshot JSON: {0}")]
    Wire(#[source] json_codec::Error),
    #[error("invalid directory snapshot identity or record: {0}")]
    Contract(#[source] storage_contract::Error),
    #[error("invalid directory snapshot path: {0}")]
    Path(#[source] name_policy::Error),
    #[error("invalid directory snapshot response: {0}")]
    Relation(&'static str),
}

pub(super) fn failure(source: DecodeError) -> api::Error {
    api::Error::invalid_response(
        "The directory snapshot response or original request could not be verified.",
        api::ErrorSource::DirectorySnapshot(source),
    )
}

pub(super) fn invalid(message: &'static str) -> api::Error {
    failure(DecodeError::Relation(message))
}

pub(super) fn decode<T: data_serialization::de::DeserializeOwned>(
    value: Value,
) -> Result<T, api::Error> {
    if !value.is_object() {
        return Err(invalid("a named object is required"));
    }
    json_codec::from_value(value).map_err(|source| failure(DecodeError::Wire(source)))
}

pub(super) fn head(
    value: Value,
    repository: RepositoryId,
) -> Result<DirectorySnapshotHead, api::Error> {
    let context = object_field(&value, "context")?;
    object_field(context, "host")?;
    object_field(context, "source")?;
    let head = object_field(&value, "head")?;
    if head.get("state").and_then(Value::as_str) == Some("head") {
        object_field(head, "snapshot")?;
    }
    let result: DirectorySnapshotHead = decode(value)?;
    if result.repository_id != repository || result.context.source.root.is_some() {
        return Err(invalid("the head belongs to another namespace scope"));
    }
    Ok(result)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Candidate {
    pub(super) selected: SelectedDirectoryDraft,
    pub(super) size: u64,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct BornSnapshot {
    base: DraftBase,
    working: FileWorkingState,
}

pub(super) fn born(value: Value, target: DraftLocation) -> Result<Option<Candidate>, api::Error> {
    object_field(&value, "base")?;
    let working = object_field(&value, "working")?;
    if matches!(working.get("state").and_then(Value::as_str),
        Some("manual_dirty" | "manual_incremental")) {
        object_field(working, "base")?;
        object_field(working, "payload")?;
    }
    let snapshot: BornSnapshot = decode(value)?;
    match snapshot.working {
        FileWorkingState::ManualDirty {
            revision,
            base,
            payload,
        } | FileWorkingState::ManualIncremental { revision, base, payload, .. } => {
            if snapshot.base != base {
                return Err(invalid(
                    "the working base differs from the selected occurrence",
                ));
            }
            Ok(Some(Candidate {
                size: payload.logical_size(),
                selected: SelectedDirectoryDraft {
                    target,
                    expected: DirectoryDraftExpectation::Born {
                        revision,
                        base,
                        payload,
                    },
                },
            }))
        }
        FileWorkingState::Automatic { .. } | FileWorkingState::ManualClean { .. } => Ok(None),
        FileWorkingState::Unborn { .. } => Err(invalid(
            "the file changed to an unborn occurrence; refresh the listing",
        )),
    }
}

pub(super) fn unborn(value: Value, target: DraftLocation) -> Result<Candidate, api::Error> {
    let snapshot = crate::files::unborn::Snapshot::parse(value, &target)?;
    let (expected, size) = snapshot.directory_expectation();
    Ok(Candidate {
        selected: SelectedDirectoryDraft { target, expected },
        size,
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Command {
    RegisterRoot {
        slot: SlotId,
        operation_id: OperationId,
    },
    Initialize {
        repository: RepositoryId,
        request: InitializeSnapshot,
    },
    Commit {
        repository: RepositoryId,
        request: CommitDirectoryDrafts,
    },
    CreateWorktree {
        repository: RepositoryId,
        request: CreateWorktree,
    },
    AllocateWorktree {
        repository: RepositoryId,
        request: AllocateWorktree,
    },
    CommitWorktree {
        repository: RepositoryId,
        source: SnapshotHandle,
        request: CommitWorktree,
    },
}

impl Command {
    pub(super) fn operation(&self) -> OperationId {
        match self {
            Self::RegisterRoot { operation_id, .. } => *operation_id,
            Self::Initialize { request, .. } => request.operation_id,
            Self::Commit { request, .. } => request.operation_id,
            Self::CreateWorktree { request, .. } => request.operation_id,
            Self::AllocateWorktree { request, .. } => request.operation_id,
            Self::CommitWorktree { request, .. } => request.operation_id,
        }
    }

    pub(super) fn endpoint(&self) -> String {
        match self {
            Self::RegisterRoot { slot, .. } => format!(
                "/api/file-manager/disks/{}/snapshot-root",
                api::segment(slot.as_str()),
            ),
            Self::Initialize { .. } => "/api/directory-snapshots".into(),
            Self::Commit { .. } => "/api/directory-snapshots/commits".into(),
            Self::CreateWorktree { .. } | Self::AllocateWorktree { .. } => {
                "/api/directory-worktrees".into()
            }
            Self::CommitWorktree { request, .. } => format!(
                "/api/directory-worktrees/{}/commits",
                request.target.worktree_id,
            ),
        }
    }

    pub(super) fn can_post(&self) -> bool {
        match self {
            Self::CreateWorktree { .. } => false,
            Self::RegisterRoot { .. }
            | Self::Initialize { .. }
            | Self::Commit { .. }
            | Self::AllocateWorktree { .. }
            | Self::CommitWorktree { .. } => true,
        }
    }

    pub(super) fn body(&self) -> Result<Value, api::Error> {
        let result = match self {
            Self::RegisterRoot { operation_id, .. } => json_codec::to_value(RegisterSnapshotRoot {
                operation_id: *operation_id,
            }),
            Self::Initialize { request, .. } => json_codec::to_value(request),
            Self::Commit { request, .. } => json_codec::to_value(request),
            Self::CreateWorktree { .. } => {
                return Err(invalid(
                    "named worktree requests are read-only; query the original result",
                ));
            }
            Self::AllocateWorktree { request, .. } => json_codec::to_value(request),
            Self::CommitWorktree { request, .. } => json_codec::to_value(request),
        };
        result.map_err(|source| failure(DecodeError::Wire(source)))
    }

    pub(super) fn outcome_endpoint(&self) -> Option<String> {
        match self {
            Self::CreateWorktree {
                repository,
                request,
            } => Some(format!(
                "/api/repositories/{repository}/directory-worktrees/creations/{}",
                request.operation_id,
            )),
            Self::AllocateWorktree {
                repository,
                request,
            } => Some(format!(
                "/api/repositories/{repository}/directory-worktrees/creations/{}",
                request.operation_id,
            )),
            Self::CommitWorktree {
                repository,
                request,
                ..
            } => Some(format!(
                "/api/repositories/{repository}/directory-worktrees/commits/{}",
                request.operation_id,
            )),
            Self::RegisterRoot { .. } | Self::Initialize { .. } => None,
            Self::Commit { request, .. } => Some(format!(
                concat!(
                    "/api/directory-snapshots/commits/{}?source_publication={}",
                    "&repository_id={}&branch_id={}&node_id={}",
                ),
                request.operation_id,
                request.source.publication,
                request.source.repository_id,
                request.source.branch_id,
                request.source.node_id,
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "directory-snapshot-original-request")]
    Original,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Record {
    format: Format,
    pub(super) principal: Principal,
    pub(super) command: Command,
}

impl Record {
    pub(super) fn new(principal: Principal, command: Command) -> Result<Self, api::Error> {
        let record = Self {
            format: Format::Original,
            principal,
            command,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), api::Error> {
        match &self.command {
            Command::RegisterRoot { .. }
            | Command::CreateWorktree { .. }
            | Command::AllocateWorktree { .. } => {}
            Command::CommitWorktree { request, .. } => {
                storage_contract::validate_note(&request.message)
                    .map_err(|source| failure(DecodeError::Contract(source)))?;
            }
            Command::Initialize { request, .. } => {
                if request.source.root.is_some() {
                    return Err(invalid("only the current registered root is supported"));
                }
            }
            Command::Commit {
                repository,
                request,
            } => {
                storage_contract::validate_note(&request.message)
                    .map_err(|source| failure(DecodeError::Contract(source)))?;
                if request.selected.is_empty() || request.selected.len() > MAX_SELECTIONS {
                    return Err(invalid(
                        "the explicit selection is empty or exceeds this page",
                    ));
                }
                let mut occurrences = Vec::new();
                for selected in &request.selected {
                    if selected.target.repository != *repository
                        || occurrences.contains(&selected.target)
                    {
                        return Err(invalid(
                            "the selection contains a foreign or duplicate occurrence",
                        ));
                    }
                    occurrences.push(selected.target.clone());
                }
            }
        }
        Ok(())
    }

    pub(super) fn encode(&self) -> Result<String, api::Error> {
        let bytes =
            json_codec::to_string(self).map_err(|source| failure(DecodeError::Wire(source)))?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(invalid(
                "the original request exceeds the browser recovery budget",
            ));
        }
        Ok(bytes)
    }

    pub(super) fn parse(bytes: &str, principal: &Principal) -> Result<Self, api::Error> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(invalid(
                "the stored original request exceeds its byte budget",
            ));
        }
        let value = storage_contract::strict_json(bytes.as_bytes())
            .map_err(|source| failure(DecodeError::Contract(source)))?;
        let command = object_field(&value, "command")?;
        match command.get("kind").and_then(Value::as_str) {
            Some("register_root") => {}
            Some("initialize") => {
                let request = object_field(command, "request")?;
                object_field(request, "host")?;
                object_field(request, "source")?;
            }
            Some("create_worktree" | "allocate_worktree") => {
                object_field(object_field(command, "request")?, "source")?;
            }
            Some("commit_worktree") => {
                object_field(command, "source")?;
                let request = object_field(command, "request")?;
                object_field(request, "target")?;
                object_field(request, "expected")?;
            }
            Some("commit") => {
                let request = object_field(command, "request")?;
                object_field(request, "source")?;
                let selected = request
                    .get("selected")
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid("the original selected set must be an array"))?;
                for item in selected {
                    object_field(item, "target")?;
                    let expected = object_field(item, "expected")?;
                    if expected.get("base").is_some() {
                        object_field(expected, "base")?;
                        object_field(expected, "payload")?;
                    } else {
                        object_field(expected, "content")?;
                    }
                }
            }
            _ => return Err(invalid("the stored request kind is not supported")),
        }
        let record: Self = decode(value)?;
        if &record.principal != principal {
            return Err(invalid("the original request belongs to another account"));
        }
        record.validate()?;
        Ok(record)
    }

    pub(super) fn lookup_result(&self, value: Value) -> Result<LookupResult, api::Error> {
        worktree::lookup(self, value)
    }

    pub(super) fn result(&self, value: Value) -> Result<Publication, api::Error> {
        match &self.command {
            Command::CreateWorktree { repository, .. }
            | Command::AllocateWorktree { repository, .. } => worktree::created(*repository, value),
            Command::CommitWorktree {
                repository,
                source,
                request,
            } => worktree::committed(*repository, *source, request.target, value),
            Command::RegisterRoot { slot, .. } => {
                let root: RegisteredSnapshotRoot = decode(value)?;
                if &root.slot_id != slot {
                    return Err(invalid("root registration belongs to another disk"));
                }
                Ok(Publication::Registered(root))
            }
            Command::Initialize { .. } => {
                // Initialization's completion identity is distinct from its
                // reserve operation. The caller binds this result to real HEAD.
                Ok(Publication::Initialized(decode(value)?))
            }
            Command::Commit { request, .. } => {
                object_field(object_field(&value, "result")?, "snapshot")?;
                let receipt: DirectoryDraftReceipt = decode(value)?;
                if receipt.operation_id != request.operation_id {
                    return Err(invalid("the receipt belongs to another directory commit"));
                }
                let snapshot = match receipt.result {
                    DirectoryDraftOutcome::Changed { snapshot } => {
                        if snapshot.publication != request.operation_id
                            || snapshot.repository_id != request.source.repository_id
                            || snapshot.branch_id != request.source.branch_id
                            || snapshot.node_id == request.source.node_id
                        {
                            return Err(invalid("the changed receipt differs from this branch"));
                        }
                        snapshot
                    }
                    DirectoryDraftOutcome::Unchanged { snapshot } => {
                        if snapshot != request.source {
                            return Err(invalid("the unchanged receipt differs from the source"));
                        }
                        snapshot
                    }
                };
                match receipt.commit_state {
                    CommitState::Published | CommitState::Acked => Ok(Publication::Committed {
                        snapshot,
                        degraded: false,
                    }),
                    CommitState::PublishedDegraded => Ok(Publication::Committed {
                        snapshot,
                        degraded: true,
                    }),
                    CommitState::Staging
                    | CommitState::Sealed
                    | CommitState::Prepared
                    | CommitState::Deciding
                    | CommitState::Decided
                    | CommitState::Protected
                    | CommitState::Recovering
                    | CommitState::RecoveryRequired
                    | CommitState::InDoubt
                    | CommitState::Aborted => Err(invalid(
                        "publication is not confirmed; retain the original request",
                    )),
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum LookupResult {
    Pending,
    Complete(Publication),
}

#[derive(Clone, Debug)]
pub(super) enum Publication {
    WorktreeCreated {
        repository: RepositoryId,
        target: WorktreeHandle,
    },
    WorktreeCommitted {
        repository: RepositoryId,
        target: WorktreeHandle,
        snapshot: SnapshotHandle,
    },
    Registered(RegisteredSnapshotRoot),
    Initialized(SnapshotHandle),
    Committed {
        snapshot: SnapshotHandle,
        degraded: bool,
    },
}

pub(super) fn object_field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, api::Error> {
    value
        .get(key)
        .filter(|field| field.is_object())
        .ok_or_else(|| invalid("the response must retain its named object fields"))
}

pub(super) fn ready_source(head: &DirectorySnapshotHead) -> Option<SnapshotHandle> {
    match head.head {
        SnapshotHeadDiscovery::Head { snapshot, .. } => Some(snapshot),
        SnapshotHeadDiscovery::NotInitialized | SnapshotHeadDiscovery::Pending { .. } => None,
    }
}
