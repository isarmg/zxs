//! Initial native backfill uses the same retained transform and ordinary writer.
//! The request has no prefix strings. Actual root registration selects geometry;
//! only the ordinary preparation/decision path creates its prefix entities.
use crate::engine::{PlanningContext, Prepared, ReadSource};
mod archive;
pub(crate) use archive::{VerifiedArchivedAutomaticBackup, verify as verify_archived};
use crate::engine::fresh_layout::containers::VerifiedContainerTransition;
use crate::persist::automatic_backup::{
    self as codec, FileSelection, Input, NamedDisk, PolicyId, Selection, Source,
};
use crate::protection::backup_paths::{
    Scope, ScopedBackupConfigurationRequest, backup_primaries_in, effective_policy_in,
    exact_policy, inherited_by_scope, initial, namespace, require_committed_backup_files,
    require_published_backup_scope, within,
};
use crate::{Engine, Error, ErrorCode, Event, FileVersion, OperationResult, Placement, Result, State};
use name_policy::PortablePath;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{
    BranchId, DiskId, EntryId, FileId, Generation, OperationId, Principal, RepositoryId,
};

#[derive(Debug, error_definition::Error)]
pub enum AutomaticBackupError {
    #[error("automatic container backup requires the original initial namespace")]
    InitialOnly,
    #[error("automatic container backup does not match its registered source")]
    Source,
    #[error("automatic container backup differs from its exact retained request")]
    Publication,
    #[error("automatic container backup publication has invalid encoded evidence: {0}")]
    Encoding(#[source] json_codec::Error),
    #[error("automatic container backup exhausted the business generation")]
    Generation,
    #[error("automatic file backup no longer names its exact repository occurrence")]
    FileScope,
}
impl AutomaticBackupError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InitialOnly => ErrorCode::UnsupportedOperation,
            Self::Source | Self::Publication | Self::Encoding(_) => ErrorCode::PublicationInvalid,
            Self::Generation => ErrorCode::ResourceLimit,
            Self::FileScope => ErrorCode::ScopeChanged,
        }
    }
}
pub(crate) fn invalid() -> Error {
    Error::AutomaticBackup(AutomaticBackupError::Publication)
}
fn wrong_source() -> Error {
    Error::AutomaticBackup(AutomaticBackupError::Source)
}

/// Reprotect every retained available version in this Initial namespace scope.
/// The caller chooses a copy count, never a mount path or source prefix.
#[derive(Clone, Copy, Debug)]
pub struct AutomaticBackupConfigurationRequest<'a> {
    pub repository_id: RepositoryId,
    pub path: Option<&'a PortablePath>,
    pub target_copies: u32,
    pub expected_generation: Generation,
    pub principal: &'a Principal,
    pub operation_id: OperationId,
}

/// Explicitly protect a real ordinary directory, including empty descendants.
/// Independent child policies keep their own scope. A pathname alone is not the
/// directory fence; Existing uses this complete request before mutable checks.
#[derive(Clone, Copy, Debug)]
pub struct AutomaticDirectoryBackupRequest<'a> {
    pub repository_id: RepositoryId,
    pub path: &'a PortablePath,
    pub directory_id: EntryId,
    pub target_copies: u32,
    pub expected_generation: Generation,
    pub principal: &'a Principal,
    pub operation_id: OperationId,
}

/// Protect one exact current occurrence; every retained version of its file
/// remains part of the same ordinary full-copy transaction and permanent receipt.
#[derive(Clone, Copy, Debug)]
pub struct AutomaticFileBackupRequest<'a> {
    pub repository_id: RepositoryId,
    pub path: &'a PortablePath,
    pub entry_id: EntryId,
    pub file_id: FileId,
    pub target_copies: u32,
    pub expected_generation: Generation,
    pub principal: &'a Principal,
    pub operation_id: OperationId,
}

pub(crate) struct DirectoryBackupSelection {
    entries: BTreeSet<EntryId>,
    route: Vec<DiskId>,
}
impl DirectoryBackupSelection {
    pub(crate) fn entries(&self) -> impl Iterator<Item = EntryId> + '_ {
        self.entries.iter().copied()
    }

    pub(crate) fn route(&self) -> &[DiskId] {
        &self.route
    }
}

