use super::*;
use crate::files::{history::Reads, test_entries};
use json_codec::json;
use storage_contract::history_window::{
    HistoryContentDisposition, HistoryContentSource, HistoryContextSummary,
    HistoryCursorFormat, HistoryCursorPayload, HistoryDirection, HistoryNodeSummary,
    HistoryPageCursors, HistoryPageIdentity, HistoryTreeId,
};

const FILE: &str = "file_00000000000000000000000000000001";
const REPO: &str = "repo_00000000000000000000000000000001";
const TREE: &str = "history_00000000000000000000000000000001";
const NODE: &str = "ver_00000000000000000000000000000001";
const NEXT_NODE: &str = "ver_00000000000000000000000000000002";
const ROOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn selected(backup: bool) -> Entry {
    test_entries::entry(json!({"file_id":FILE,"entry_id":"entry","path":"a.txt",
        "repository_id":REPO,"repository_path":"a.txt","node_id":NODE,
        "version_id":NODE,"own_version_id":NODE,"backup":backup}))
}

fn empty_state(backup: bool) -> State {
    let mut state = State { authenticated:true, session_epoch:3, ..Default::default() };
    state.files.slot = if backup { "B01" } else { "P01" }.into();
    state.files.selected = Some(selected(backup));
    state.files.history_generation = 8;
    state.files.history_loading = true;
    state
}

fn response(
    target: &Target, request: &HistoryWindowRequest, current: &str, visible: &str,
) -> HistoryWindowPage {
    let context = HistoryResolvedContext {
        repository_id:HistoryRepositoryId::parse(REPO).unwrap(),
        history_tree_id:HistoryTreeId::parse(TREE).unwrap(),
        path:Some(name_policy::PortablePath::parse("a.txt").unwrap()),
        current_node_id:Some(HistoryNodeId::parse(current).unwrap()),
        selected_branch_id:None,
    };
    let anchor = request.anchor_node_id.as_ref()
        .or_else(||request.cursor.as_ref().and_then(|cursor|cursor.payload().anchor_node_id.as_ref()));
    let identity = HistoryPageIdentity {
        history_root:HistoryRoot::parse(ROOT).unwrap(), file_id:request.file_id.clone(),
        collection:request.collection.clone(), query_hash:request.query_hash(&context,anchor),
        context, limit:request.limit, action_max_entries:10_000,
    };
    let node: HistoryNodeSummary = json_codec::from_value(json!({
        "node_id":visible,"version_id":visible,"kind":"content","sequence":"1",
        "current":visible==current,"branch_id":null,"path":"a.txt","previous_path":null,
        "note":"visible node","tags":["inherited"],"size":"0","content_hash":"0".repeat(64),
        "content_state":"AVAILABLE","download_name":"a.txt","storage_name":"a.txt",
        "parents":{"state":"complete"},"fields":{"message":{"byte_length":"0"},
        "author":{"byte_length":"0"},"created_at":{"byte_length":"20"}}
    })).unwrap();
    let page = HistoryWindowPage::Nodes {
        identity, context:HistoryContextSummary {
            note:"visible file note".into(), content:Some(HistoryContentDisposition {
                version_id:Some(HistoryVersionId::parse(current).unwrap()),
                source:HistoryContentSource::Owned, stored_state:Some(storage_contract::ContentState::Available),
            }),
        }, nodes:vec![node], edges:vec![],
        page:HistoryPageCursors { previous_cursor:None,next_cursor:None },
    };
    history::admit(json_codec::to_value(page).unwrap(),request,
        request.history_root.and(target.resolved.as_ref())).unwrap()
}

fn loaded_state() -> State {
    let mut state = empty_state(false);
    let target = Target::capture(&state).unwrap();
    let request = make_request(&target,HistoryCollection::Nodes,None,None,false).unwrap();
    let page = response(&target,&request,NODE,NODE);
    let mut history = FileHistory::default();
    history.install_nodes(page,state.files.selected.as_ref().unwrap(),false).unwrap();
    state.files.file_history = history.into();
    state
}

fn error(status: u16) -> api::Error {
    api::Error { status, code:storage_contract::ErrorCode::TransportError,
        message:"read failed".into(), retryable:false, request_id:None, source:None }
}

#[test]
fn fixed_slots_replace_only_the_same_resource_and_late_completions_do_not_release_successors() {
    let mut reads = Reads::default();
    let branches = reads.begin(Resource::Branches,None);
    let old = reads.begin(Resource::Relation,None);
    let current = reads.begin(Resource::Relation,None);
    assert!(!reads.complete(Resource::Relation,old));
    assert!(reads.current(Resource::Relation,current));
    assert!(reads.current(Resource::Branches,branches));
    assert!(reads.complete(Resource::Relation,current));
    assert!(reads.loading());
    reads.cancel_all();
    assert!(!reads.loading());
    assert!(!reads.current(Resource::Branches,branches));
}

