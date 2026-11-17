//! Read-only replay of an authenticated original Backup archive publication.
//! Source geometry comes from that original Config and permanent namespace
//! registration, never the successor Config or a reopened old owner's root.
use crate::engine::ArchivedBackupPublication;
use crate::persist::automatic_backup::{self as codec, Input, Source};
use crate::protection::backup_paths::automatic::{
    invalid, reconstruct_with, source_geometry, verify_reconstructed_candidate, wrong_source,
};
use crate::protection::backup_paths::{Scope, backup_primaries_in, initial, namespace};
use crate::{Result, State};

/// The whole original logical/physical delta was reconstructed from the exact
/// native request. No serialization or constructor outside this verifier.
pub(crate) struct VerifiedArchivedAutomaticBackup<'a> {
    previous: &'a State,
    next: &'a State,
}
impl VerifiedArchivedAutomaticBackup<'_> {
    pub(crate) fn states(&self) -> (&State, &State) {
        (self.previous, self.next)
    }
}

pub(crate) fn verify<'a>(
    source: &'a ArchivedBackupPublication<'_>,
) -> Result<Option<VerifiedArchivedAutomaticBackup<'a>>> {
    let candidate = source.candidate();
    let previous = source.previous();
    let next = source.next();
    let config = source.config();
    let result = next
        .operations
        .get(&candidate.operation_id)
        .ok_or_else(invalid)?;
    // Current's native receipts also contain generated_policy_ids. They must
    // retain their own verifier rather than being decoded as automatic requests.
    if ![codec::KIND, codec::DIRECTORY_KIND, codec::FILE_KIND].contains(&result.kind.as_str()) {
        return Ok(None);
    }
    if candidate.generation().is_some()
        || candidate.registry().is_some()
        || config.generation().is_some()
        || candidate.sequence != next.sequence
        || candidate.request_digest != result.request_digest
        || candidate.pool_id != config.pool_id
        || candidate.previous_snapshot.as_deref() != source.snapshot()
    {
        return Err(invalid());
    }
    config.verify_original_candidate_membership(candidate)?;
    config.require_initial_backup_source(previous)?;
    let containers = crate::engine::fresh_layout::containers::verify_transition(
        config, previous, next, candidate,
    )?;
    let (expected, facts, _) = reconstruct_with(
        config,
        previous,
        next,
        result,
        |input, scope| archived_source(source, input, scope),
        |primary, copies| {
            initial::InitialRoute::bind(route(config, primary, copies)?, |slot| disk(config, slot))
        },
    )?;
    containers.require_pair(previous, next, candidate)?;
    verify_reconstructed_candidate(previous, next, candidate, &containers, expected, &facts)?;
    Ok(Some(VerifiedArchivedAutomaticBackup { previous, next }))
}

fn disk<'a>(config: &'a crate::Config, slot: &str) -> Result<&'a crate::DiskConfig> {
    config
        .disks
        .iter()
        .find(|disk| disk.slot_id.eq_ignore_ascii_case(slot))
        .ok_or_else(wrong_source)
}

fn route(config: &crate::Config, primary: &str, copies: u32) -> Result<Vec<String>> {
    crate::route_topology::Topology::new(config.disks.iter().map(|disk| {
        crate::route_topology::Node {
            slot: &disk.slot_id,
            upstream: disk.upstream_slot_id.as_deref(),
            level: disk.level,
        }
    }))?
    .unique_prefix(primary, copies)
}

fn archived_source(
    source: &ArchivedBackupPublication<'_>,
    input: &Input,
    scope: &Scope,
) -> Result<Source> {
    let config = source.config();
    let state = source.previous();
    let repository = input.repository_id.to_string();
    let repo = state
        .repositories
        .get(&repository)
        .ok_or_else(wrong_source)?;
    let metadata = source.namespace_metadata(input.repository_id, repo.main_branch_id.parse()?)?;
    let primaries = backup_primaries_in(
        state,
        &namespace(state, &repository)?.entries,
        &repository,
        scope,
        |_, path| {
            Ok(
                crate::protection::backup_paths::configured_primary(state, &repository, path)
                    .into(),
            )
        },
    )?;
    source_geometry(
        config,
        state,
        input,
        scope,
        metadata.binding(),
        metadata.disk().disk_id.parse()?,
        primaries,
        |slot| disk(config, slot),
        |primary, copies| route(config, primary, copies),
        |id| metadata.require_backup_target(id),
    )
}
