//! One logical transition kernel for the pre-history container lifecycle.
//! Physical feedback and permanent receipts remain separate admission boundaries.
use crate::business::file_draft::{DraftError as Failure, invalid};
use crate::business::namespace::working::{WorkingEntry, WorkingNamespace};
use crate::{Error, Result, State};
use name_policy::PortablePath;
use storage_contract::file_container::{ContainerNamespace, FileContainerRecord};
use storage_contract::file_container_tags::ContainerTags;
use storage_contract::file_draft::manifest::WorkingManifest;
use storage_contract::file_draft::{
    CommitMode, DraftLocation, FileWorkingState, UnbornWorking, WorkingRevision,
};
use storage_contract::{
    CommitId, DiskId, EntryId, FileId, Generation, OperationId, RepositoryId, Sequence,
};

pub(super) mod first;
pub(crate) use first::replay_version as replay_first_version;
mod replay;
mod current;
pub(crate) use first::{FirstCommit, build as build_first_content};
pub(crate) use replay::{
    birth as replay_birth, first as replay_first, save as replay_save, finish_state,
};
pub(crate) use current::{validate as validate_current, validate_record as validate_current_record};

pub(crate) struct Birth {
    pub(crate) repository: RepositoryId,
    pub(crate) path: PortablePath,
    pub(crate) expected_namespace_head: CommitId,
    pub(crate) file: FileId,
    pub(crate) entry: EntryId,
    pub(crate) owner: DiskId,
}

/// Allocation facts are supplied once by the real mutation and replayed from
/// its original receipt. In particular, a duplicate name never allocates an
/// alternative identity or selects an equal-byte existing container.
pub(crate) enum Edit<'a> {
    Birth(&'a Birth),
    Save {
        target: &'a DraftLocation,
        expected: WorkingRevision,
        manifest: &'a WorkingManifest,
    },
    SetMode {
        target: &'a DraftLocation,
        expected: WorkingRevision,
        mode: CommitMode,
    },
    FirstCommit(FirstCommit<'a>),
}

pub(crate) struct Projection {
    state: State,
    target: DraftLocation,
}

impl Projection {
    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    pub(crate) fn target(&self) -> &DraftLocation {
        &self.target
    }

    pub(crate) fn into_state(self) -> State {
        self.state
    }
}

pub(crate) fn selected<'a>(
    namespace: &WorkingNamespace<'a>,
    target: &DraftLocation,
    expected: WorkingRevision,
) -> Result<&'a FileContainerRecord> {
    let path = name_policy::LookupPath::from(&target.path);
    let Some(WorkingEntry::Unborn { file, record, path }) = namespace.get(&path) else {
        return Err(Error::FileDraft(Failure::UnbornRequired));
    };
    if namespace.repository() != target.repository || *file != target.file
        || record.namespace.entry != target.entry || path != &target.path
    {
        return Err(Error::FileDraft(Failure::WorkingNamespace));
    }
    let occurrence = super::ordinary::OrdinaryOccurrence::capture(namespace.state(), *file, record)?;
    occurrence.require_target(target)?;
    occurrence.require_visible(|path| match namespace.get(&name_policy::LookupPath::from(path)) {
        Some(WorkingEntry::Published(entry)) => Some(entry),
        _ => None,
    })?;
    if record.working.revision() != expected {
        return Err(Error::FileDraft(Failure::RevisionChanged {
            expected,
            actual: record.working.revision(),
        }));
    }
    Ok(*record)
}

pub(crate) fn require_fresh(previous: &State, id: FileId, target_entry: EntryId) -> Result<()> {
    let file = id.to_string();
    if previous.file_containers.contains_key(&id)
        || previous.container_tags.contains_key(&id)
        || previous.versions.values().any(|version| version.file_id == file)
        || previous.branches.values().any(|branch| branch.file_id.as_ref() == Some(&file))
        || previous.commits.values().any(|commit| commit.entries.iter().any(|entry| {
            entry.entry_id == target_entry.to_string() || entry.file_id == file
        }))
        || previous.file_containers.values().any(|record| record.namespace.entry == target_entry)
    {
        return Err(invalid());
    }
    Ok(())
}