impl Engine {
    pub fn configure_automatic_backups(
        &mut self,
        request: AutomaticBackupConfigurationRequest<'_>,
    ) -> Result<OperationResult> {
        let input = Input {
            repository_id: request.repository_id,
            path: request.path.cloned(),
            target_copies: request.target_copies,
            expected_generation: request.expected_generation,
        };
        self.configure_automatic_backup_input(input, Selection::RetainedFiles,
            request.principal, request.operation_id)
    }

    pub fn configure_automatic_directory_backups(
        &mut self, request: AutomaticDirectoryBackupRequest<'_>,
    ) -> Result<OperationResult> {
        let input = Input {
            repository_id: request.repository_id, path: Some(request.path.clone()),
            target_copies: request.target_copies,
            expected_generation: request.expected_generation,
        };
        self.configure_automatic_backup_input(input, Selection::DirectoryTree(request.directory_id),
            request.principal, request.operation_id)
    }

    pub fn configure_automatic_file_backups(
        &mut self,
        request: AutomaticFileBackupRequest<'_>,
    ) -> Result<OperationResult> {
        let input = Input {
            repository_id: request.repository_id,
            path: Some(request.path.clone()),
            target_copies: request.target_copies,
            expected_generation: request.expected_generation,
        };
        let selection = Selection::File(FileSelection {
            entry_id: request.entry_id,
            file_id: request.file_id,
        });
        self.configure_automatic_backup_input(
            input, selection, request.principal, request.operation_id,
        )
    }

    fn configure_automatic_backup_input(
        &mut self, input: Input, selection: Selection,
        principal: &Principal, operation_id: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(selection.kind(), principal.as_str(), operation_id,
            codec::selected_request(&input, selection)?, |engine, next, pending| {
                let context = engine.planning_context();
                let repository = input.repository_id.to_string();
                let prefixes = BTreeMap::new();
                let expected_scope = selection.directory().map(|entry| entry.to_string());
                let mut scoped = input.scoped(&repository, &prefixes,
                    principal.as_str(), operation_id);
                scoped.expected_scope_id = expected_scope.as_deref();
                require_published_backup_scope(engine, &repository, scoped.path)?;
                let (scope, previous) = select_scope(next, &input, selection, scoped)?;
                require_committed_backup_files(engine.state(), scope.files.keys())?;
                let source = source(&context, &input, &scope)?;
                directory_selection(context.config, next, &input, selection, &source,
                    &scope, previous.as_ref())?;
                let mut generated = Vec::new();
                let configured = initial::apply(
                    |primary, copies| initial::live_route(&context, primary, copies),
                    next, scoped, scope, previous, BTreeMap::new(),
                    || {
                        let id = PolicyId::generate()?;
                        if context.state.protection_policies.contains_key(&id.to_string())
                            || generated.contains(&id)
                        {
                            return Err(invalid());
                        }
                        generated.push(id);
                        Ok(id.to_string())
                    },
                    pending,
                    |version, placement| Ok(Prepared {
                        version: version.clone(), placement: placement.clone(),
                        source: engine.valid_backup_replica(&version.version_id)?,
                    }),
                )?;
                codec::selected_outcome(&input, selection, &source, &generated,
                    &engine.state().repositories[&repository].history_tree_id,
                    &configured, crate::business::now()?)
            })
    }
}

fn select_scope(
    state: &State,
    input: &Input,
    selection: Selection,
    request: ScopedBackupConfigurationRequest<'_>,
) -> Result<(Scope, Option<crate::ProtectionPolicy>)> {
    let Selection::File(selected) = selection else {
        return initial::select(state, request);
    };
    let path = input.path.as_ref().ok_or_else(invalid)?;
    let current = namespace(state, request.repository_id)?;
    // Resolve the requested key once. Do not use source_scope's per-FileId
    // preferred-occurrence reduction to manufacture this entry proof.
    let entry = current.entries.iter()
        .find(|entry| entry.lookup_key() == path.key())
        .filter(|entry| {
            entry.kind == crate::NamespaceEntryKind::File
                && entry.entry_id == selected.entry_id.to_string()
                && entry.file_id == selected.file_id.to_string()
        })
        .ok_or(Error::AutomaticBackup(AutomaticBackupError::FileScope))?;
    let scope = Scope {
        id: entry.file_id.clone(),
        kind: entry.kind,
        path: entry.path().to_owned(),
        files: BTreeMap::from([(entry.file_id.clone(), entry.clone())]),
    };
    initial::select_scope(state, request, scope)
}

