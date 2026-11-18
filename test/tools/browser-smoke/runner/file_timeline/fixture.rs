//! All expected IDs are read from actual original publications in the private
//! test pool, never synthesized by the browser hook. The current ordinary
//! fresh-layout delete/undo gate stops this positive seed before Chrome begins.
//! This is a known missing reachable fixture, not an ignored assertion, a pass,
//! or authority to inject a ledger event in place of a real mutation.
use super::{Evidence, FINAL, Result};
use storage_contract::{Entry, OperationId, history_window::{HistoryCollection, HistoryFileId, HistoryPageLimit, HistoryRepositoryId}};
use storage_engine::{CommitRequest, DeleteFileRequest, Engine, FileDeletionTarget, NamespaceOperation, Repository, UndoFileDeletionRequest};

const ACTOR: &str = "smoke-admin";

fn save(e: &mut Engine, repo: &Repository, path: &str, value: usize) -> Result<Entry> {
    let ws = e.create_workspace(&repo.main_branch_id, ACTOR)?;
    let bytes = format!("real ordered timeline body {value}");
    e.stage_bytes(&ws.workspace_id, path, bytes.as_bytes())?;
    e.commit_workspace(CommitRequest { workspace_id: ws.workspace_id, expected_head: ws.base_head,
        fencing_token: ws.fencing_token, principal: ACTOR.into(), operation_id: OperationId::generate()?,
        message: format!("retain the original body {value}"), request_digest: None })?;
    e.list_tree(&repo.main_branch_id)?.into_iter().find(|r|r.path()==path)
        .ok_or_else(||"the actual FILE publication is missing".into())
}
fn rename(e: &mut Engine, repo: &Repository, from: &str, to: &str) -> Result<OperationId> {
    let op=OperationId::generate()?;
    e.file_namespace(&repo.repository_id, NamespaceOperation::Rename {
        source: name_policy::LookupPath::parse(from)?, destination: name_policy::PortablePath::parse(to)?,
    },ACTOR,op)?;
    Ok(op)
}
pub(super) fn seed(temp: &tempfile::TempDir) -> Result<Evidence> {
    let mut e=Engine::open(&temp.path().join("pool"))?;
    let slot=e.config().disks[0].slot_id.clone();
    let repo=e.create_disk_root(&slot, ACTOR, OperationId::generate()?)?;
    let first=save(&mut e,&repo,"timeline-original.txt",0)?;
    super::super::fixture::initialize_directory_fixture(&mut e,&repo,"")?;
    let older=rename(&mut e,&repo,"timeline-original.txt",FINAL)?;
    let one=save(&mut e,&repo,FINAL,1)?;
    let two=save(&mut e,&repo,FINAL,2)?;
    // A path fact is not a complete branch inventory: the *actual original
    // deletion* freezes the upper-end branch heads. Undo restores this FILE.
    let file=HistoryFileId::parse(&first.file_id)?;
    let newer=OperationId::generate()?;
    e.soft_delete_file(DeleteFileRequest { target: FileDeletionTarget {
        repository_id:&HistoryRepositoryId::parse(&repo.repository_id)?, file_id:&file, path:&name_policy::LookupPath::parse(FINAL)?,
    },principal:ACTOR,operation_id:newer })?;
    let deletion=e.file_deletion(&first.file_id).ok_or("missing original deletion")?.node_id.clone();
    let undo=OperationId::generate()?;
    e.undo_file_deletion(UndoFileDeletionRequest{file_id:&file, deletion_node_id:&deletion,
        principal:ACTOR,operation_id:undo})?;
    // Force the original node outside the initial actual node card window
    // without making the two-event interval artificially oversized.
    let limit=HistoryPageLimit::default_for(&HistoryCollection::Nodes).get();
    for i in 0..=limit {
        let current=save(&mut e,&repo,FINAL,3+i)?;
        if current.file_id!=first.file_id {return Err("the same FILE changed ownership".into());}
    }
    Ok(Evidence { slot, file:first.file_id, old:first.version_id, one:one.version_id,
        two:two.version_id, older:older.to_string(),newer:newer.to_string(),undo:undo.to_string() })
}