fn birth_record(
    namespace: &WorkingNamespace<'_>,
    birth: &Birth,
    operation: OperationId,
) -> Result<FileContainerRecord> {
    require_fresh(namespace.state(), birth.file, birth.entry)?;
    let binding: ContainerNamespace = namespace.namespace_for_birth(&birth.path, birth.entry)?;
    Ok(FileContainerRecord {
        history_tree_id: namespace.history(),
        owner_disk: birth.owner,
        created_by: operation,
        namespace: binding,
        working: FileWorkingState::Unborn { working: UnbornWorking::birth() },
        // This is a logical preparation, never an admitted snapshot. The actual
        // EXCL container stage must supply the only owner occurrence before a
        // Candidate can exist; zero or caller-invented inodes are not inserted.
        replicas: std::collections::BTreeMap::new(),
    })
}

pub(crate) fn begin(previous: &State, sequence: Sequence) -> Result<State> {
    let mut next = previous.clone();
    next.begin_tracking();
    crate::container_storage::capture_names(&mut next);
    next.sequence = sequence.get();
    next.business_generation = Generation::new(previous.business_generation).next()
        .map_err(|source| Error::FileDraft(Failure::PublicationGeneration(source)))?.get();
    Ok(next)
}

/// No disk effects, receipt, or writer capability. Both live planning and cold
/// replay must consume this same complete logical projection, then attach only
/// the exact physical feedback proved by their container/extent producer.
pub(crate) fn project(
    previous: &State,
    config: &crate::ValidatedConfig,
    operation: OperationId,
    sequence: Sequence,
    edit: Edit<'_>,
    maximum_entries: usize,
    maximum_bytes: u64,
) -> Result<Projection> {
    if previous.operations.contains_key(&operation) || sequence.get() <= previous.sequence {
        return Err(invalid());
    }
    let repository = match &edit {
        Edit::Birth(birth) => birth.repository,
        Edit::Save { target, .. } | Edit::SetMode { target, .. } => target.repository,
        Edit::FirstCommit(commit) => commit.target.repository,
    };
    let namespace = WorkingNamespace::from_main_state(previous, repository, maximum_entries)?;
    let mut next = begin(previous, sequence)?;
    let target = match edit {
        Edit::Birth(birth) => {
            let root = previous.repositories.get(&repository.to_string()).ok_or_else(invalid)?;
            let head = previous.branches.get(&root.main_branch_id).ok_or_else(invalid)?;
            let disk = config.disks.iter().find(|disk| {
                disk.slot_id.eq_ignore_ascii_case(&root.primary_slot_id)
            }).ok_or_else(invalid)?;
            if head.head_commit_id.parse::<CommitId>()? != birth.expected_namespace_head {
                return Err(Error::FileDraft(Failure::SourceChanged));
            }
            if disk.level != 1 || disk.upstream_slot_id.is_some()
                || disk.disk_id.parse::<DiskId>()? != birth.owner
            {
                return Err(Error::FileDraft(Failure::UnsupportedScope));
            }
            let record = birth_record(&namespace, birth, operation)?;
            next.file_containers.insert(birth.file, record);
            next.container_tags.insert(birth.file, ContainerTags::empty());
            DraftLocation {
                repository,
                path: birth.path.clone(),
                entry: birth.entry,
                file: birth.file,
            }
        }
        Edit::Save { target, expected, manifest } => {
            let record = selected(&namespace, target, expected)?;
            let working = record.working.unborn().ok_or_else(invalid)?;
            if manifest.operation() != operation {
                return Err(invalid());
            }
            let working = working.saved(target.file, manifest.clone(), maximum_bytes)
                .map_err(|source| Error::FileDraft(Failure::WorkingManifest(source)))?;
            next.file_containers.get_mut(&target.file).ok_or_else(invalid)?.working =
                FileWorkingState::Unborn { working };
            target.clone()
        }
        Edit::SetMode { target, expected, mode } => {
            let record = selected(&namespace, target, expected)?;
            let working = record.working.unborn().ok_or_else(invalid)?;
            let working = working.change_mode(mode)
                .map_err(|source| Error::FileDraft(Failure::RevisionExhausted(source)))?;
            next.file_containers.get_mut(&target.file).ok_or_else(invalid)?.working =
                FileWorkingState::Unborn { working };
            target.clone()
        }
        Edit::FirstCommit(commit) => {
            first::project(previous, config, &namespace, &mut next, operation, &commit)?;
            commit.target.clone()
        }
    };
    // Birth, save and mode keep history unchanged. Explicit first commit adds
    // only its real first Version/branch; every case uses the same current union.
    crate::container_storage::capture_changed_names(previous, &mut next);
    WorkingNamespace::from_main_state(&next, repository, maximum_entries)?;
    Ok(Projection { state: next, target })
}