fn source(context: &PlanningContext<'_>, input: &Input, scope: &Scope) -> Result<Source> {
    source_for_replay(context, input, scope, None)
}

fn source_for_replay(
    context: &PlanningContext<'_>, input: &Input, scope: &Scope,
    readonly: Option<&crate::engine::ReadOnlyPublishedCandidate<'_>>,
) -> Result<Source> {
    if context.current_namespaces.is_some() {
        return Err(Error::AutomaticBackup(AutomaticBackupError::InitialOnly));
    }
    context.config.require_initial_backup_source(context.state)?;
    let repository = input.repository_id.to_string();
    let repo = context.state.repositories.get(&repository).ok_or_else(wrong_source)?;
    let published = readonly.map(|proof| context.published_namespace_metadata(
        proof, input.repository_id, repo.main_branch_id.parse()?,
    )).transpose()?;
    let origin = if published.is_none() {
        Some(context.namespace_host_root(input.repository_id, repo.main_branch_id.parse()?)?)
    } else { None };
    let (binding, disk) = match (&published, &origin) {
        (Some(source), None) => (source.binding(), source.disk()),
        (None, Some(source)) => (source.binding(), source.disk()),
        _ => return Err(wrong_source()),
    };
    let primaries = backup_primaries_in(context.state,
        &namespace(context.state, &repository)?.entries, &repository, scope,
        |file, path| context.file_placement_settings(&repository, file, path)
            .map(|settings| settings.0))?;
    source_geometry(
        context.config, context.state, input, scope, binding, disk.disk_id.parse()?,
        primaries, |slot| context.disk(slot), |primary, copies| context.route(primary, copies),
        |id| match &published {
            Some(source) => source.require_backup_target(id),
            None => {
                // New/Accepted preparation still requires every original held source.
                context.observe_backup_source_path(input.repository_id,
                    repo.main_branch_id.parse()?, id)?.require_context(context)
            }
        },
    )
}

fn source_geometry<'a>(
    config: &crate::Config,
    state: &State,
    input: &Input,
    scope: &Scope,
    binding: &storage_contract::working_tree::RootBinding,
    owner: DiskId,
    primaries: BTreeSet<String>,
    disk: impl Fn(&str) -> Result<&'a crate::DiskConfig>,
    select_route: impl Fn(&str, u32) -> Result<Vec<String>>,
    require_target: impl Fn(DiskId) -> Result<()>,
) -> Result<Source> {
    let repository = input.repository_id.to_string();
    if exact_policy(state, &repository, scope)
        .is_some_and(|policy| !policy.backup_prefixes.is_empty())
        || scope.files.keys().any(|file| state.file_backup_paths.get(file)
            .is_some_and(|paths| !paths.is_empty()))
    {
        return Err(wrong_source());
    }
    let mut route = None;
    for primary in primaries {
        if disk(&primary)?.disk_id.parse::<DiskId>()? != owner {
            return Err(wrong_source());
        }
        let selected = select_route(&primary, input.target_copies)?;
        if route.as_ref().is_some_and(|old| old != &selected) {
            return Err(wrong_source());
        }
        route = Some(selected);
    }
    let route = route.ok_or_else(wrong_source)?;
    let mut disks = Vec::with_capacity(route.len());
    for slot in &route {
        let disk = disk(slot)?;
        let id = disk.disk_id.parse::<DiskId>()?;
        if id != owner {
            require_target(id)?;
        }
        disks.push(NamedDisk::from_config(config, disk)?);
    }
    Ok(Source { root: binding.clone(), disks })
}

