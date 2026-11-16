//! Initial native backfill uses the same retained transform and ordinary writer.
//! The request has no prefix strings. Actual root registration selects geometry;
//! only the ordinary preparation/decision path creates its prefix entities.
use crate::engine::{PlanningContext, Prepared, ReadSource};
use crate::engine::fresh_layout::containers::VerifiedContainerTransition;
use crate::persist::automatic_backup::{self as codec, Input, NamedDisk, PolicyId, Source};
use crate::protection::backup_paths::{
    Scope, backup_primaries_in, exact_policy, initial, namespace,
};
use crate::{Engine, Error, ErrorCode, Event, FileVersion, OperationResult, Placement, Result, State};
use name_policy::PortablePath;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{DiskId, Generation, OperationId, Principal, RepositoryId};

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
}
impl AutomaticBackupError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InitialOnly => ErrorCode::UnsupportedOperation,
            Self::Source | Self::Publication | Self::Encoding(_) => ErrorCode::PublicationInvalid,
            Self::Generation => ErrorCode::ResourceLimit,
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
        self.mutate(codec::KIND, request.principal.as_str(), request.operation_id,
            codec::request(&input)?, |engine, next, pending| {
                let context = engine.planning_context();
                let repository = input.repository_id.to_string();
                let prefixes = BTreeMap::new();
                let scoped = input.scoped(&repository, &prefixes,
                    request.principal.as_str(), request.operation_id);
                let (scope, previous) = initial::select(next, scoped)?;
                let source = source(&context, &input, &scope)?;
                let mut generated = Vec::new();
                let configured = initial::apply(
                    &context, next, scoped, scope, previous, BTreeMap::new(),
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
                codec::outcome(&input, &source, &generated,
                    &engine.state().repositories[&repository].history_tree_id,
                    &configured, crate::business::now()?)
            })
    }
}

fn source(context: &PlanningContext<'_>, input: &Input, scope: &Scope) -> Result<Source> {
    if context.current_namespaces.is_some() || context.config.generation().is_some() {
        return Err(Error::AutomaticBackup(AutomaticBackupError::InitialOnly));
    }
    let repository = input.repository_id.to_string();
    let repo = context.state.repositories.get(&repository).ok_or_else(wrong_source)?;
    let origin = context.namespace_host_root(input.repository_id, repo.main_branch_id.parse()?)?;
    let primaries = backup_primaries_in(context.state,
        &namespace(context.state, &repository)?.entries, &repository, scope,
        |file, path| context.file_placement_settings(&repository, file, path)
            .map(|settings| settings.0))?;
    if exact_policy(context.state, &repository, scope)
        .is_some_and(|policy| !policy.backup_prefixes.is_empty())
        || scope.files.keys().any(|file| context.state.file_backup_paths.get(file)
            .is_some_and(|paths| !paths.is_empty()))
    {
        return Err(wrong_source());
    }
    let owner = origin.disk().disk_id.parse::<DiskId>()?;
    let mut route = None;
    for primary in primaries {
        if context.disk(&primary)?.disk_id.parse::<DiskId>()? != owner {
            return Err(wrong_source());
        }
        let selected = context.route(&primary, input.target_copies)?;
        if route.as_ref().is_some_and(|old| old != &selected) {
            return Err(wrong_source());
        }
        route = Some(selected);
    }
    let route = route.ok_or_else(wrong_source)?;
    let mut disks = Vec::with_capacity(route.len());
    for slot in &route {
        let disk = context.disk(slot)?;
        let id = disk.disk_id.parse::<DiskId>()?;
        if id != owner {
            // Read-only source admission works during blocked cold replay too.
            // New writes separately pass PlanningContext::require_write, and
            // ContainerPreparation retains the same full geometry/entity gate.
            context.observe_backup_source_path(input.repository_id,
                repo.main_branch_id.parse()?, id)?.require_context(context)?;
        }
        disks.push(NamedDisk::from_config(context.config, disk)?);
    }
    Ok(Source { root: origin.binding().clone(), disks })
}

