use super::*;
use crate::{
    Branch, BranchKind, BranchState, Commit, FileBranchMerge, FileMetadataKind, FileMetadataNode,
    FilePathNode, FileVersion, HistoryState, HistoryTree, OperationId, Repository,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use storage_contract::history_window::{
    self as history_contract, HistoryCollection, HistoryContentSource, HistoryCursor,
    HistoryDirection, HistoryEndpoint, HistoryField, HistoryPageCursors, HistoryPageIdentity,
    HistoryPageLimit, HistoryParentSet, HistoryPosition, HistoryResolvedContext, HistoryWindowPage,
    HistoryWindowRequest,
};

const FILE: &str = "file_00000000000000000000000000000001";
const HISTORY: &str = "history_00000000000000000000000000000001";
const FOREIGN_FILE: &str = "file_00000000000000000000000000000002";
const FOREIGN_HISTORY: &str = "history_00000000000000000000000000000002";
const ROOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn id(prefix: &str, n: usize) -> String {
    format!("{prefix}_{n:032x}")
}

fn operation() -> OperationId {
    "op_00000000000000000000000000000000".parse().unwrap()
}

fn body(n: usize, file: &str, history: &str) -> FileVersion {
    FileVersion {
        history_tree_id: history.into(),
        version_id: id("ver", n),
        file_id: file.into(),
        creation_primary_slot_id: "P01".into(),
        logical_size: n as u64,
        version_sequence: if n == 16 { u64::MAX } else { n as u64 + 1 },
        content_hash: "0".repeat(64).parse().unwrap(),
        checksum_chunk_bytes: 262144,
        checksum_manifest_hash: "0".repeat(64),
        creation_name: "原稿.txt".into(),
        storage_name: format!("v{n:06}_20261030T000000Z_00000000.txt"),
        created_at: "2026-10-30T00:00:00Z".into(),
        source_version_ids: (0..n).map(|index| id("ver", index)).collect(),
        created_as_full_checkpoint: true,
    }
}

// This is a query-only scope fixture, deliberately including foreign records.
// Actual publication and cold-recovery admission have separate real-I/O tests.
pub(super) fn fixture() -> MetadataView {
    let mut state = State::default();
    state.histories.insert(
        HISTORY.into(),
        HistoryTree {
            history_tree_id: HISTORY.into(),
            repository_id: id("repo", 0),
            root_commit_id: id("commit", 0),
            main_branch_id: Some(id("br", 1000)),
            generation: 1,
            state: HistoryState::Active,
            provenance: None,
        },
    );
    state.repositories.insert(
        id("repo", 0),
        Repository {
            repository_id: id("repo", 0),
            name: "query fixture".into(),
            history_tree_id: HISTORY.into(),
            main_branch_id: id("br", 1000),
            primary_slot_id: "P01".into(),
            target_copies: 1,
            name_policy: "portable".into(),
            unicode_version: "15.1".into(),
            smb_enabled: false,
            smb_binding_generation: 1,
            file_manager_root: true,
        },
    );
    for n in 0..17 {
        let version = body(n, FILE, HISTORY);
        let entry = crate::Entry {
            entry_id: id("entry", 0),
            path: name_policy::PortablePath::parse("目录/原稿.txt").unwrap(),
            kind: crate::NamespaceEntryKind::File,
            file_id: FILE.into(),
            version_id: version.version_id.clone(),
            size: version.logical_size,
        };
        state.commits.insert(
            id("commit", n),
            Commit {
                commit_id: id("commit", n),
                history_tree_id: HISTORY.into(),
                kind: "save".into(),
                parent_commit_ids: if n == 0 {
                    vec![]
                } else {
                    vec![id("commit", n - 1)]
                },
                tree_id: id("tree", n),
                sequence: if n == 16 { u64::MAX } else { n as u64 / 2 + 1 },
                operation_id: operation(),
                author: if n == 16 {
                    "作者🦀\"\\\n".repeat(1024)
                } else {
                    "author".into()
                },
                message: if n == 16 {
                    "消息🦀\"\\\n".repeat(8192)
                } else {
                    String::new()
                },
                entries: vec![entry],
                namespace_deletions: None,
                pending_file_versions: BTreeMap::new(),
                provenance: None,
            },
        );
        state.versions.insert(version.version_id.clone(), version);
    }
    let mut foreign_commit = state.commits[&id("commit", 0)].clone();
    foreign_commit.commit_id = id("commit", 100);
    foreign_commit.history_tree_id = FOREIGN_HISTORY.into();
    foreign_commit.sequence = 0;
    foreign_commit.entries[0].relocate(name_policy::PortablePath::parse("foreign.txt").unwrap());
    state
        .commits
        .insert(foreign_commit.commit_id.clone(), foreign_commit);
    state.file_metadata_nodes.insert(
        id("fmeta", 0),
        FileMetadataNode {
            node_id: id("fmeta", 0),
            history_tree_id: HISTORY.into(),
            file_id: FILE.into(),
            version_id: id("ver", 0),
            parent_node_ids: vec![id("ver", 0)],
            branch_id: id("br", 0),
            kind: FileMetadataKind::Tags,
            tags: (0..16)
                .map(|index| format!("{index:02}{}", "界".repeat(42)))
                .collect(),
            operation_id: operation(),
            author: "标签作者🦀".repeat(2048),
            sequence: 1,
            created_at: 1,
        },
    );
    state
        .file_node_parents
        .insert(id("ver", 1), vec![id("fmeta", 0)]);
    state.file_path_nodes.insert(
        id("fnode", 0),
        FilePathNode {
            node_id: id("fnode", 0),
            history_tree_id: HISTORY.into(),
            file_id: FILE.into(),
            version_id: id("ver", 7),
            parent_node_ids: vec![id("ver", 7)],
            branch_id: id("br", 0),
            path: "目录/终稿.txt".into(),
            previous_path: "目录/原稿.txt".into(),
            operation_id: operation(),
            author: "改名作者🦀".repeat(2048),
            sequence: 4,
            created_at: 2,
        },
    );
    state.file_node_parents.insert(
        id("ver", 16),
        (0..16)
            .map(|n| id("ver", n))
            .chain([id("fnode", 0)])
            .collect(),
    );
    state
        .version_notes
        .insert(id("ver", 0), format!("{}ab", "备注".repeat(21)));
    state
        .file_path_notes
        .insert(id("fnode", 0), "改名备注".repeat(10));
    state
        .content_states
        .insert(id("ver", 2), crate::ContentState::Purged);
    for n in 0..21 {
        state.branches.insert(
            id("br", n),
            Branch {
                branch_id: id("br", n),
                history_tree_id: HISTORY.into(),
                name: format!("branch {n}"),
                head_commit_id: id("commit", if n == 18 { 0 } else { 16 }),
                fork_commit_id: id("commit", 0),
                branch_kind: BranchKind::File,
                file_id: Some(FILE.into()),
                file_node_id: Some(id("ver", if n == 18 { 0 } else { 16 })),
                pending_file_versions: if n == 18 {
                    BTreeMap::from([(FILE.into(), id("ver", 0))])
                } else {
                    BTreeMap::new()
                },
                generation: 1,
                state: match n {
                    19 => BranchState::Merged,
                    20 => BranchState::Migrated,
                    _ => BranchState::Active,
                },
            },
        );
    }
    let mut directory_branch = state.branches[&id("br", 0)].clone();
    directory_branch.branch_id = id("br", 1000);
    directory_branch.branch_kind = BranchKind::Full;
    directory_branch.file_id = None;
    directory_branch.file_node_id = None;
    state
        .branches
        .insert(directory_branch.branch_id.clone(), directory_branch);
    let mut foreign_branch = state.branches[&id("br", 0)].clone();
    foreign_branch.branch_id = id("br", 100);
    foreign_branch.history_tree_id = FOREIGN_HISTORY.into();
    state
        .branches
        .insert(foreign_branch.branch_id.clone(), foreign_branch);
    state.file_branch_aliases.insert(id("br", 19), id("br", 0));
    state.file_selections.insert(FILE.into(), id("br", 0));
    for n in 0..3 {
        state.file_branch_merges.insert(
            id("fmerge", n),
            FileBranchMerge {
                merge_id: id("fmerge", n),
                history_tree_id: HISTORY.into(),
                file_id: FILE.into(),
                input_branch_ids: (0..19).map(|index| id("br", index)).collect(),
                input_node_ids: (0..19).map(|index| id("ver", index % 17)).collect(),
                chosen_node_id: id("ver", 7),
                result_node_id: id("ver", 7),
                surviving_branch_id: id("br", 0),
                reused_node: true,
                parent_node_ids: if n == 0 {
                    vec![id("ver", 1), id("fmeta", 0)]
                } else {
                    vec![]
                },
                operation_id: operation(),
                sequence: if n == 2 { u64::MAX } else { 11 },
            },
        );
    }
    let mut foreign_body = body(100, FOREIGN_FILE, HISTORY);
    foreign_body.source_version_ids.clear();
    state
        .versions
        .insert(foreign_body.version_id.clone(), foreign_body);
    let mut foreign_body = body(101, FILE, FOREIGN_HISTORY);
    foreign_body.source_version_ids.clear();
    state
        .versions
        .insert(foreign_body.version_id.clone(), foreign_body);
    let mut foreign_path = state.file_path_nodes[&id("fnode", 0)].clone();
    foreign_path.node_id = id("fnode", 100);
    foreign_path.history_tree_id = FOREIGN_HISTORY.into();
    state
        .file_path_nodes
        .insert(foreign_path.node_id.clone(), foreign_path);
    let mut foreign_metadata = state.file_metadata_nodes[&id("fmeta", 0)].clone();
    foreign_metadata.node_id = id("fmeta", 100);
    foreign_metadata.file_id = FOREIGN_FILE.into();
    state
        .file_metadata_nodes
        .insert(foreign_metadata.node_id.clone(), foreign_metadata);
    let mut foreign_merge = state.file_branch_merges[&id("fmerge", 0)].clone();
    foreign_merge.merge_id = id("fmerge", 100);
    foreign_merge.history_tree_id = FOREIGN_HISTORY.into();
    foreign_merge.parent_node_ids = vec![id("ver", 100), id("ver", 101)];
    state
        .file_branch_merges
        .insert(foreign_merge.merge_id.clone(), foreign_merge);
    let state = Arc::new(state);
    let history_window_roots = Some(HistoryRootBindings::recovered(
        &state,
        BTreeMap::from([(HISTORY.into(), ROOT.into())]),
    ));
    MetadataView {
        state,
        history_window_roots,
        history_action_max_entries: 123,
    }
}

pub(super) fn oracle_node_keys(state: &State) -> Vec<(u64, String)> {
    let mut keys = state
        .versions
        .values()
        .filter(|version| version.file_id == FILE && version.history_tree_id == HISTORY)
        .map(|version| {
            let sequence = state
                .commits
                .values()
                .filter(|commit| {
                    commit.history_tree_id == HISTORY
                        && commit.entries.iter().any(|entry| {
                            entry.file_id == FILE && entry.version_id == version.version_id
                        })
                })
                .map(|commit| (commit.sequence, &commit.commit_id))
                .min()
                .map_or(version.version_sequence, |(sequence, _)| sequence);
            (sequence, version.version_id.clone())
        })
        .chain(
            state
                .file_path_nodes
                .values()
                .filter(|node| node.file_id == FILE && node.history_tree_id == HISTORY)
                .map(|node| (node.sequence, node.node_id.clone())),
        )
        .chain(
            state
                .file_metadata_nodes
                .values()
                .filter(|node| node.file_id == FILE && node.history_tree_id == HISTORY)
                .map(|node| (node.sequence, node.node_id.clone())),
        )
        .collect::<Vec<_>>();
    keys.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    keys
}

fn oracle_parents(state: &State, node: &str) -> Vec<String> {
    let visible = oracle_node_keys(state)
        .into_iter()
        .map(|(_, id)| id)
        .collect::<BTreeSet<_>>();
    let base = if let Some(metadata) = state.file_metadata_nodes.get(node) {
        &metadata.parent_node_ids
    } else if let Some(path) = state.file_path_nodes.get(node) {
        &path.parent_node_ids
    } else {
        let version = &state.versions[node];
        state
            .file_node_parents
            .get(node)
            .unwrap_or(&version.source_version_ids)
    };
    let mut parents = base
        .iter()
        .filter(|id| visible.contains(id.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>();
    for merge in state.file_branch_merges.values().filter(|merge| {
        merge.file_id == FILE && merge.history_tree_id == HISTORY && merge.result_node_id == node
    }) {
        parents.extend(
            merge
                .parent_node_ids
                .iter()
                .filter(|id| id.as_str() != node && visible.contains(id.as_str()))
                .cloned(),
        );
    }
    parents.into_iter().collect()
}

fn oracle_tags(state: &State, node: &str) -> BTreeSet<String> {
    let mut current = node;
    let mut seen = BTreeSet::new();
    while seen.insert(current) {
        if let Some(metadata) = state.file_metadata_nodes.get(current) {
            return metadata.tags.clone();
        }
        let parents = if let Some(path) = state.file_path_nodes.get(current) {
            &path.parent_node_ids
        } else {
            let version = &state.versions[current];
            state
                .file_node_parents
                .get(current)
                .unwrap_or(&version.source_version_ids)
        };
        match parents.first() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    BTreeSet::new()
}

fn oracle_path<'a>(state: &'a State, node: &'a str) -> &'a str {
    let mut current = node;
    let mut seen = BTreeSet::new();
    while seen.insert(current) {
        if let Some(path) = state.file_path_nodes.get(current) {
            return &path.path;
        }
        if let Some(version) = state.versions.get(current) {
            return state
                .commits
                .values()
                .filter(|commit| commit.history_tree_id == HISTORY)
                .filter_map(|commit| {
                    commit
                        .entries
                        .iter()
                        .find(|entry| entry.file_id == FILE && entry.version_id == current)
                        .map(|entry| ((commit.sequence, &commit.commit_id), entry.path()))
                })
                .min_by_key(|(key, _)| *key)
                .map_or(&version.creation_name, |(_, path)| path);
        }
        let metadata = &state.file_metadata_nodes[current];
        let source = if metadata.kind == FileMetadataKind::Merge {
            state
                .file_branch_merges
                .values()
                .rfind(|merge| {
                    merge.file_id == FILE
                        && merge.history_tree_id == HISTORY
                        && !merge.reused_node
                        && merge.result_node_id == current
                })
                .map(|merge| &merge.chosen_node_id)
        } else {
            metadata.parent_node_ids.first()
        };
        match source {
            Some(source) => current = source,
            None => return &state.versions[&metadata.version_id].creation_name,
        }
    }
    panic!("oracle fixture source chains must be acyclic")
}

#[test]
fn borrowed_node_facts_match_independent_complete_state_oracle() {
    let view = fixture();
    let state = view.state();
    let scope = nodes::Scope {
        file: FILE,
        history: HISTORY,
    };
    let cancel = HistoryWindowCancellation::new();
    let mut actual = nodes::all_nodes(state, scope, &cancel)
        .map(|node| {
            let node = node.unwrap();
            let first = nodes::first_commit(state, scope, &node.version(state).version_id, &cancel)
                .unwrap();
            (node.sequence(first), node.id().to_owned())
        })
        .collect::<Vec<_>>();
    actual.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    assert_eq!(actual, oracle_node_keys(state));
    assert_eq!(actual.first().unwrap().0, u64::MAX);
    assert!(actual.windows(2).any(|pair| pair[0].0 == pair[1].0));
    for (_, id) in &actual {
        let selected = nodes::node(state, scope, id).unwrap();
        let mut parents = BTreeSet::new();
        nodes::visit_parents(state, scope, id, &cancel, |parent| {
            parents.insert(parent.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            parents.into_iter().collect::<Vec<_>>(),
            oracle_parents(state, id),
            "parents for {id}"
        );
        assert_eq!(
            nodes::tags(state, scope, id, &cancel)
                .unwrap()
                .cloned()
                .unwrap_or_default(),
            oracle_tags(state, id),
            "tags for {id}"
        );
        assert_eq!(
            nodes::path(state, scope, selected, &cancel).unwrap(),
            oracle_path(state, id),
            "path for {id}"
        );
    }
    for id in [
        id("ver", 100),
        id("ver", 101),
        id("fnode", 100),
        id("fmeta", 100),
    ] {
        assert!(
            nodes::node(state, scope, &id).is_none(),
            "foreign node {id}"
        );
    }
}

#[test]
fn merge_path_uses_the_chosen_source_and_keeps_the_referenced_real_body() {
    let mut view = fixture();
    let state = Arc::make_mut(&mut view.state);
    let mut metadata = state.file_metadata_nodes[&id("fmeta", 0)].clone();
    metadata.node_id = id("fmeta", 1);
    metadata.kind = FileMetadataKind::Merge;
    metadata.version_id = id("ver", 0);
    metadata.parent_node_ids = vec![id("fnode", 0), id("ver", 7), id("fmeta", 0)];
    let parents = metadata.parent_node_ids.clone();
    state
        .file_metadata_nodes
        .insert(metadata.node_id.clone(), metadata);
    let mut merge = state.file_branch_merges[&id("fmerge", 0)].clone();
    merge.merge_id = id("fmerge", 3);
    merge.result_node_id = id("fmeta", 1);
    merge.chosen_node_id = id("fmeta", 0);
    merge.parent_node_ids = parents;
    merge.reused_node = false;
    state
        .file_branch_merges
        .insert(merge.merge_id.clone(), merge);
    let scope = nodes::Scope {
        file: FILE,
        history: HISTORY,
    };
    let node = nodes::node(state, scope, &id("fmeta", 1)).unwrap();
    assert_eq!(node.version(state).version_id, id("ver", 0));
    assert_eq!(
        nodes::path(state, scope, node, &HistoryWindowCancellation::new()).unwrap(),
        "目录/原稿.txt"
    );
    assert_eq!(oracle_path(state, &id("fmeta", 1)), "目录/原稿.txt");
}

#[test]
fn cold_projection_helpers_observe_cancellation_before_visiting_results() {
    let view = fixture();
    let state = view.state();
    let scope = nodes::Scope {
        file: FILE,
        history: HISTORY,
    };
    let cancel = HistoryWindowCancellation::new();
    cancel.cancel();
    assert!(cancel.is_cancelled());
    assert_eq!(
        nodes::first_commit(state, scope, &id("ver", 16), &cancel)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::OperationAborted
    );
    assert_eq!(
        nodes::tags(state, scope, &id("ver", 16), &cancel)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::OperationAborted
    );
    let node = nodes::node(state, scope, &id("fnode", 0)).unwrap();
    assert_eq!(
        nodes::path(state, scope, node, &cancel).unwrap_err().code(),
        storage_contract::ErrorCode::OperationAborted
    );
    let mut visits = 0;
    let error = nodes::visit_parents(state, scope, &id("ver", 16), &cancel, |_| {
        visits += 1;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::OperationAborted);
    assert_eq!(visits, 0);
}

type NodeKey = (std::cmp::Reverse<u64>, String);

fn select_nodes(
    state: &State,
    position: selection::Position<NodeKey>,
    limit: usize,
) -> Result<selection::Selection<NodeKey, String>> {
    let scope = nodes::Scope {
        file: FILE,
        history: HISTORY,
    };
    let cancel = HistoryWindowCancellation::new();
    let mut selector = selection::Selector::new(position, limit)?;
    for node in nodes::all_nodes(state, scope, &cancel) {
        let node = node?;
        let commit = nodes::first_commit(state, scope, &node.version(state).version_id, &cancel)?;
        selector.offer(
            (
                std::cmp::Reverse(node.sequence(commit)),
                node.id().to_owned(),
            ),
            node.id().to_owned(),
        );
    }
    selector.finish()
}

#[test]
fn stable_node_windows_exhaust_the_complete_oracle_in_both_directions() {
    let view = fixture();
    let state = view.state();
    let expected = oracle_node_keys(state)
        .into_iter()
        .map(|(sequence, id)| (std::cmp::Reverse(sequence), id))
        .collect::<Vec<_>>();
    for limit in [1, 2, 3, 7, 100] {
        let mut page = select_nodes(state, selection::Position::Start, limit).unwrap();
        let retry = select_nodes(state, selection::Position::Start, limit).unwrap();
        assert_eq!(page.entries, retry.entries);
        assert_eq!(
            (page.has_before, page.has_after),
            (retry.has_before, retry.has_after)
        );
        assert!(!page.has_before);
        let mut forward = Vec::new();
        for _ in 0..=expected.len() {
            assert!(!page.entries.is_empty());
            assert!(page.entries.len() <= limit);
            forward.extend(page.entries.iter().map(|(key, _)| key.clone()));
            if !page.has_after {
                break;
            }
            let last = page.entries.last().unwrap().0.clone();
            let next =
                select_nodes(state, selection::Position::After(last.clone()), limit).unwrap();
            assert!(next.entries.first().is_some_and(|(key, _)| key > &last));
            assert!(next.has_before);
            page = next;
        }
        assert_eq!(forward, expected, "forward budget {limit}");
        assert!(!page.has_after);
        let mut backwards = Vec::new();
        for _ in 0..=expected.len() {
            assert!(page.entries.len() <= limit);
            backwards.push(
                page.entries
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>(),
            );
            if !page.has_before {
                break;
            }
            let first = page.entries.first().unwrap().0.clone();
            let previous =
                select_nodes(state, selection::Position::Before(first.clone()), limit).unwrap();
            assert!(previous.entries.last().is_some_and(|(key, _)| key < &first));
            assert!(previous.has_after);
            page = previous;
        }
        backwards.reverse();
        assert_eq!(
            backwards.into_iter().flatten().collect::<Vec<_>>(),
            expected,
            "reverse budget {limit}"
        );
        assert!(!page.has_before);
    }
}

#[test]
fn exact_anchor_tail_empty_and_invalid_boundaries_make_unambiguous_progress() {
    let view = fixture();
    let state = view.state();
    let expected = oracle_node_keys(state)
        .into_iter()
        .map(|(sequence, id)| (std::cmp::Reverse(sequence), id))
        .collect::<Vec<_>>();
    for (index, key) in expected.iter().enumerate() {
        let page = select_nodes(state, selection::Position::At(key.clone()), 2).unwrap();
        assert_eq!(
            page.entries
                .iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>(),
            expected[index..expected.len().min(index + 2)]
        );
        assert_eq!(page.has_before, index > 0);
        assert_eq!(page.has_after, index + 2 < expected.len());
    }
    let tail = select_nodes(
        state,
        selection::Position::After(expected.last().unwrap().clone()),
        2,
    )
    .unwrap();
    assert!(tail.entries.is_empty());
    assert!(!tail.has_after);
    let head = select_nodes(
        state,
        selection::Position::Before(expected.first().unwrap().clone()),
        2,
    )
    .unwrap();
    assert!(head.entries.is_empty());
    assert!(!head.has_before);
    let empty = select_nodes(&State::default(), selection::Position::Start, 2).unwrap();
    assert!(empty.entries.is_empty());
    assert!(!empty.has_before && !empty.has_after);
    for position in [
        selection::Position::After((std::cmp::Reverse(u64::MAX), id("ver", 100))),
        selection::Position::Before((std::cmp::Reverse(0), id("ver", 0))),
        selection::Position::At((std::cmp::Reverse(1), id("fnode", 100))),
    ] {
        assert_eq!(
            select_nodes(state, position, 2).err().unwrap().code(),
            storage_contract::ErrorCode::InvalidRequest
        );
    }
    assert_eq!(
        select_nodes(state, selection::Position::Start, 0)
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::InvalidRequest
    );
}

fn select_parents(
    state: &State,
    node: &str,
    position: selection::Position<String>,
    limit: usize,
) -> Result<selection::Selection<String, ()>> {
    let scope = nodes::Scope {
        file: FILE,
        history: HISTORY,
    };
    let mut selector = selection::Selector::new(position, limit)?;
    nodes::visit_parents(
        state,
        scope,
        node,
        &HistoryWindowCancellation::new(),
        |parent| {
            selector.offer(parent.to_owned(), ());
            Ok(())
        },
    )?;
    selector.finish()
}

#[test]
fn parent_windows_enumerate_high_indegree_and_reused_merge_parents_without_scope_leaks() {
    let view = fixture();
    let state = view.state();
    for node in [
        id("ver", 0),
        id("ver", 7),
        id("ver", 16),
        id("fnode", 0),
        id("fmeta", 0),
    ] {
        let expected = oracle_parents(state, &node);
        for limit in [1, 2, 3] {
            let mut page = select_parents(state, &node, selection::Position::Start, limit).unwrap();
            let mut actual = Vec::new();
            for _ in 0..=expected.len() {
                assert!(page.entries.len() <= limit);
                actual.extend(page.entries.iter().map(|(key, _)| key.clone()));
                if !page.has_after {
                    break;
                }
                let last = page
                    .entries
                    .last()
                    .expect("nonterminal parent page makes progress")
                    .0
                    .clone();
                page = select_parents(
                    state,
                    &node,
                    selection::Position::After(last.clone()),
                    limit,
                )
                .unwrap();
                assert!(page.entries.first().is_some_and(|(key, _)| key > &last));
            }
            assert_eq!(actual, expected, "node {node}, limit {limit}");
            assert!(!page.has_after);
        }
    }
    assert_eq!(oracle_parents(state, &id("ver", 16)).len(), 17);
    assert!(oracle_parents(state, &id("ver", 7)).contains(&id("fmeta", 0)));
    assert!(!oracle_parents(state, &id("ver", 7)).contains(&id("ver", 100)));
    assert!(!oracle_parents(state, &id("ver", 7)).contains(&id("ver", 101)));
}

#[test]
fn parent_scan_observes_cancellation_inside_one_large_relation() {
    let view = fixture();
    let scope = nodes::Scope {
        file: FILE,
        history: HISTORY,
    };
    let cancel = HistoryWindowCancellation::new();
    let owner = cancel.clone();
    let mut visits = 0;
    let error = nodes::visit_parents(view.state(), scope, &id("ver", 16), &cancel, |_| {
        visits += 1;
        if visits == 2 {
            owner.cancel();
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::OperationAborted);
    assert_eq!(visits, 2);
    assert!(cancel.is_cancelled());
    assert!(owner.is_cancelled());
    let request = request(HistoryCollection::Nodes, 1);
    for token in [&cancel, &owner] {
        assert_eq!(
            view.history_window(&request, &context(), token)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::OperationAborted
        );
    }
}

#[test]
fn node_scan_observes_cancellation_before_filtering_foreign_records() {
    let view = fixture();
    let scope = nodes::Scope {
        file: FOREIGN_FILE,
        history: FOREIGN_HISTORY,
    };
    let cancel = HistoryWindowCancellation::new();
    assert!(
        nodes::all_nodes(view.state(), scope, &cancel)
            .next()
            .is_none()
    );
    cancel.cancel();
    let error = nodes::all_nodes(view.state(), scope, &cancel)
        .next()
        .expect("a rejected candidate still observes cancellation")
        .err()
        .unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::OperationAborted);
}

pub(super) fn context() -> HistoryResolvedContext {
    HistoryResolvedContext {
        repository_id: id("repo", 0).parse().unwrap(),
        history_tree_id: HISTORY.parse().unwrap(),
        path: Some(name_policy::PortablePath::parse("目录/原稿.txt").unwrap()),
        current_node_id: Some(id("ver", 16).parse().unwrap()),
        selected_branch_id: Some(id("br", 0).parse().unwrap()),
    }
}

pub(super) fn request(collection: HistoryCollection, limit: usize) -> HistoryWindowRequest {
    let limit = HistoryPageLimit::for_collection(limit, &collection).unwrap();
    HistoryWindowRequest {
        file_id: FILE.parse().unwrap(),
        collection,
        location: None,
        history_root: Some(ROOT.parse().unwrap()),
        cursor: None,
        anchor_node_id: None,
        limit,
    }
}

fn public_page(view: &MetadataView, request: &HistoryWindowRequest) -> HistoryWindowPage {
    let context = context();
    let cancel = HistoryWindowCancellation::new();
    let page = view.history_window(request, &context, &cancel).unwrap();
    assert_eq!(
        page,
        view.history_window(request, &context, &cancel).unwrap()
    );
    assert!(
        json_codec::to_vec(&page).unwrap().len() <= history_contract::MAX_HISTORY_RESPONSE_BYTES
    );
    page
}

fn page_rows(page: HistoryWindowPage) -> (HistoryPageIdentity, HistoryPageCursors, Vec<String>) {
    match page {
        HistoryWindowPage::Nodes {
            identity,
            nodes,
            edges,
            page,
            ..
        } => {
            assert!(nodes.len() <= identity.limit.get());
            assert!(edges.len() <= history_contract::MAX_HISTORY_PREVIEW_EDGES);
            (
                identity,
                page,
                nodes
                    .into_iter()
                    .map(|node| node.node_id.to_string())
                    .collect(),
            )
        }
        HistoryWindowPage::Versions {
            identity,
            versions,
            page,
        } => {
            assert!(versions.len() <= identity.limit.get());
            (
                identity,
                page,
                versions
                    .into_iter()
                    .map(|version| version.version_id.to_string())
                    .collect(),
            )
        }
        HistoryWindowPage::Branches {
            identity,
            branches,
            page,
        } => {
            assert!(branches.len() <= identity.limit.get());
            (
                identity,
                page,
                branches
                    .into_iter()
                    .map(|branch| branch.branch_id.to_string())
                    .collect(),
            )
        }
        HistoryWindowPage::Merges {
            identity,
            merges,
            page,
        } => {
            assert!(merges.len() <= identity.limit.get());
            (
                identity,
                page,
                merges
                    .into_iter()
                    .map(|merge| merge.merge_id.to_string())
                    .collect(),
            )
        }
        HistoryWindowPage::NodeParents {
            identity,
            parents,
            page,
        } => {
            assert!(parents.len() <= identity.limit.get());
            (
                identity,
                page,
                parents
                    .into_iter()
                    .map(|parent| parent.parent_node_id.to_string())
                    .collect(),
            )
        }
        HistoryWindowPage::VersionSources {
            identity,
            sources,
            page,
        } => {
            assert!(sources.len() <= identity.limit.get());
            (
                identity,
                page,
                sources
                    .into_iter()
                    .map(|source| format!("{}:{}", source.ordinal, source.version_id))
                    .collect(),
            )
        }
        HistoryWindowPage::MergeInputs {
            identity,
            inputs,
            page,
        } => {
            assert!(inputs.len() <= identity.limit.get());
            (
                identity,
                page,
                inputs
                    .into_iter()
                    .map(|input| format!("{}:{}:{}", input.ordinal, input.branch_id, input.node_id))
                    .collect(),
            )
        }
        HistoryWindowPage::MergeParents {
            identity,
            parents,
            page,
        } => {
            assert!(parents.len() <= identity.limit.get());
            (
                identity,
                page,
                parents
                    .into_iter()
                    .map(|parent| format!("{}:{}", parent.ordinal, parent.parent_node_id))
                    .collect(),
            )
        }
        HistoryWindowPage::NodeField { .. } => panic!("record page requested"),
    }
}

fn assert_public_walk(view: &MetadataView, mut request: HistoryWindowRequest, expected: &[String]) {
    let mut forward = Vec::new();
    let mut seen_cursors = BTreeSet::new();
    for _ in 0..=expected.len() {
        let (identity, cursors, rows) = page_rows(public_page(view, &request));
        assert_eq!(identity.history_root.to_string(), ROOT);
        assert_eq!(identity.file_id.as_str(), FILE);
        assert_eq!(identity.context, context());
        assert_eq!(identity.collection, request.collection);
        assert_eq!(identity.limit, request.limit);
        assert_eq!(identity.action_max_entries, 123);
        assert!(rows.len() <= request.limit.get());
        forward.extend(rows.iter().cloned());
        let last_rows = rows;
        let last_previous = cursors.previous_cursor;
        match cursors.next_cursor {
            Some(next) => {
                assert!(!last_rows.is_empty(), "nonterminal page must make progress");
                assert!(seen_cursors.insert(next.as_str().to_owned()), "cursor loop");
                request.cursor = Some(next);
                request.anchor_node_id = None;
            }
            None => {
                assert_eq!(forward, expected, "forward {:?}", request.collection);
                let mut backward = vec![last_rows];
                let mut previous = last_previous;
                for _ in 0..=expected.len() {
                    let Some(cursor) = previous.take() else {
                        break;
                    };
                    request.cursor = Some(cursor);
                    request.anchor_node_id = None;
                    let (_, cursors, rows) = page_rows(public_page(view, &request));
                    assert!(!rows.is_empty());
                    backward.push(rows);
                    previous = cursors.previous_cursor;
                }
                assert!(previous.is_none(), "reverse cursor loop");
                backward.reverse();
                assert_eq!(
                    backward.into_iter().flatten().collect::<Vec<_>>(),
                    expected,
                    "backward {:?}",
                    request.collection
                );
                return;
            }
        }
    }
    panic!(
        "forward cursor did not terminate for {:?}",
        request.collection
    );
}

#[test]
fn content_commit_order_cannot_be_replaced_by_version_sequence_candidates() {
    let view = fixture();
    let query = request(HistoryCollection::Nodes, 2);
    let HistoryWindowPage::Nodes { nodes, .. } = public_page(&view, &query) else {
        panic!("node page requested");
    };
    let actual = nodes
        .iter()
        .map(|node| node.node_id.to_string())
        .collect::<Vec<_>>();
    let expected = oracle_node_keys(view.state())
        .into_iter()
        .take(2)
        .map(|(_, id)| id)
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert_eq!(actual, [id("ver", 16), id("ver", 14)]);
    let mut body_order = view
        .state()
        .versions
        .values()
        .filter(|version| version.file_id == FILE && version.history_tree_id == HISTORY)
        .map(|version| {
            (
                std::cmp::Reverse(version.version_sequence),
                version.version_id.clone(),
            )
        })
        .collect::<Vec<_>>();
    body_order.sort();
    let premature_candidates = body_order
        .into_iter()
        .take(2)
        .map(|(_, id)| id)
        .collect::<Vec<_>>();
    assert_eq!(premature_candidates, [id("ver", 16), id("ver", 15)]);
    assert_ne!(actual, premature_candidates);
}

#[test]
fn every_public_record_collection_matches_its_independent_complete_oracle() {
    let view = fixture();
    let state = view.state();
    let node_ids = oracle_node_keys(state)
        .into_iter()
        .map(|(_, id)| id)
        .collect::<Vec<_>>();
    let mut versions = state
        .versions
        .values()
        .filter(|version| version.file_id == FILE && version.history_tree_id == HISTORY)
        .map(|version| (version.version_sequence, version.version_id.clone()))
        .collect::<Vec<_>>();
    versions.sort();
    let versions = versions.into_iter().map(|(_, id)| id).collect::<Vec<_>>();
    let branches = state
        .branches
        .values()
        .filter(|branch| {
            branch.file_id.as_deref() == Some(FILE)
                && branch.history_tree_id == HISTORY
                && branch.state != BranchState::Migrated
        })
        .map(|branch| branch.branch_id.clone())
        .collect::<Vec<_>>();
    let mut merges = state
        .file_branch_merges
        .values()
        .filter(|merge| merge.file_id == FILE && merge.history_tree_id == HISTORY)
        .map(|merge| (std::cmp::Reverse(merge.sequence), merge.merge_id.clone()))
        .collect::<Vec<_>>();
    merges.sort();
    let merges = merges.into_iter().map(|(_, id)| id).collect::<Vec<_>>();
    for limit in [1, 2, 3, 7] {
        for (collection, expected) in [
            (HistoryCollection::Nodes, &node_ids),
            (HistoryCollection::Versions, &versions),
            (HistoryCollection::Branches, &branches),
            (HistoryCollection::Merges, &merges),
        ] {
            assert_public_walk(&view, request(collection, limit), expected);
        }
        for node in &node_ids {
            assert_public_walk(
                &view,
                request(
                    HistoryCollection::NodeParents {
                        node_id: node.parse().unwrap(),
                    },
                    limit,
                ),
                &oracle_parents(state, node),
            );
        }
        for version in &versions {
            let expected = state.versions[version]
                .source_version_ids
                .iter()
                .enumerate()
                .map(|(ordinal, source)| format!("{ordinal}:{source}"))
                .collect::<Vec<_>>();
            assert_public_walk(
                &view,
                request(
                    HistoryCollection::VersionSources {
                        version_id: version.parse().unwrap(),
                    },
                    limit,
                ),
                &expected,
            );
        }
        for merge in &merges {
            let merge = &state.file_branch_merges[merge];
            let expected = merge
                .input_branch_ids
                .iter()
                .zip(&merge.input_node_ids)
                .enumerate()
                .map(|(ordinal, (branch, node))| format!("{ordinal}:{branch}:{node}"))
                .collect::<Vec<_>>();
            assert_public_walk(
                &view,
                request(
                    HistoryCollection::MergeInputs {
                        merge_id: merge.merge_id.parse().unwrap(),
                    },
                    limit,
                ),
                &expected,
            );
            let expected = merge
                .parent_node_ids
                .iter()
                .enumerate()
                .map(|(ordinal, parent)| format!("{ordinal}:{parent}"))
                .collect::<Vec<_>>();
            assert_public_walk(
                &view,
                request(
                    HistoryCollection::MergeParents {
                        merge_id: merge.merge_id.parse().unwrap(),
                    },
                    limit,
                ),
                &expected,
            );
        }
    }
}

#[test]
fn public_node_anchors_preserve_current_identity_and_page_local_graph_meaning() {
    let view = fixture();
    let state = view.state();
    let expected = oracle_node_keys(state)
        .into_iter()
        .map(|(_, id)| id)
        .collect::<Vec<_>>();
    for anchor in &expected {
        let mut request = request(HistoryCollection::Nodes, 2);
        request.anchor_node_id = Some(anchor.parse().unwrap());
        let HistoryWindowPage::Nodes {
            identity,
            nodes,
            edges,
            ..
        } = public_page(&view, &request)
        else {
            panic!("node page requested");
        };
        assert_eq!(nodes[0].node_id.as_str(), anchor);
        assert_eq!(
            identity.context.current_node_id.as_ref().unwrap().as_str(),
            id("ver", 16)
        );
        let loaded = nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<BTreeSet<_>>();
        for edge in &edges {
            assert!(loaded.contains(edge.node_id.as_str()));
            assert!(
                oracle_parents(state, edge.node_id.as_str())
                    .iter()
                    .any(|parent| parent == edge.parent_node_id.as_str())
            );
            assert_eq!(
                edge.endpoint,
                if loaded.contains(edge.parent_node_id.as_str()) {
                    HistoryEndpoint::InWindow
                } else {
                    HistoryEndpoint::OutsideWindow
                }
            );
        }
        for node in &nodes {
            assert_eq!(node.current, node.node_id.as_str() == id("ver", 16));
            assert_eq!(
                node.tags.iter().cloned().collect::<BTreeSet<_>>(),
                oracle_tags(state, node.node_id.as_str())
            );
            assert_eq!(node.path, oracle_path(state, node.node_id.as_str()));
            let body = nodes::node(
                state,
                nodes::Scope {
                    file: FILE,
                    history: HISTORY,
                },
                node.node_id.as_str(),
            )
            .unwrap()
            .version(state);
            assert_eq!(node.version_id.as_str(), body.version_id);
            assert_eq!(
                node.content_state,
                state.content_states.get(&body.version_id).copied()
            );
            let observed = edges
                .iter()
                .filter(|edge| edge.node_id == node.node_id)
                .map(|edge| edge.parent_node_id.to_string())
                .collect::<Vec<_>>();
            match &node.parents {
                HistoryParentSet::Complete => {
                    assert_eq!(observed, oracle_parents(state, node.node_id.as_str()))
                }
                HistoryParentSet::More { cursor } => {
                    assert!(!observed.is_empty());
                    assert_eq!(cursor.payload().history_root.to_string(), ROOT);
                    assert_eq!(
                        cursor.payload().collection,
                        HistoryCollection::NodeParents {
                            node_id: node.node_id.clone()
                        }
                    );
                }
            }
        }
    }
}

fn high_indegree_fixture() -> MetadataView {
    let mut view = fixture();
    let state = Arc::make_mut(&mut view.state);
    let mut parents = state.file_node_parents[&id("ver", 16)].clone();
    for index in 200..200 + history_contract::MAX_HISTORY_PREVIEW_EDGES + 11 {
        let mut version = body(index, FILE, HISTORY);
        version.source_version_ids.clear();
        parents.push(version.version_id.clone());
        state.versions.insert(version.version_id.clone(), version);
    }
    parents.extend([id("ver", 100), id("ver", 101)]);
    state.file_node_parents.insert(id("ver", 16), parents);
    view.history_window_roots = Some(HistoryRootBindings::recovered(
        &view.state,
        BTreeMap::from([(HISTORY.into(), ROOT.into())]),
    ));
    view
}

#[test]
fn node_preview_budget_keeps_every_parent_reachable_with_explicit_unloaded_endpoints() {
    let view = high_indegree_fixture();
    let expected = oracle_parents(view.state(), &id("ver", 16));
    assert!(expected.len() > history_contract::MAX_HISTORY_PREVIEW_EDGES);
    let HistoryWindowPage::Nodes { nodes, edges, .. } =
        public_page(&view, &request(HistoryCollection::Nodes, 1))
    else {
        panic!("node page requested");
    };
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].node_id.as_str(), id("ver", 16));
    assert!(!edges.is_empty());
    assert!(edges.len() <= history_contract::MAX_HISTORY_PREVIEW_EDGES);
    assert!(
        edges
            .iter()
            .all(|edge| edge.endpoint == HistoryEndpoint::OutsideWindow)
    );
    let mut actual = edges
        .into_iter()
        .map(|edge| edge.parent_node_id.to_string())
        .collect::<Vec<_>>();
    let HistoryParentSet::More { cursor } = &nodes[0].parents else {
        panic!("a parent collection larger than the edge budget must remain open");
    };
    let mut follow = request(
        HistoryCollection::NodeParents {
            node_id: id("ver", 16).parse().unwrap(),
        },
        cursor.payload().limit.get(),
    );
    follow.cursor = Some(cursor.clone());
    let mut seen = BTreeSet::new();
    for _ in 0..=expected.len() {
        let (_, cursors, rows) = page_rows(public_page(&view, &follow));
        actual.extend(rows);
        match cursors.next_cursor {
            Some(next) => {
                assert!(seen.insert(next.as_str().to_owned()));
                follow.cursor = Some(next);
            }
            None => break,
        }
    }
    assert_eq!(actual, expected);
    let HistoryWindowPage::Nodes { nodes, edges, .. } =
        public_page(&view, &request(HistoryCollection::Nodes, 64))
    else {
        panic!("node page requested");
    };
    assert!(edges.len() <= history_contract::MAX_HISTORY_PREVIEW_EDGES);
    for node in nodes {
        if matches!(node.parents, HistoryParentSet::More { .. }) {
            assert!(
                edges.iter().any(|edge| edge.node_id == node.node_id),
                "nonempty parent sets retain a first edge and a usable continuation"
            );
        }
    }
}

#[test]
fn branch_summaries_keep_pending_real_body_aliases_and_sparse_content_facts() {
    let view = fixture();
    let mut request = request(HistoryCollection::Branches, 3);
    let mut observed = BTreeMap::new();
    for _ in 0..22 {
        let HistoryWindowPage::Branches { branches, page, .. } = public_page(&view, &request)
        else {
            panic!("branch page requested");
        };
        for branch in branches {
            observed.insert(branch.branch_id.to_string(), branch);
        }
        match page.next_cursor {
            Some(next) => request.cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(observed.len(), 20);
    assert!(!observed.contains_key(&id("br", 20)));
    assert!(!observed.contains_key(&id("br", 100)));
    let pending = &observed[&id("br", 18)];
    assert_eq!(pending.node_id.as_ref().unwrap().as_str(), id("ver", 0));
    assert_eq!(
        pending.content.version_id.as_ref().unwrap().as_str(),
        id("ver", 0)
    );
    let HistoryContentSource::Inherited { source_version_id } = &pending.content.source else {
        panic!("pending branch reads its inherited real body");
    };
    assert_eq!(source_version_id.as_ref().unwrap().as_str(), id("ver", 0));
    assert_eq!(pending.content.stored_state, None);
    let alias = &observed[&id("br", 19)];
    assert!(alias.read_only);
    assert_eq!(alias.alias_of.as_ref().unwrap().as_str(), id("br", 0));
    assert!(observed[&id("br", 0)].current);
}

#[test]
fn pending_empty_sources_remain_node_less_inherited_branches() {
    let mut view = fixture();
    let state = Arc::make_mut(&mut view.state);
    let mut commit = state.commits[&id("commit", 0)].clone();
    commit.commit_id = id("commit", 2000);
    commit.entries.clear();
    commit.parent_commit_ids = vec![id("commit", 0)];
    commit.pending_file_versions = BTreeMap::from([(FILE.into(), String::new())]);
    state.commits.insert(commit.commit_id.clone(), commit);
    let mut branch = state.branches[&id("br", 18)].clone();
    branch.branch_id = id("br", 21);
    branch.head_commit_id = id("commit", 2000);
    branch.file_node_id = None;
    branch.pending_file_versions = BTreeMap::from([(FILE.into(), String::new())]);
    state.branches.insert(branch.branch_id.clone(), branch);
    view.history_window_roots = Some(HistoryRootBindings::recovered(
        &view.state,
        BTreeMap::from([(HISTORY.into(), ROOT.into())]),
    ));
    let HistoryWindowPage::Branches { branches, .. } =
        public_page(&view, &request(HistoryCollection::Branches, 128))
    else {
        panic!("branch page requested");
    };
    let branch = branches
        .iter()
        .find(|branch| branch.branch_id.as_str() == id("br", 21))
        .unwrap();
    assert!(branch.node_id.is_none());
    assert!(branch.content.version_id.is_none());
    assert_eq!(
        branch.content.source,
        HistoryContentSource::Inherited {
            source_version_id: None
        }
    );
    assert!(branch.content.stored_state.is_none());
    assert!(!branch.read_only);
}

#[test]
fn explicit_parent_start_cursor_preserves_the_first_member_and_ordinary_navigation() {
    let view = fixture();
    let mut request = request(
        HistoryCollection::NodeParents {
            node_id: id("ver", 16).parse().unwrap(),
        },
        2,
    );
    let first = public_page(&view, &request);
    let HistoryWindowPage::NodeParents { page, .. } = &first else {
        panic!("parent page requested");
    };
    let mut payload = page.next_cursor.as_ref().unwrap().payload().clone();
    payload.position = HistoryPosition::Start;
    payload.direction = HistoryDirection::Forward;
    request.cursor = Some(HistoryCursor::from_payload(payload).unwrap());
    assert_eq!(public_page(&view, &request), first);
    assert_public_walk(
        &view,
        request,
        &oracle_parents(view.state(), &id("ver", 16)),
    );
}

#[test]
fn summaries_keep_original_source_counts_merge_provenance_and_external_field_lengths() {
    let view = fixture();
    let state = view.state();
    let HistoryWindowPage::Versions { versions, .. } =
        public_page(&view, &request(HistoryCollection::Versions, 128))
    else {
        panic!("version page requested");
    };
    assert_eq!(versions.len(), 17);
    for version in versions {
        let body = &state.versions[version.version_id.as_str()];
        let commit = state
            .commits
            .values()
            .filter(|commit| {
                commit.history_tree_id == HISTORY
                    && commit
                        .entries
                        .iter()
                        .any(|entry| entry.file_id == FILE && entry.version_id == body.version_id)
            })
            .min_by_key(|commit| (commit.sequence, &commit.commit_id))
            .unwrap();
        assert_eq!(version.version_sequence, body.version_sequence);
        assert_eq!(version.source_count, body.source_version_ids.len() as u64);
        assert_eq!(version.size, body.logical_size);
        assert_eq!(
            version.content_state,
            state.content_states.get(&body.version_id).copied()
        );
        assert_eq!(
            version.fields.message.byte_length,
            commit.message.len() as u64
        );
        assert_eq!(
            version.fields.author.byte_length,
            commit.author.len() as u64
        );
        assert_eq!(
            version.fields.created_at.byte_length,
            body.created_at.len() as u64
        );
        assert_eq!(version.storage_name, body.storage_name);
        assert_eq!(
            version.note,
            state
                .version_notes
                .get(&body.version_id)
                .map_or("", String::as_str)
        );
        if body.version_id == id("ver", 0) {
            assert_eq!(version.note.len(), 128);
        }
        if body.version_id == id("ver", 16) {
            assert_eq!(version.source_count, 16);
        }
    }
    assert_eq!(oracle_parents(state, &id("ver", 16)).len(), 17);
    let HistoryWindowPage::Merges { merges, .. } =
        public_page(&view, &request(HistoryCollection::Merges, 128))
    else {
        panic!("merge page requested");
    };
    assert_eq!(merges.len(), 3);
    for summary in merges {
        let saved = &state.file_branch_merges[summary.merge_id.as_str()];
        assert_eq!(summary.sequence, saved.sequence);
        assert_eq!(summary.chosen_node_id.as_str(), saved.chosen_node_id);
        assert_eq!(summary.result_node_id.as_str(), saved.result_node_id);
        assert_eq!(
            summary.surviving_branch_id.as_str(),
            saved.surviving_branch_id
        );
        assert_eq!(summary.reused_node, saved.reused_node);
        assert_eq!(summary.operation_id, saved.operation_id);
        assert_eq!(summary.input_count, saved.input_node_ids.len() as u64);
        assert_eq!(summary.parent_count, saved.parent_node_ids.len() as u64);
    }
}

fn assert_field_fragments(
    view: &MetadataView,
    node: &str,
    field: HistoryField,
    expected: &str,
    limit: usize,
) {
    let mut request = request(
        HistoryCollection::NodeField {
            node_id: node.parse().unwrap(),
            field,
        },
        limit,
    );
    let mut offset = 0;
    let mut complete = String::new();
    let mut cursors = BTreeSet::new();
    for _ in 0..=expected.len() {
        let HistoryWindowPage::NodeField {
            identity,
            field: actual_field,
            total_bytes,
            byte_offset,
            text,
            end_of_field,
            next_cursor,
        } = public_page(view, &request)
        else {
            panic!("field fragment requested");
        };
        assert_eq!(identity.history_root.to_string(), ROOT);
        assert_eq!(actual_field, field);
        assert_eq!(total_bytes, expected.len() as u64);
        assert_eq!(byte_offset, offset as u64);
        assert!(text.len() <= limit);
        assert!(expected.is_char_boundary(offset));
        let end = offset + text.len();
        assert!(expected.is_char_boundary(end));
        assert_eq!(text, expected[offset..end]);
        complete.push_str(&text);
        offset = end;
        assert_eq!(end_of_field, next_cursor.is_none());
        match next_cursor {
            Some(next) => {
                assert!(
                    !text.is_empty(),
                    "an unfinished fragment makes UTF-8 progress"
                );
                assert!(cursors.insert(next.as_str().to_owned()));
                request.cursor = Some(next);
            }
            None => {
                assert_eq!(offset, expected.len());
                assert_eq!(complete, expected);
                return;
            }
        }
    }
    panic!("field cursor did not terminate");
}

#[test]
fn every_unbounded_field_is_completely_reachable_in_utf8_fragments_without_truncation() {
    let view = fixture();
    let state = view.state();
    let cases = [
        (
            id("ver", 16),
            HistoryField::Message,
            state.commits[&id("commit", 16)].message.as_str(),
        ),
        (
            id("ver", 16),
            HistoryField::Author,
            state.commits[&id("commit", 16)].author.as_str(),
        ),
        (
            id("ver", 16),
            HistoryField::CreatedAt,
            state.versions[&id("ver", 16)].created_at.as_str(),
        ),
        (
            id("fnode", 0),
            HistoryField::Author,
            state.file_path_nodes[&id("fnode", 0)].author.as_str(),
        ),
        (
            id("fmeta", 0),
            HistoryField::Author,
            state.file_metadata_nodes[&id("fmeta", 0)].author.as_str(),
        ),
        (
            id("fnode", 0),
            HistoryField::CreatedAt,
            "1970-01-01T00:00:02Z",
        ),
        (
            id("fmeta", 0),
            HistoryField::CreatedAt,
            "1970-01-01T00:00:01Z",
        ),
        (id("ver", 0), HistoryField::Message, ""),
        (id("fnode", 0), HistoryField::Message, ""),
        (id("fmeta", 0), HistoryField::Message, ""),
    ];
    for (node, field, expected) in cases {
        for limit in [257, 4096] {
            assert_field_fragments(&view, &node, field, expected, limit);
        }
    }
    assert_field_fragments(
        &view,
        &id("ver", 0),
        HistoryField::CreatedAt,
        "2026-10-30T00:00:00Z",
        7,
    );
}

#[test]
fn cursors_cannot_change_scope_budget_or_query_or_invent_a_boundary() {
    let view = fixture();
    let original = request(HistoryCollection::Nodes, 2);
    let HistoryWindowPage::Nodes { page, .. } = public_page(&view, &original) else {
        panic!("node page requested");
    };
    let cursor = page.next_cursor.unwrap();
    let cancel = HistoryWindowCancellation::new();
    for mutation in 0..5 {
        let mut payload = cursor.payload().clone();
        match mutation {
            0 => payload.file_id = FOREIGN_FILE.parse().unwrap(),
            1 => payload.history_tree_id = FOREIGN_HISTORY.parse().unwrap(),
            2 => payload.query_hash = "b".repeat(64).parse().unwrap(),
            3 => {
                payload.position = HistoryPosition::Node {
                    sequence: u64::MAX,
                    node_id: id("ver", 100).parse().unwrap(),
                }
            }
            4 => {
                payload.direction = HistoryDirection::Backward;
                payload.position = HistoryPosition::Node {
                    sequence: 0,
                    node_id: id("fnode", 100).parse().unwrap(),
                };
            }
            _ => unreachable!(),
        }
        let mut changed = original.clone();
        changed.cursor = Some(HistoryCursor::from_payload(payload).unwrap());
        assert_eq!(
            view.history_window(&changed, &context(), &cancel)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::InvalidRequest
        );
    }
    let mut changed = original.clone();
    changed.cursor = Some(cursor.clone());
    changed.limit = HistoryPageLimit::new(3).unwrap();
    assert_eq!(
        view.history_window(&changed, &context(), &cancel)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::InvalidRequest
    );
    changed = request(HistoryCollection::Versions, 2);
    changed.cursor = Some(cursor.clone());
    assert_eq!(
        view.history_window(&changed, &context(), &cancel)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::InvalidRequest
    );
    changed = original.clone();
    changed.cursor = Some(cursor);
    changed.anchor_node_id = Some(id("ver", 0).parse().unwrap());
    assert_eq!(
        view.history_window(&changed, &context(), &cancel)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::InvalidRequest
    );
    changed = original;
    changed.history_root = Some("b".repeat(64).parse().unwrap());
    assert_eq!(
        view.history_window(&changed, &context(), &cancel)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::HistoryChanged
    );
}

#[test]
fn invalid_utf8_field_offsets_and_tiny_unusable_budgets_are_explicit_errors() {
    let view = fixture();
    let mut request = request(
        HistoryCollection::NodeField {
            node_id: id("ver", 16).parse().unwrap(),
            field: HistoryField::Message,
        },
        7,
    );
    let HistoryWindowPage::NodeField { next_cursor, .. } = public_page(&view, &request) else {
        panic!("field fragment requested");
    };
    let cursor = next_cursor.unwrap();
    for offset in [1, u64::MAX] {
        let mut payload = cursor.payload().clone();
        payload.position = HistoryPosition::Field {
            byte_offset: offset,
        };
        request.cursor = Some(HistoryCursor::from_payload(payload).unwrap());
        assert_eq!(
            view.history_window(&request, &context(), &HistoryWindowCancellation::new())
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::InvalidRequest
        );
    }
    request.cursor = None;
    request.limit = HistoryPageLimit::new(1).unwrap();
    assert_eq!(
        view.history_window(&request, &context(), &HistoryWindowCancellation::new())
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::InvalidRequest
    );
}

#[test]
fn foreign_auxiliary_subjects_and_nonexistent_member_ordinals_never_return_successful_empty_pages()
{
    let view = fixture();
    for collection in [
        HistoryCollection::NodeParents {
            node_id: id("ver", 100).parse().unwrap(),
        },
        HistoryCollection::NodeParents {
            node_id: id("fnode", 100).parse().unwrap(),
        },
        HistoryCollection::VersionSources {
            version_id: id("ver", 100).parse().unwrap(),
        },
        HistoryCollection::VersionSources {
            version_id: id("ver", 101).parse().unwrap(),
        },
        HistoryCollection::MergeInputs {
            merge_id: id("fmerge", 100).parse().unwrap(),
        },
        HistoryCollection::MergeParents {
            merge_id: id("fmerge", 100).parse().unwrap(),
        },
        HistoryCollection::NodeField {
            node_id: id("fmeta", 100).parse().unwrap(),
            field: HistoryField::Author,
        },
    ] {
        assert!(
            view.history_window(
                &request(collection, 2),
                &context(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
        );
    }
    let mut request = request(
        HistoryCollection::MergeInputs {
            merge_id: id("fmerge", 0).parse().unwrap(),
        },
        2,
    );
    let HistoryWindowPage::MergeInputs { page, .. } = public_page(&view, &request) else {
        panic!("input page requested");
    };
    let mut payload = page.next_cursor.unwrap().payload().clone();
    payload.position = HistoryPosition::Member { ordinal: u64::MAX };
    request.cursor = Some(HistoryCursor::from_payload(payload).unwrap());
    assert_eq!(
        view.history_window(&request, &context(), &HistoryWindowCancellation::new())
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::InvalidRequest
    );
}

#[test]
fn every_public_collection_checks_cancellation_including_legitimately_empty_results() {
    let view = fixture();
    let cancel = HistoryWindowCancellation::new();
    cancel.cancel();
    for collection in [
        HistoryCollection::Nodes,
        HistoryCollection::Versions,
        HistoryCollection::Branches,
        HistoryCollection::Merges,
        HistoryCollection::NodeParents {
            node_id: id("ver", 0).parse().unwrap(),
        },
        HistoryCollection::VersionSources {
            version_id: id("ver", 0).parse().unwrap(),
        },
        HistoryCollection::MergeInputs {
            merge_id: id("fmerge", 0).parse().unwrap(),
        },
        HistoryCollection::MergeParents {
            merge_id: id("fmerge", 1).parse().unwrap(),
        },
        HistoryCollection::NodeField {
            node_id: id("ver", 0).parse().unwrap(),
            field: HistoryField::Message,
        },
    ] {
        assert_eq!(
            view.history_window(&request(collection, 2), &context(), &cancel)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::OperationAborted
        );
    }
}
