//! Seed two real, distinct FILE merges, not lifecycle events fabricated for the
//! UI. The last merge is the final pool mutation: later HEADs are not evidence
//! of this exact observation. No whole-container delete/undo is required.
use super::{Evidence, FINAL, Result};
use storage_contract::{
    BranchName, Entry, FileMergeInput, OperationId,
    history_window::{HistoryCollection, HistoryPageLimit},
};
use storage_engine::{CommitRequest, Engine, FileBranchMergeRequest};

const ACTOR: &str = "smoke-admin";

fn save(e: &mut Engine, branch: &str, value: usize) -> Result<Entry> {
    let ws = e.create_workspace(branch, ACTOR)?;
    let bytes = format!("real ordered timeline body {value}");
    e.stage_bytes(&ws.workspace_id, FINAL, bytes.as_bytes())?;
    e.commit_workspace(CommitRequest {
        workspace_id: ws.workspace_id,
        expected_head: ws.base_head,
        fencing_token: ws.fencing_token,
        principal: ACTOR.into(),
        operation_id: OperationId::generate()?,
        message: format!("retain actual body {value}"),
        request_digest: None,
    })?;
    e.list_tree(branch)?.into_iter().find(|r| r.path() == FINAL)
        .ok_or_else(|| "the actual FILE publication is missing".into())
}

fn pin(e: &mut Engine, body: &Entry, name: &str) -> Result<String> {
    e.create_file_branch(
        &body.file_id, &body.version_id, &BranchName::parse(name)?, ACTOR, OperationId::generate()?,
    )?.branch_id.ok_or_else(|| "the original FILE branch creation returned no selected branch".into())
}

fn merge(
    e: &mut Engine, main: &str, other: &str, main_body: &Entry, pinned: &Entry,
) -> Result<storage_contract::OperationResult> {
    if main_body.file_id != pinned.file_id || main == other
        || e.file_branch_head(main)? != main_body.version_id
        || e.file_branch_head(other)? != pinned.version_id
    {
        return Err("actual merge inputs are not the two selected FILE branch heads".into());
    }
    let inputs = [
        FileMergeInput { branch_id: main.into(), expected_node_id: main_body.version_id.clone() },
        FileMergeInput { branch_id: other.into(), expected_node_id: pinned.version_id.clone() },
    ];
    let operation_id = OperationId::generate()?;
    let versions = e.state().versions.len();
    let outcome = e.merge_file_branches(FileBranchMergeRequest {
        file_id: &main_body.file_id,
        inputs: &inputs,
        chosen_node_id: &main_body.version_id,
        surviving_branch_id: main,
        location: None,
        principal: ACTOR,
        operation_id,
    })?;
    if outcome.operation_id != operation_id || e.state().versions.len() != versions
        || e.file_branch_head(main)? != main_body.version_id
    {
        return Err("the original merge did not adopt the selected existing body".into());
    }
    Ok(outcome)
}

pub(super) fn seed(temp: &tempfile::TempDir) -> Result<Evidence> {
    let mut e = Engine::open(&temp.path().join("pool"))?;
    let slot = e.config().disks[0].slot_id.clone();
    let repo = e.create_disk_root(&slot, ACTOR, OperationId::generate()?)?;
    let old = save(&mut e, &repo.main_branch_id, 0)?;
    super::super::fixture::initialize_directory_fixture(&mut e, &repo, "")?;
    let a = pin(&mut e, &old, "older pinned input")?;
    let main = e.file_context(&repo.repository_id, FINAL)
        .ok_or("registered path has no admitted FILE branch context")?.branch_id;
    if e.file_branch_head(&main)? != old.version_id {
        return Err("registered FILE main does not start at the original body".into());
    }
    // Pin old on a, then push it outside the normal page BEFORE either boundary.
    // Thus complete interval membership needs only the two intervening bodies.
    let limit = HistoryPageLimit::default_for(&HistoryCollection::Nodes).get();
    let mut lower = old.clone();
    for value in 1..=limit + 1 {
        lower = save(&mut e, &main, value)?;
    }
    let older = merge(&mut e, &main, &a, &lower, &old)?;
    let b = pin(&mut e, &lower, "upper pinned input")?;
    let one = save(&mut e, &main, limit + 2)?;
    let two = save(&mut e, &main, limit + 3)?;
    if old.file_id != one.file_id || one.file_id != two.file_id
        || e.file_node_parents(&two.file_id, &two.version_id)? != [one.version_id.clone()]
    {
        return Err("the real middle FILE contents do not have the expected parent".into());
    }
    let newer = merge(&mut e, &main, &b, &two, &lower)?;
    if e.state().sequence != newer.sequence {
        return Err("the final merge did not produce the admitted observation".into());
    }
    Ok(Evidence {
        slot, file: old.file_id, old: old.version_id, one: one.version_id, two: two.version_id,
        older: older.operation_id.to_string(), newer: newer.operation_id.to_string(),
        observation: newer.sequence.to_string(),
    })
}