fn directory_selection(
    config: &crate::Config, state: &State, input: &Input, selection: Selection,
    source: &Source, scope: &Scope, previous: Option<&crate::ProtectionPolicy>,
) -> Result<Option<DirectoryBackupSelection>> {
    let Selection::DirectoryTree(expected) = selection else { return Ok(None); };
    if scope.kind != crate::NamespaceEntryKind::Directory
        || scope.id != expected.to_string() || scope.path.is_empty()
    {
        return Err(wrong_source());
    }
    let repository = input.repository_id.to_string();
    let repo = state.repositories.get(&repository).ok_or_else(wrong_source)?;
    let branch = repo.main_branch_id.parse::<BranchId>()?;
    let owner_record = state.namespace_directories.get(&expected.to_string())
        .ok_or_else(wrong_source)?;
    let owner = owner_record.owner_disk;
    if owner_record.repository != input.repository_id || owner_record.branch != branch
        || config.storage_roots.get(&owner) != Some(&source.root)
        || source.root.registration.disk_id.parse::<DiskId>()? != owner
        || source.disks.len() != usize::try_from(input.target_copies).map_err(|_| wrong_source())?
        || source.disks.is_empty() || source.disks.len() > 3
    {
        return Err(wrong_source());
    }
    let mut route = Vec::with_capacity(source.disks.len());
    let mut preceding: Option<&crate::DiskConfig> = None;
    for named in &source.disks {
        let id = named.disk_id()?;
        let disk = config.disks.iter().find(|disk| disk.disk_id == id.to_string())
            .ok_or_else(wrong_source)?;
        if NamedDisk::from_config(config, disk)? != *named || route.contains(&id) {
            return Err(wrong_source());
        }
        match preceding {
            None if id != owner || disk.level != 1 || disk.upstream_slot_id.is_some() => {
                return Err(wrong_source());
            }
            Some(previous) if disk.upstream_slot_id.as_ref()
                .is_none_or(|slot| !slot.eq_ignore_ascii_case(&previous.slot_id))
                || previous.level.checked_add(1) != Some(disk.level) => {
                return Err(wrong_source());
            }
            None | Some(_) => (),
        }
        route.push(id);
        preceding = Some(disk);
    }
    let current = namespace(state, &repository)?;
    let mut entries = BTreeSet::new();
    for entry in current.entries.iter().filter(|entry| {
        entry.kind == crate::NamespaceEntryKind::Directory && within(&scope.path, entry.path())
            && inherited_by_scope(&current.entries, &scope.path, previous,
                effective_policy_in(state, &current.entries, &repository, None, entry.path()))
    }) {
        let id = entry.entry_id.parse::<EntryId>()?;
        for parent in crate::engine::fresh_layout::directories::chain(state, id)? {
            if parent.repository != input.repository_id || parent.branch != branch
                || parent.owner_disk != owner || !parent.replicas.contains_key(&owner)
            {
                return Err(wrong_source());
            }
            entries.insert(parent.entry);
        }
    }
    if !entries.contains(&expected) { return Err(wrong_source()); }
    Ok(Some(DirectoryBackupSelection { entries, route }))
}

/// Pure scope projection for the full container-map verifier. This only binds
/// physical rows to the strict request and original namespace/configuration;
/// the same Candidate must still pass the complete business replay below before
/// publication. No root, preparation or installer is issued here.
pub(crate) fn candidate_directory_selection(
    config: &crate::Config, previous: &State, next: &State, result: &OperationResult,
) -> Result<Option<DirectoryBackupSelection>> {
    if !codec::recognizes(result)? || result.kind != codec::DIRECTORY_KIND {
        return Ok(None);
    }
    if next.operations.get(&result.operation_id) != Some(result)
        || previous.operations.contains_key(&result.operation_id)
        || result.sequence != next.sequence || result.sequence <= previous.sequence
    {
        return Err(invalid());
    }
    let decoded = codec::decode(result)?;
    let principal = next.operation_principals.get(&result.operation_id).ok_or_else(invalid)?;
    let repository = decoded.input.repository_id.to_string();
    let prefixes = BTreeMap::new();
    let expected_scope = decoded.selection.directory().map(|entry| entry.to_string());
    let mut scoped = decoded.input.scoped(&repository, &prefixes, principal, result.operation_id);
    scoped.expected_scope_id = expected_scope.as_deref();
    let (scope, old) = select_scope(previous, &decoded.input, decoded.selection, scoped)?;
    directory_selection(config, previous, &decoded.input, decoded.selection,
        &decoded.source, &scope, old.as_ref())
}

