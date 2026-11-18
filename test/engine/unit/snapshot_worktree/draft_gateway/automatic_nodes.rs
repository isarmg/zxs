//! Real Born + no-past-node Unborn, independent preparation, emergency fence,
//! native receipt/cold reopen and stable ancestor/time-window observations.
use super::Fixture;
use crate::Engine;
use storage_contract::automatic_observation::{Position, Request, Scope};
use storage_contract::file_draft::{BirthRequest, CommitMode, DraftCommand, DraftLocation};
use storage_contract::snapshot_worktree::NamespaceDirectoryId;
use storage_contract::{OperationId, Principal, Timestamp};

fn mode(engine: &mut Engine, target: &DraftLocation, principal: &Principal, mode: CommitMode) {
    let record = &engine.state().file_containers[&target.file].working;
    let unborn = record.unborn().is_some();
    let command = DraftCommand::SetMode { target: target.clone(), expected: record.revision(), mode: mode.into() };
    let op = OperationId::generate().unwrap();
    if unborn { engine.change_unborn_working_mode(&command, principal, op).unwrap(); }
    else { engine.change_file_working(&command, principal, op).unwrap(); }
}

fn save(engine: &mut Engine, target: &DraftLocation, principal: &Principal, bytes: &[u8])
    -> crate::OperationResult {
    let mut handle = engine.open_native_working(target, principal, true).unwrap();
    assert_eq!(engine.write_native_working(&mut handle, principal, 0, bytes).unwrap(), bytes.len());
    engine.flush_native_working(&mut handle, principal).unwrap().unwrap()
}

fn scope(target: &DraftLocation, directory: NamespaceDirectoryId, start: u64, end: u64) -> Request {
    Request { scope: Scope { repository: target.repository, directory,
        start: Timestamp::from_unix_seconds(start), end: Timestamp::from_unix_seconds(end) }, position: Position::First }
}

#[test]
fn native_save_preparation_does_not_hold_manual_switch_and_a_stale_worker_has_no_node() {
    let (fixture, mut engine) = Fixture::new();
    let target = &fixture.targets[0];
    let actor = "admin".parse().unwrap();
    mode(&mut engine, target, &actor, CommitMode::Automatic);
    let saved = save(&mut engine, target, &actor, b"first replacement");
    let prepared = engine.prepare_automatic_file(target.file).unwrap().execute().unwrap();
    assert!(engine.prepare_automatic_file(target.file).is_err());
    let count = engine.state().versions.len();
    mode(&mut engine, target, &actor, CommitMode::Manual);
    let current = engine.file_working_state(target).unwrap();
    assert!(engine.finish_automatic_file(prepared).is_err());
    assert_eq!(engine.state().versions.len(), count);
    assert_eq!(engine.file_working_state(target).unwrap(), current);
    assert_eq!(engine.operation(saved.operation_id).unwrap(), saved);
    assert!(current.incremental().is_some());
    let query = scope(target, NamespaceDirectoryId::Repository(target.repository), 0, u64::MAX);
    assert!(engine.automatic_node_page(&query).unwrap().0.rows.is_empty());
}