#[test]
fn stale_auxiliary_failure_cannot_invalidate_or_release_a_newer_read() {
    let mut state = loaded_state();
    let target = Target::capture(&state).unwrap();
    let old = state.files.history_aux.begin(Resource::Relation,None);
    let current = state.files.history_aux.begin(Resource::Relation,None);
    complete_auxiliary(&mut state,&target,Resource::Relation,old,Err(error(503))).unwrap();
    assert!(state.files.file_history.ready());
    assert!(state.files.history_aux.current(Resource::Relation,current));
    assert_eq!(state.files.file_history.file_note(),Some("visible file note"));
}

#[test]
fn current_read_failure_keeps_loaded_editor_text_but_closes_the_basis_and_all_auxiliary_work() {
    let mut state = loaded_state();
    let target = Target::capture(&state).unwrap();
    let current = state.files.history_aux.begin(Resource::Relation,None);
    state.files.history_aux.begin(Resource::Field,None);
    assert_eq!(complete_auxiliary(&mut state,&target,Resource::Relation,current,Err(error(503))).unwrap_err().status,503);
    assert!(state.files.file_history.loaded());
    assert!(!state.files.file_history.ready());
    assert_eq!(state.files.file_history.file_note(),Some("visible file note"));
    assert!(!state.files.history_aux.loading());
}

#[test]
fn main_completion_requires_session_generation_and_exact_selected_file() {
    for (epoch,generation,file) in [(2,8,FILE),(3,7,FILE),(3,8,"another-file")] {
        let mut state = loaded_state();
        complete_history_load(&mut state,epoch,generation,file,Err(error(503))).unwrap();
        assert!(state.files.file_history.ready());
        assert!(state.files.history_loading);
    }
    let mut state = loaded_state();
    assert!(complete_history_load(&mut state,3,8,FILE,Err(error(503))).is_err());
    assert!(state.files.file_history.loaded());
    assert!(!state.files.file_history.ready());
    assert!(!state.files.history_loading);
    assert_eq!(state.files.file_history.file_note(),Some("visible file note"));
}

#[test]
fn disk_directory_alias_and_stored_body_are_all_part_of_the_read_target() {
    let mut state = loaded_state();
    let target = Target::capture(&state).unwrap();
    assert!(target.current(&state));
    state.files.slot = "B02".into();
    assert!(!target.current(&state));
    state.files.slot = target.slot.clone();
    state.files.directory = "other".into();
    assert!(!target.current(&state));
    state.files.directory.clear();
    test_entries::update(state.files.selected.as_mut().unwrap(),json!({"repository_path":"other/a.txt"}));
    assert!(!target.current(&state));
    state.files.selected = Some(target.entry.clone());
    test_entries::update(state.files.selected.as_mut().unwrap(),json!({"node_id":NEXT_NODE,"version_id":NEXT_NODE}));
    assert!(!target.current(&state));
}

#[test]
fn an_active_existing_editor_preserves_its_window_and_requires_a_later_refresh() {
    let mut state = loaded_state();
    state.files.history_editors = 1;
    state.files.checked_nodes = crate::files::tags::tests::selection(&["kept"]);
    let target = Target::capture(&state).unwrap();
    let key = state.files.file_history.window_key();
    let request = make_request(&target,HistoryCollection::Nodes,None,None,false).unwrap();
    let page = response(&target,&request,NODE,NEXT_NODE);
    assert!(!complete_nodes(&mut state,&target,8,Ok(page),false).unwrap());
    assert_eq!(state.files.file_history.window_key(),key);
    assert!(state.files.file_history.node(NODE).is_some());
    assert!(!state.files.file_history.ready());
    assert!(!state.files.history_loading);
    assert!(state.files.checked_nodes.contains("kept"));
    assert!(!state.notice.is_empty());
}

#[test]
fn initial_loading_is_not_blocked_by_an_old_editor_still_cleaning_up() {
    let mut state = empty_state(false);
    state.files.history_editors = 1;
    let target = Target::capture(&state).unwrap();
    let request = make_request(&target,HistoryCollection::Nodes,None,None,false).unwrap();
    let page = response(&target,&request,NODE,NODE);
    assert!(complete_nodes(&mut state,&target,8,Ok(page),false).unwrap());
    assert!(state.files.file_history.ready());
    assert!(state.files.file_history.node(NODE).is_some());
}