fn reconstruct(
    context: &PlanningContext<'_>,
    next: &State,
    result: &OperationResult,
    readonly: Option<&crate::engine::ReadOnlyPublishedCandidate<'_>>,
) -> Result<(State, Vec<(FileVersion, Placement)>, Option<DirectoryBackupSelection>)> {
    reconstruct_with(
        context.config, context.state, next, result,
        |input, scope| source_for_replay(context, input, scope, readonly),
        |primary, copies| initial::live_route(context, primary, copies),
    )
}

fn reconstruct_with(
    config: &crate::ValidatedConfig,
    previous_state: &State,
    next: &State,
    result: &OperationResult,
    source: impl FnOnce(&Input, &Scope) -> Result<Source>,
    route: impl FnMut(&str, u32) -> Result<initial::InitialRoute>,
) -> Result<(State, Vec<(FileVersion, Placement)>, Option<DirectoryBackupSelection>)> {
    if next.operations.get(&result.operation_id) != Some(result)
        || previous_state.operations.contains_key(&result.operation_id)
        || next.sequence != result.sequence || next.sequence <= previous_state.sequence
    {
        return Err(invalid());
    }
    let principal = next.operation_principals.get(&result.operation_id).ok_or_else(invalid)?;
    Engine::require_actor(principal)?;
    crate::directory_conversion::require_import_not_stopped(previous_state, result.operation_id)?;
    let decoded = codec::decode(result)?;
    let repository = decoded.input.repository_id.to_string();
    let prefixes = BTreeMap::new();
    let expected_scope = decoded.selection.directory().map(|entry| entry.to_string());
    let mut scoped = decoded.input.scoped(&repository, &prefixes, principal, result.operation_id);
    scoped.expected_scope_id = expected_scope.as_deref();
    let mut expected = previous_state.clone();
    expected.begin_tracking();
    crate::container_storage::capture_names(&mut expected);
    expected.sequence = next.sequence;
    expected.business_generation = expected.business_generation.checked_add(1)
        .ok_or(Error::AutomaticBackup(AutomaticBackupError::Generation))?;
    let (scope, previous) = select_scope(&expected, &decoded.input, decoded.selection, scoped)?;
    if source(&decoded.input, &scope)? != decoded.source {
        return Err(wrong_source());
    }
    let directories = directory_selection(config, previous_state, &decoded.input,
        decoded.selection, &decoded.source, &scope, previous.as_ref())?;
    let mut ids = decoded.generated.iter();
    let mut facts = Vec::new();
    let mut seen = BTreeSet::new();
    let configured = initial::apply(
        route, &mut expected, scoped, scope, previous, BTreeMap::new(),
        || {
            let id = *ids.next().ok_or_else(invalid)?;
            if !seen.insert(id)
                || previous_state.protection_policies.contains_key(&id.to_string())
            {
                return Err(invalid());
            }
            Ok(id.to_string())
        },
        &mut facts, |version, placement| Ok((version.clone(), placement.clone())),
    )?;
    if ids.next().is_some() { return Err(invalid()); }
    // Preserve the ordinary non-content gates before any payload/root writer.
    crate::file_deletion::freeze::verify(previous_state, &expected)?;
    crate::business::file_tags::preserve_existing(previous_state, &expected)?;
    crate::container_storage::capture_changed_names(previous_state, &mut expected);
    let receipt = codec::receipt(config, &decoded,
        &previous_state.repositories[&repository].history_tree_id, &configured,
        &facts, result.operation_id, next.sequence, principal)?;
    if receipt != *result { return Err(invalid()); }
    expected.operations.insert(result.operation_id, receipt.clone());
    expected.operation_principals.insert(result.operation_id, principal.clone());
    expected.events.push(Event {
        sequence: next.sequence, operation_id: result.operation_id,
        kind: decoded.selection.kind().into(),
        object_id: receipt.history_tree_id.clone().ok_or_else(invalid)?,
    });
    crate::node_retirement_engine::verify(previous_state, &expected)?;
    crate::file_deletion::publication::verify(previous_state, &expected, None)?;
    Ok((expected, facts, directories))
}