#[test]
fn actual_automatic_nodes_bind_original_save_cause_stable_parent_and_half_open_time_after_cold_open() {
    let (fixture, mut engine) = Fixture::with_nested_directory(true);
    let actor: Principal = "admin".parse().unwrap();
    let repo = fixture.targets[0].repository;
    let branch = engine.state().repositories[&repo.to_string()].main_branch_id.clone();
    let parent = engine.list_tree(&branch).unwrap().into_iter()
        .find(|item| item.path.as_str() == "Folder/Child").unwrap();
    let main = &fixture.targets[0];
    let unborn = engine.create_unborn_file(&BirthRequest {
        repository: repo, path: name_policy::PortablePath::parse("Folder/Child/new.txt").unwrap(),
        expected_namespace_head: engine.state().branches[&branch].head_commit_id.parse().unwrap(),
    }, &actor, OperationId::generate().unwrap()).unwrap();
    let nested = crate::persist::file_draft::birth::decode(&unborn).unwrap().target().clone();
    let version_count = engine.state().versions.len();
    let mut receipts = Vec::new();
    for target in [main, &nested] {
        mode(&mut engine, target, &actor, CommitMode::Automatic);
        let saved = save(&mut engine, target, &actor, b"actual automatic publication");
        let version_before = engine.state().versions.len();
        let prepared = engine.prepare_automatic_file(target.file).unwrap().execute().unwrap();
        assert_eq!(engine.state().versions.len(), version_before);
        let node = engine.finish_automatic_file(prepared).unwrap();
        let working = engine.file_working_state(target).unwrap();
        let controller = working.automatic_controller().unwrap();
        let cause = controller.last_node().unwrap();
        assert_eq!(cause.save.operation, saved.operation_id);
        assert_eq!(cause.operation, node.operation_id);
        assert_eq!(cause.sequence.get(), node.sequence);
        assert!(controller.pending_save().is_none() && working.draft().is_none());
        assert!(engine.prepare_automatic_file(target.file).is_err());
        receipts.push(node);
    }
    assert_eq!(engine.state().versions.len(), version_count + 2);
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    let window = scope(main, NamespaceDirectoryId::Repository(repo), 0, u64::MAX);
    let (all, more) = engine.automatic_node_page(&window).unwrap();
    assert_eq!(all.rows.len(), 2); assert!(more.is_none() && all.next.is_none());
    let sub = scope(&nested, NamespaceDirectoryId::Entry(parent.entry_id.parse().unwrap()), 0, u64::MAX);
    let page = engine.automatic_node_page(&sub).unwrap().0;
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].key.file, nested.file);
    assert_eq!(page.rows[0].operation, receipts[1].operation_id);
    let t = page.rows[0].published_at.unix_seconds();
    assert!(engine.automatic_node_page(&scope(&nested, sub.scope.directory, 0, t)).unwrap().0.rows.is_empty());
    assert_eq!(engine.automatic_node_page(&scope(&nested, sub.scope.directory, t, t+1)).unwrap().0.rows, page.rows);
    for receipt in receipts { assert_eq!(engine.operation(receipt.operation_id).unwrap(), receipt); }
}

#[test]
fn emergency_before_copy_keeps_all_existing_material_and_the_saved_bytes_after_reopen() {
    let (fixture, mut engine) = Fixture::new();
    let target = &fixture.targets[0];
    let principal = "admin".parse().unwrap();
    mode(&mut engine, target, &principal, CommitMode::Automatic);
    let saved = save(&mut engine, target, &principal, b"newest durable replacement");
    let expected = super::read_draft(&engine, target);
    let job = engine.prepare_automatic_file(target.file).unwrap();
    mode(&mut engine, target, &principal, CommitMode::Manual);
    let before = fixture.media();
    let version_count = engine.state().versions.len();
    assert!(job.execute().is_err());
    assert_eq!(fixture.media(), before);
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.state().versions.len(), version_count);
    assert_eq!(engine.operation(saved.operation_id).unwrap(), saved);
    assert_eq!(super::read_draft(&engine, target), expected);
    let query = scope(target, NamespaceDirectoryId::Repository(target.repository), 0, u64::MAX);
    assert!(engine.automatic_node_page(&query).unwrap().0.rows.is_empty());
}