fn reconstruct(
    context: &PlanningContext<'_>,
    next: &State,
    result: &OperationResult,
) -> Result<(State, Vec<(FileVersion, Placement)>)> {
    if next.operations.get(&result.operation_id) != Some(result)
        || context.state.operations.contains_key(&result.operation_id)
        || next.sequence != result.sequence || next.sequence <= context.state.sequence
    {
        return Err(invalid());
    }
    let principal = next.operation_principals.get(&result.operation_id).ok_or_else(invalid)?;
    Engine::require_actor(principal)?;
    crate::directory_conversion::require_import_not_stopped(context.state, result.operation_id)?;
    let decoded = codec::decode(result)?;
    let repository = decoded.input.repository_id.to_string();
    let prefixes = BTreeMap::new();
    let scoped = decoded.input.scoped(&repository, &prefixes, principal, result.operation_id);
    let mut expected = context.state.as_ref().clone();
    expected.begin_tracking();
    crate::container_storage::capture_names(&mut expected);
    expected.sequence = next.sequence;
    expected.business_generation = expected.business_generation.checked_add(1)
        .ok_or(Error::AutomaticBackup(AutomaticBackupError::Generation))?;
    let (scope, previous) = initial::select(&expected, scoped)?;
    if source(context, &decoded.input, &scope)? != decoded.source {
        return Err(wrong_source());
    }
    let mut ids = decoded.generated.iter();
    let mut facts = Vec::new();
    let mut seen = BTreeSet::new();
    let configured = initial::apply(
        context, &mut expected, scoped, scope, previous, BTreeMap::new(),
        || {
            let id = *ids.next().ok_or_else(invalid)?;
            if !seen.insert(id)
                || context.state.protection_policies.contains_key(&id.to_string())
            {
                return Err(invalid());
            }
            Ok(id.to_string())
        },
        &mut facts, |version, placement| Ok((version.clone(), placement.clone())),
    )?;
    if ids.next().is_some() { return Err(invalid()); }
    // Preserve the ordinary non-content gates before any payload/root writer.
    crate::file_deletion::freeze::verify(context.state, &expected)?;
    crate::business::file_tags::preserve_existing(context.state, &expected)?;
    crate::container_storage::capture_changed_names(context.state, &mut expected);
    let receipt = codec::receipt(context.config, &decoded,
        &context.state.repositories[&repository].history_tree_id, &configured,
        &facts, result.operation_id, next.sequence, principal)?;
    if receipt != *result { return Err(invalid()); }
    expected.operations.insert(result.operation_id, receipt.clone());
    expected.operation_principals.insert(result.operation_id, principal.clone());
    expected.events.push(Event {
        sequence: next.sequence, operation_id: result.operation_id,
        kind: codec::KIND.into(),
        object_id: receipt.history_tree_id.clone().ok_or_else(invalid)?,
    });
    crate::node_retirement_engine::verify(context.state, &expected)?;
    crate::file_deletion::publication::verify(context.state, &expected, None)?;
    Ok((expected, facts))
}

pub(crate) fn verify_prepared(
    context: &PlanningContext<'_>,
    next: &State,
    result: &OperationResult,
    prepared: &[Prepared],
) -> Result<()> {
    if !codec::recognizes(result)? { return Ok(()); }
    context.require_write()?;
    let (expected, facts) = reconstruct(context, next, result)?;
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
    Ok(())
}

pub(crate) fn verify_candidate(
    context: &PlanningContext<'_>,
    next: &State,
    candidate: &crate::journal::Candidate,
    containers: &VerifiedContainerTransition<'_>,
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
    let (mut expected, facts) = reconstruct(context, next, result)?;
    // The exact same container verifier proved these physical rows. Neither an
    // automatic receipt nor a reconstructed logical plan can supply them.
    containers.attach_feedback(&mut expected)?;
    if !crate::publication_shape::same_transition(context.state, &expected, next)
        || facts.len() != candidate.versions.len()
        || facts.iter().zip(&candidate.versions).any(|((version, placement), evidence)| {
            version != &evidence.version || placement != &evidence.placement
        })
    {
        return Err(invalid());
    }
    Ok(())
}