#[test]
fn initial_backup_discovery_captures_a_root_before_jumping_to_the_stored_node() {
    let mut state = empty_state(true);
    let target = Target::capture(&state).unwrap();
    let fresh = make_request(&target,HistoryCollection::Nodes,None,None,false).unwrap();
    assert!(fresh.history_root.is_none() && fresh.anchor_node_id.is_none());
    assert!(make_request(&target,HistoryCollection::Nodes,None,Some(NODE),false).is_err());
    let first = response(&target,&fresh,NEXT_NODE,NEXT_NODE);
    complete_nodes(&mut state,&target,8,Ok(first),false).unwrap();
    assert_eq!(state.files.file_history.current_node_id(),Some(NODE));
    assert_eq!(backup_anchor(&state,&target.entry).as_deref(),Some(NODE));
    let rooted = Target::capture(&state).unwrap();
    let anchor = make_request(&rooted,HistoryCollection::Nodes,None,Some(NODE),true).unwrap();
    assert_eq!(anchor.history_root,Some(HistoryRoot::parse(ROOT).unwrap()));
    assert_eq!(anchor.anchor_node_id.as_ref().map(HistoryNodeId::as_str),Some(NODE));
    state.files.cancel_history();
    state.files.history_loading = true;
    let generation = state.files.history_generation;
    let page = response(&rooted,&anchor,NEXT_NODE,NODE);
    complete_nodes(&mut state,&rooted,generation,Ok(page),true).unwrap();
    assert!(backup_anchor(&state,&target.entry).is_none());
    assert_eq!(state.files.file_history.current_node_id(),Some(NODE));
    assert!(state.files.selected.as_ref().unwrap().backup());
}

fn cursor(target: &Target, collection: HistoryCollection, position: HistoryPosition, limit: usize) -> HistoryCursor {
    let mut request = make_request(target,collection,None,None,true).unwrap();
    request.limit = HistoryPageLimit::for_collection(limit,&request.collection).unwrap();
    let resolved = target.resolved.as_ref().unwrap();
    HistoryCursor::from_payload(HistoryCursorPayload {
        format:HistoryCursorFormat::HistoryWindowV1,
        history_root:HistoryRoot::parse(ROOT).unwrap(),history_tree_id:resolved.history_tree_id.clone(),
        file_id:request.file_id.clone(),collection:request.collection.clone(),
        query_hash:request.query_hash(resolved,None),limit:request.limit,
        direction:HistoryDirection::Forward,position,anchor_node_id:None,
    }).unwrap()
}

#[test]
fn continuation_uses_the_cursor_limit_and_rejects_cross_collection_or_mixed_anchor_queries() {
    let state = loaded_state();
    let target = Target::capture(&state).unwrap();
    let cursor = cursor(&target,HistoryCollection::Nodes,HistoryPosition::Node {
        sequence:1,node_id:HistoryNodeId::parse(NODE).unwrap(),
    },3);
    let request = make_request(&target,HistoryCollection::Nodes,Some(cursor.as_str()),None,true).unwrap();
    assert_eq!(request.limit.get(),3);
    assert!(endpoint(&request).contains("?limit=3&repository_id="));
    assert!(endpoint(&request).contains("&history_root="));
    assert!(endpoint(&request).contains("&cursor="));
    assert!(make_request(&target,HistoryCollection::Branches,Some(cursor.as_str()),None,true).is_err());
    assert!(make_request(&target,HistoryCollection::Nodes,Some(cursor.as_str()),Some(NODE),true).is_err());
}

#[test]
fn every_relation_uses_one_slot_and_version_sources_keep_the_advanced_route() {
    let state = loaded_state();
    let target = Target::capture(&state).unwrap();
    let node_id = HistoryNodeId::parse(NODE).unwrap();
    let merge_id = HistoryMergeId::parse("fmerge_00000000000000000000000000000001").unwrap();
    for collection in [HistoryCollection::NodeParents { node_id },
        HistoryCollection::MergeInputs { merge_id:merge_id.clone() },
        HistoryCollection::MergeParents { merge_id },
        HistoryCollection::VersionSources { version_id:HistoryVersionId::parse(NODE).unwrap() }]
    { assert_eq!(resource(&collection),Resource::Relation); }
    let request = make_request(&target,HistoryCollection::VersionSources {
        version_id:HistoryVersionId::parse(NODE).unwrap(),
    },None,None,true).unwrap();
    assert!(endpoint(&request).starts_with(&format!("/api/files/{FILE}/history/versions/{NODE}/sources?")));
}

#[test]
fn a_branch_review_cursor_cannot_cycle_backwards_or_repeat() {
    let state = loaded_state();
    let target = Target::capture(&state).unwrap();
    let branch = |suffix| storage_contract::history_window::HistoryBranchId::parse(
        &format!("br_{suffix:032x}")).unwrap();
    let first = cursor(&target,HistoryCollection::Branches,HistoryPosition::Branch { branch_id:branch(1) },3);
    let second = cursor(&target,HistoryCollection::Branches,HistoryPosition::Branch { branch_id:branch(2) },3);
    branch_progress(Some(&first),Some(&second)).unwrap();
    assert!(branch_progress(Some(&second),Some(&first)).is_err());
    assert!(branch_progress(Some(&first),Some(&first)).is_err());
}

#[test]
fn current_session_401_is_checked_before_any_obsolete_resource_fence() {
    Owner::new().with(|| {
        let state = RwSignal::new(loaded_state());
        let unauthorized: Result<HistoryWindowPage,api::Error> = Err(error(401));
        assert!(!accept_read_response(state,2,&unauthorized));
        assert!(state.with_untracked(State::session_active));
        assert!(!accept_read_response(state,3,&unauthorized));
        assert!(!state.with_untracked(State::session_active));
    });
}