#[test]
fn emergency_after_real_unborn_full_keeps_zero_history_and_the_exact_saved_body() {
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    let repository = fixture.targets[0].repository;
    let branch = &engine.state().repositories[&repository.to_string()].main_branch_id;
    let head = engine.state().branches[branch].head_commit_id.parse().unwrap();
    let birth = engine.create_unborn_file(&BirthRequest {
        repository,
        path: name_policy::PortablePath::parse("first-only.txt").unwrap(),
        expected_namespace_head: head,
    }, &principal, OperationId::generate().unwrap()).unwrap();
    let target = crate::persist::file_draft::birth::decode(&birth).unwrap().target().clone();
    mode(&mut engine, &target, &principal, CommitMode::Automatic);
    let saved = save(&mut engine, &target, &principal, &vec![67; 150_000]);
    let prepared = engine.prepare_automatic_file(target.file).unwrap().execute().unwrap();
    let before = engine.state().versions.len();
    mode(&mut engine, &target, &principal, CommitMode::Manual);
    let current = engine.unborn_working_state(&target).unwrap();
    assert!(current.automatic_controller().is_none());
    let media = fixture.media();
    assert!(engine.finish_automatic_file(prepared).is_err());
    assert_eq!(fixture.media(), media);
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.state().versions.len(), before);
    assert_eq!(engine.unborn_working_state(&target).unwrap(), current);
    assert_eq!(engine.operation(saved.operation_id).unwrap(), saved);
    let handle = engine.open_native_working(&target, &principal, false).unwrap();
    let mut bytes = vec![0; 150_000];
    let count = engine.read_native_working(&handle, &mut bytes, 0).unwrap();
    assert_eq!(count, bytes.len());
    assert!(bytes.iter().all(|byte| *byte == 67));
}

#[test]
fn same_idle_writer_reopens_only_its_acknowledged_automatic_body_for_the_next_range() {
    let (fixture,mut engine)=Fixture::new();
    let target=&fixture.targets[0];
    let actor:Principal="admin".parse().unwrap();
    mode(&mut engine,target,&actor,CommitMode::Automatic);
    let mut h=engine.open_native_working(target,&actor,true).unwrap();
    engine.write_native_working(&mut h,&actor,0,b"acked-A").unwrap();
    let a=engine.flush_native_working(&mut h,&actor).unwrap().unwrap();
    let a_id=h.operation().unwrap();
    assert_eq!(a.operation_id,a_id);
    let task=engine.prepare_automatic_file(target.file).unwrap().execute().unwrap();
    let published=engine.finish_automatic_file(task).unwrap();
    let after_a=engine.file_draft_base(target).unwrap();
    assert_ne!(a_id,published.operation_id);
    let mut prefix=[0;7];
    assert_eq!(engine.read_native_working(&h,&mut prefix,0).unwrap(),prefix.len());
    assert_eq!(&prefix,b"acked-A");
    let intruder:Principal="different".parse().unwrap();
    assert!(engine.write_native_working(&mut h,&intruder,0,b"not allowed").is_err());
    assert_eq!(h.operation(),Some(a_id));
    engine.write_native_working(&mut h,&actor,6,b"B").unwrap();
    let b=engine.flush_native_working(&mut h,&actor).unwrap().unwrap();
    assert_ne!(a_id,b.operation_id);
    assert_eq!(engine.flush_native_working(&mut h,&actor).unwrap(),Some(b.clone()));
    assert_eq!(engine.operation(a_id).unwrap(),a);
    assert_eq!(engine.operation(published.operation_id).unwrap(),published);
    assert_eq!(engine.file_draft_base(target).unwrap(),after_a);
    let current=engine.file_working_state(target).unwrap();
    assert_eq!(current.draft().unwrap().0,&after_a);
    assert_eq!(current.automatic_controller().unwrap().pending_save().unwrap().operation,b.operation_id);
    assert_eq!(engine.read_native_working(&h,&mut prefix,0).unwrap(),prefix.len());
    assert_eq!(&prefix,b"acked-B");
    drop(h);drop(engine);
    let cold=Engine::open(&fixture.system).unwrap();
    assert_eq!(cold.operation(b.operation_id).unwrap(),b);
    assert_eq!(cold.operation(published.operation_id).unwrap(),published);
    assert_eq!(cold.file_working_state(target).unwrap(),current);
}