/// Issued only after the one complete logical replay and exact Prepared facts.
/// It permits the same caller to attach physical feedback, not to install it.
pub(crate) struct VerifiedAutomaticBackup {
    directories: Option<DirectoryBackupSelection>,
}
impl VerifiedAutomaticBackup {
    pub(crate) fn directories(&self) -> Option<&DirectoryBackupSelection> {
        self.directories.as_ref()
    }
}

/// A new generation's physical feedback must belong to this exact ordinary
/// automatic request. Full business replay follows the container pair proof;
/// this check alone grants neither an image nor a writer.
pub(crate) fn verify_addition_candidate_source(
    config: &crate::ValidatedConfig,
    previous: &State,
    result: &OperationResult,
    candidate: &crate::journal::Candidate,
) -> Result<()> {
    if !codec::recognizes(result)? || candidate.registry().is_some()
        || candidate.generation() != config.generation_hash()
    {
        return Err(wrong_source());
    }
    config.verify_candidate_membership(candidate)?;
    config.require_initial_backup_source(previous)
}

pub(crate) fn verify_prepared(
    context: &PlanningContext<'_>,
    next: &State,
    result: &OperationResult,
    prepared: &[Prepared],
) -> Result<Option<VerifiedAutomaticBackup>> {
    if !codec::recognizes(result)? { return Ok(None); }
    context.require_write()?;
    let (expected, facts, directories) = reconstruct(context, next, result, None)?;
    if !crate::publication_shape::same_transition(context.state, &expected, next)
        || prepared.len() != facts.len()
    {
        return Err(invalid());
    }
    for (prepared, (version, placement)) in prepared.iter().zip(&facts) {
        if &prepared.version != version || &prepared.placement != placement {
            return Err(invalid());
        }
        let ReadSource::VerifiedReplica(source) = &prepared.source else {
            return Err(invalid());
        };
        source.require_backup_source(context, version)?;
    }
    Ok(Some(VerifiedAutomaticBackup { directories }))
}

pub(crate) fn verify_candidate(
    context: &PlanningContext<'_>,
    next: &State,
    candidate: &crate::journal::Candidate,
    containers: &VerifiedContainerTransition<'_>,
) -> Result<()> {
    verify_candidate_source(context, next, candidate, containers, None)
}

pub(crate) fn verify_readonly_candidate(
    context: &PlanningContext<'_>, next: &State, candidate: &crate::journal::Candidate,
    containers: &VerifiedContainerTransition<'_>,
    source: &crate::engine::ReadOnlyPublishedCandidate<'_>,
) -> Result<()> {
    source.require_pair(context, next, candidate)?;
    verify_candidate_source(context, next, candidate, containers, Some(source))
}

fn verify_candidate_source(
    context: &PlanningContext<'_>, next: &State, candidate: &crate::journal::Candidate,
    containers: &VerifiedContainerTransition<'_>,
    readonly: Option<&crate::engine::ReadOnlyPublishedCandidate<'_>>,
) -> Result<()> {
    let result = next.operations.get(&candidate.operation_id).ok_or_else(invalid)?;
    if !codec::recognizes(result)? { return Ok(()); }
    containers.require_pair(context.state, next, candidate)?;
    if candidate.registry().is_some() || candidate.sequence != next.sequence
        || candidate.request_digest != result.request_digest
        || candidate.pool_id != context.config.pool_id
        || &candidate.previous_snapshot != context.snapshot
    {
        return Err(invalid());
    }
    let (expected, facts, _) = reconstruct(context, next, result, readonly)?;
    verify_reconstructed_candidate(context.state, next, candidate, containers, expected, &facts)
}

fn verify_reconstructed_candidate(
    previous: &State,
    next: &State,
    candidate: &crate::journal::Candidate,
    containers: &VerifiedContainerTransition<'_>,
    mut expected: State,
    facts: &[(FileVersion, Placement)],
) -> Result<()> {
    // The exact same container verifier proved these physical rows. Neither an
    // automatic receipt nor a reconstructed logical plan can supply them.
    containers.attach_feedback(&mut expected)?;
    if !crate::publication_shape::same_transition(previous, &expected, next)
        || facts.len() != candidate.versions.len()
        || facts.iter().zip(&candidate.versions).any(|((version, placement), evidence)| {
            version != &evidence.version || placement != &evidence.placement
        })
    {
        return Err(invalid());
    }
    Ok(())
}
