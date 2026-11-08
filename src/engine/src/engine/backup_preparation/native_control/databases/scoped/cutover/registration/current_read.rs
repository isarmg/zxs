//! All-participant read admission after common database installation. No
//! progress file is an activation proof and no source fence is released here.
use super::*;
use crate::engine::disk_registry::current::backup::BackupNamespaceAdmission;
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

/// Fresh common database/native Publication observation plus every target's
/// exact namespace loan. This is deliberately not a writer/Engine capability.
#[derive(Clone, Debug)]
pub struct BackupCurrentReadReport {
    pub common: BackupCommonDatabasesReport,
    pub targets: usize,
    pub branch_scopes: usize,
    pub readable_branch_scopes: usize,
    pub directory_scopes: usize,
    pub entries_per_target: usize,
    pub deleted_files: usize,
    pub unavailable_versions: usize,
}

#[derive(PartialEq, Eq)]
struct Counts {
    branches: usize,
    readable_branches: usize,
    directories: usize,
    entries: usize,
    deletions: usize,
    unavailable: usize,
}
fn count(view: &BackupNamespaceAdmission<'_>, stop: &AtomicBool) -> Result<Counts> {
    let state = view.state();
    let mut result = Counts {
        branches: state.branches.len(),
        readable_branches: 0,
        directories: state.directory_branches.len(),
        entries: 0,
        deletions: state.file_deletions.len(),
        unavailable: state
            .content_states
            .values()
            .filter(|value| {
                matches!(
                    value,
                    ContentState::Purged | ContentState::Migrated | ContentState::Retired
                )
            })
            .count(),
    };
    for (id, branch) in &state.branches {
        preparation_checkpoint(stop)?;
        // Historical migrated branches stay in the full snapshot, but never
        // become a readable namespace just because this is a new generation.
        if branch.state == BranchState::Migrated {
            continue;
        }
        result.readable_branches += 1;
        result.entries = result
            .entries
            .checked_add(view.branch(id)?.1.len())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "Backup namespace entry count overflow",
                )
            })?;
    }
    for id in state.directory_branches.keys() {
        preparation_checkpoint(stop)?;
        result.entries = result
            .entries
            .checked_add(view.directory(id)?.1.len())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "Backup namespace entry count overflow",
                )
            })?;
    }
    Ok(result)
}

pub(super) fn observe(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupCurrentReadReport> {
    let before = common_databases::observe(context, prepared, decision, expected, stop)?;
    let plan = published_registry::Plan::capture(prepared, decision, expected, stop)?;
    let mut counts = None;
    for index in 0..prepared.targets.len() {
        preparation_checkpoint(stop)?;
        let current =
            plan.with_namespaces(prepared, index, expected, stop, |view| count(view, stop))?;
        if counts.as_ref().is_some_and(|previous| previous != &current) {
            return Err(invalid(
                "Backup targets disagree on their exact published namespace",
            ));
        }
        counts = Some(current);
    }
    // Old member journals, all source bindings and every real database/body
    // are reobserved before returning a common result. No online subset wins.
    let common = common_databases::observe(context, prepared, decision, expected, stop)?;
    if common.common_manifest_hash != before.common_manifest_hash {
        return Err(invalid(
            "Backup common database binding changed during namespace admission",
        ));
    }
    let counts = counts.expect("invariant: original Backup selection has at least one target");
    Ok(BackupCurrentReadReport {
        common,
        targets: prepared.targets.len(),
        branch_scopes: counts.branches,
        readable_branch_scopes: counts.readable_branches,
        directory_scopes: counts.directories,
        entries_per_target: counts.entries,
        deleted_files: counts.deletions,
        unavailable_versions: counts.unavailable,
    })
}