#[test]
fn unacknowledged_edit_is_not_reissued_or_rebased_if_automatic_publication_intervenes() {
    let (fixture,mut engine)=Fixture::new();
    let target=&fixture.targets[0];
    let actor:Principal="admin".parse().unwrap();
    mode(&mut engine,target,&actor,CommitMode::Automatic);
    let mut h=engine.open_native_working(target,&actor,true).unwrap();
    engine.write_native_working(&mut h,&actor,0,b"durable").unwrap();
    let saved=engine.flush_native_working(&mut h,&actor).unwrap().unwrap();
    let task=engine.prepare_automatic_file(target.file).unwrap().execute().unwrap();
    engine.write_native_working(&mut h,&actor,1,b"only staged").unwrap();
    let staged=h.operation().unwrap();
    let decided=engine.finish_automatic_file(task).unwrap();
    let unchanged=engine.state().clone();
    assert!(engine.flush_native_working(&mut h,&actor).is_err());
    assert!(!engine.state().operations.contains_key(&staged));
    assert_eq!(engine.state().sequence,unchanged.sequence);
    assert_eq!(engine.state().file_containers[&target.file],unchanged.file_containers[&target.file]);
    assert_eq!(engine.operation(saved.operation_id).unwrap(),saved);
    assert_eq!(engine.operation(decided.operation_id).unwrap(),decided);
}

#[test]
fn idle_automatic_writer_rejects_renamed_reselected_and_independently_copied_occurrences() {
    for change in ["rename","selection","copy"] {
        let (fixture,mut engine)=Fixture::new();
        let target=&fixture.targets[0];
        let actor:Principal="admin".parse().unwrap();
        let op=||OperationId::generate().unwrap();
        let repo=target.repository.to_string();
        let file=target.file.to_string();
        let initial=engine.file_draft_base(target).unwrap();
        // Allocate the destination/fork before A, not as an intervening
        // operation masquerading as the tested identity change.
        let fork=(change=="selection").then(||engine.create_file_branch(&file,&initial.version_id().to_string(),
            &storage_contract::BranchName::parse("another selected source").unwrap(),actor.as_str(),op())
            .unwrap().branch_id.unwrap());
        let destination=(change=="copy").then(||{
            let slot=engine.config().disks[1].slot_id.clone();
            engine.create_repository(&storage_contract::RepositoryName::parse("unrelated actual copy").unwrap(),
                &slot,1,actor.as_str(),op()).unwrap()
        });
        mode(&mut engine,target,&actor,CommitMode::Automatic);
        let mut held=engine.open_native_working(target,&actor,true).unwrap();
        engine.write_native_working(&mut held,&actor,0,b"A's real publication").unwrap();
        let a=engine.flush_native_working(&mut held,&actor).unwrap().unwrap();
        let job=engine.prepare_automatic_file(target.file).unwrap().execute().unwrap();
        let published=engine.finish_automatic_file(job).unwrap();
        match change {
            "rename" => {
                engine.file_namespace(&repo,crate::NamespaceOperation::parse(
                    crate::NamespaceOperationKind::Rename,target.path.as_str(),Some("renamed.txt")).unwrap(),
                    actor.as_str(),op()).unwrap();
                assert_eq!(engine.state().file_containers[&target.file].namespace.name.as_str(),"renamed.txt");
            }
            "selection" => {
                let branch=fork.unwrap();
                engine.select_file_branch_at(&repo,&target.path,&file,&branch,actor.as_str(),op()).unwrap();
                assert_eq!(engine.file_draft_base(target).unwrap().branch_id().to_string(),branch);
            }
            "copy" => {
                let destination=destination.unwrap();
                let (source_head,target_head)={
                    let state=engine.state();
                    (state.branches[&state.repositories[&repo].main_branch_id].head_commit_id.clone(),
                        state.branches[&destination.main_branch_id].head_commit_id.clone())
                };
                engine.copy_file_history(&crate::FileHistoryCopyRequest{
                    source_repository_id:repo,source_path:target.path.as_str().to_owned(),expected_source_head:source_head,
                    target_repository_id:destination.repository_id,target_name:"fork.txt".into(),
                    expected_target_head:target_head,
                },actor.as_str(),op()).unwrap();
                let copied=engine.list_tree(&destination.main_branch_id).unwrap();
                assert_eq!(copied.len(),1);
                assert_ne!(copied[0].file_id,file);
                assert_ne!(copied[0].entry_id,target.entry.to_string());
            }
            _ => unreachable!(),
        }
        let sequence=engine.state().sequence;
        assert!(engine.write_native_working(&mut held,&actor,1,b"must not edit another source").is_err());
        assert_eq!(held.operation(),Some(a.operation_id));
        assert_eq!(engine.state().sequence,sequence);
        assert_eq!(engine.operation(a.operation_id).unwrap(),a);
        assert_eq!(engine.operation(published.operation_id).unwrap(),published);
    }
}

#[test]
fn a_deleted_or_hidden_projection_cannot_issue_or_rebind_an_idle_writer() {
    let (fixture,mut engine)=Fixture::new();
    let target=&fixture.targets[0];
    let actor:Principal="admin".parse().unwrap();
    mode(&mut engine,target,&actor,CommitMode::Automatic);
    let mut h=engine.open_native_working(target,&actor,true).unwrap();
    engine.write_native_working(&mut h,&actor,0,b"a real saved range").unwrap();
    let saved=engine.flush_native_working(&mut h,&actor).unwrap().unwrap();
    let task=engine.prepare_automatic_file(target.file).unwrap().execute().unwrap();
    let published=engine.finish_automatic_file(task).unwrap();
    let file=storage_contract::history_window::HistoryFileId::parse(&target.file.to_string()).unwrap();
    let repository=storage_contract::history_window::HistoryRepositoryId::parse(&target.repository.to_string()).unwrap();
    let path=name_policy::LookupPath::parse(target.path.as_str()).unwrap();
    let loc=crate::FileDeletionTarget{file_id:&file,repository_id:&repository,path:&path};
    let deletion=OperationId::generate().unwrap();
    assert!(engine.soft_delete_file(crate::DeleteFileRequest{target:loc,principal:actor.as_str(),operation_id:deletion}).is_err());
    assert!(engine.state().file_deletions.get(file.as_str()).is_none());
    assert!(!engine.state().operations.contains_key(&deletion));
    // Rejection probe only, never a published FILE deletion. Use the effective
    // heads from the real read-only preflight; keep the installed decision,
    // placement, controller and operation count untouched to exercise rejection
    // of the wrong source, not the broader intervening-operation guard.
    let pre=engine.file_deletion_preflight(loc,&Default::default()).unwrap();
    let node=storage_contract::FileDeletionNode{
        node_id:crate::new_id("fdel").unwrap(),repository_id:target.repository.to_string(),
        history_tree_id:pre.repository().history_tree_id.clone(),file_id:file.to_string(),
        path:target.path.clone(),branch_heads:pre.branches().iter().map(|r|
            (r.branch().branch_id.clone(),r.node_id().to_owned())).collect(),
        parent_node_ids:pre.parent_node_ids().to_vec(),operation_id:deletion,author:actor.as_str().into(),
        sequence:engine.state().sequence,created_at:0,
    };
    let admitted=std::sync::Arc::clone(&engine.state);
    std::sync::Arc::make_mut(&mut engine.state).file_deletions.insert(file.to_string(),node);
    assert!(engine.write_native_working(&mut h,&actor,1,b"untrusted hidden view").is_err());
    engine.state=admitted;
    assert_eq!(h.operation(),Some(saved.operation_id));
    assert_eq!(engine.operation(published.operation_id).unwrap(),published);
}
