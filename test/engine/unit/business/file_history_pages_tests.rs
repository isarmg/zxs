//! Exhaustive paging over the bounded file-history scope equals independent
//! whole-history oracles computed directly from State.
use super::*;
use crate::{CommitRequest, Config, FileMergeInput, FileNodeTagsUpdateRequest, FileTagTarget};

struct Pool {
    _temporary: tempfile::TempDir,
    root: std::path::PathBuf,
    engine: Engine,
    repo: Repository,
}

impl Pool {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let config = Config::development([
            temporary.path().join("primary"),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ]);
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Pages").unwrap(),
                "P01",
                3,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        Self {
            _temporary: temporary,
            root,
            engine,
            repo,
        }
    }
    fn save_in(&mut self, branch: &str, path: &str, bytes: &[u8], message: &str) -> Entry {
        let workspace = self.engine.create_workspace(branch, "admin").unwrap();
        self.engine
            .stage_bytes(&workspace.workspace_id, path, bytes)
            .unwrap();
        self.engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: message.into(),
                request_digest: None,
            })
            .unwrap();
        self.engine
            .list_tree(branch)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path == path)
            .unwrap()
    }
    fn save(&mut self, path: &str, bytes: &[u8], message: &str) -> Entry {
        let branch = self.repo.main_branch_id.clone();
        self.save_in(&branch, path, bytes, message)
    }
    fn fork(&mut self, file: &str, node: &str, name: &str) -> String {
        let version = self.engine.file_node_version(file, node).unwrap();
        self.engine
            .create_file_branch_from_node(
                file,
                &version,
                node,
                &storage_contract::BranchName::parse(name).unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap()
            .result["branch_id"]
            .as_str()
            .unwrap()
            .into()
    }
    fn choose(&mut self, file: &str, branch: &str, path: &str) {
        self.engine
            .select_file_branch_at(
                &self.repo.repository_id,
                &name_policy::PortablePath::parse(path).unwrap(),
                file,
                branch,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
    fn tag(&mut self, file: &str, branch: &str, node: &str, tags: &[&str]) -> String {
        self.engine
            .update_file_node_tags(FileNodeTagsUpdateRequest {
                file_id: file,
                targets: &[FileTagTarget {
                    branch_id: branch.into(),
                    source_node_id: node.into(),
                    expected_node_id: node.into(),
                }],
                add: &tags.iter().map(|tag| (*tag).to_owned()).collect::<Vec<_>>(),
                remove: &[],
                location: None,
                principal: "admin",
                operation_id: storage_contract::OperationId::generate(),
            })
            .unwrap()
            .result["nodes"][0]["node_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn merge(&mut self, file: &str, inputs: &[&str], chosen: &str, survivor: &str) -> String {
        let inputs = inputs
            .iter()
            .map(|branch| FileMergeInput {
                branch_id: (*branch).into(),
                expected_node_id: self.engine.file_branch_head(branch).unwrap(),
            })
            .collect::<Vec<_>>();
        self.engine
            .merge_file_branches(crate::FileBranchMergeRequest {
                file_id: file,
                inputs: &inputs,
                chosen_node_id: chosen,
                surviving_branch_id: survivor,
                location: None,
                principal: "admin",
                operation_id: storage_contract::OperationId::generate(),
            })
            .unwrap()
            .result["node_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn rename(&mut self, from: &str, to: &str) {
        self.engine
            .file_namespace(
                &self.repo.repository_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Rename,
                    from,
                    Some(to),
                )
                .unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
    fn scope<'a>(&'a self, file: &'a str) -> FileHistoryScope<'a> {
        self.engine
            .file_history_scope(file, &self.repo.history_tree_id)
            .unwrap()
    }
    fn root(&self) -> HistoryRoot {
        self.engine
            .history_root(&self.repo.history_tree_id)
            .unwrap()
    }
}

/// A graph fixture with content, renames, tags, a three-way merge, a merge
/// that reuses an existing node, a second file and a detached file.
struct Graph {
    pool: Pool,
    file: String,
    other_file: String,
    merge_node: String,
    reused_merge_id: String,
    left: String,
    right: String,
    third: String,
}

fn graph() -> Graph {
    let mut pool = Pool::new();
    let base = pool.save("docs/report.txt", b"base", "create report");
    let file = base.file_id.clone();
    let left = pool.fork(&file, &base.version_id, "left");
    pool.choose(&file, &left, "docs/report.txt");
    pool.save("docs/report.txt", b"left body", "left save");
    let right = pool.fork(&file, &base.version_id, "right");
    pool.choose(&file, &right, "docs/report.txt");
    pool.save("docs/report.txt", b"right body", "right save");
    let right_head = pool.engine.file_branch_head(&right).unwrap();
    pool.tag(&file, &right, &right_head, &["review", "草稿"]);
    let third = pool.fork(&file, &base.version_id, "third");
    pool.choose(&file, &third, "docs/report.txt");
    pool.save("docs/report.txt", b"third body", "third save");
    let chosen = pool.engine.file_branch_head(&third).unwrap();
    pool.choose(&file, &left, "docs/report.txt");
    let merge_node = pool.merge(&file, &[&left, &right], &chosen, &left);
    // Reuse: merging the survivor with the third branch whose head is the
    // merge's chosen node records inputs without creating a node.
    pool.merge(&file, &[&left, &third], &merge_node, &left);
    let reused_merge_id = pool
        .engine
        .state()
        .file_branch_merges
        .values()
        .find(|merge| merge.reused_node)
        .unwrap()
        .merge_id
        .clone();
    pool.rename("docs/report.txt", "docs/renamed.txt");
    pool.save("docs/renamed.txt", b"after rename", "renamed save");
    let other = pool.save("docs/other.txt", b"other", "other file");
    pool.save("docs/other.txt", b"other two", "other file again");
    pool.engine
        .set_file_note(
            &file,
            None,
            &storage_contract::Note::parse("file note".into()).unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    Graph {
        pool,
        file,
        other_file: other.file_id,
        merge_node,
        reused_merge_id,
        left,
        right,
        third,
    }
}

// Independent whole-history oracles over State, never through the scope.

fn oracle_nodes(
    engine: &Engine,
    owners: &crate::state_projection::DerivedOwners,
    file: &str,
    history: &str,
) -> Vec<NodeKey> {
    let state = engine.state();
    let mut keys = Vec::new();
    for version in state
        .versions
        .values()
        .filter(|v| v.file_id == file && v.history_tree_id == history)
    {
        let mut sequence = version.version_sequence;
        let mut best: Option<(u64, &str)> = None;
        for commit in state.commits.values() {
            if engine
                .commit_namespace(&commit.commit_id)
                .unwrap()
                .entries()
                .unwrap()
                .iter()
                .any(|entry| {
                    entry.kind == NamespaceEntryKind::File && entry.version_id == version.version_id
                })
            {
                let candidate = (commit.sequence, commit.commit_id.as_str());
                if best.is_none_or(|current| candidate < current) {
                    best = Some(candidate);
                }
            }
        }
        if let Some((commit_sequence, commit_id)) = best {
            sequence = commit_sequence;
            assert_eq!(
                owners.first_commit(&version.version_id).unwrap().commit_id,
                commit_id
            );
        }
        keys.push(NodeKey {
            sequence,
            node_id: version.version_id.clone(),
        });
    }
    for node in state
        .file_path_nodes
        .values()
        .filter(|n| n.file_id == file && n.history_tree_id == history)
    {
        keys.push(NodeKey {
            sequence: node.sequence,
            node_id: node.node_id.clone(),
        });
    }
    for node in state
        .file_metadata_nodes
        .values()
        .filter(|n| n.file_id == file && n.history_tree_id == history)
    {
        keys.push(NodeKey {
            sequence: node.sequence,
            node_id: node.node_id.clone(),
        });
    }
    keys.sort();
    keys
}

fn oracle_merges(state: &State, file: &str, history: &str) -> Vec<MergeKey> {
    let mut keys = state
        .file_branch_merges
        .values()
        .filter(|m| m.file_id == file && m.history_tree_id == history)
        .map(|m| MergeKey {
            sequence: m.sequence,
            merge_id: m.merge_id.clone(),
        })
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

fn oracle_versions(state: &State, file: &str, history: &str) -> Vec<VersionKey> {
    let mut keys = state
        .versions
        .values()
        .filter(|v| v.file_id == file && v.history_tree_id == history)
        .map(|v| VersionKey {
            version_sequence: v.version_sequence,
            version_id: v.version_id.clone(),
        })
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

fn oracle_branches(state: &State, file: &str, history: &str) -> Vec<String> {
    state
        .branches
        .values()
        .filter(|b| {
            b.file_id.as_deref() == Some(file)
                && b.history_tree_id == history
                && b.state != BranchState::Migrated
        })
        .map(|b| b.branch_id.clone())
        .collect()
}

fn oracle_parents(engine: &Engine, file: &str, history: &str, node: &str) -> Vec<String> {
    let state = engine.state();
    let visible = |id: &str| {
        state
            .versions
            .get(id)
            .is_some_and(|v| v.file_id == file && v.history_tree_id == history)
            || state
                .file_path_nodes
                .get(id)
                .is_some_and(|n| n.file_id == file && n.history_tree_id == history)
            || state
                .file_metadata_nodes
                .get(id)
                .is_some_and(|n| n.file_id == file && n.history_tree_id == history)
    };
    let mut parents = engine
        .file_node_parents(file, node)
        .unwrap()
        .into_iter()
        .filter(|parent| visible(parent))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    parents.dedup();
    parents
}

/// Walk forward from the first page, then backward from the last page, with
/// every page size from 1 to `len + 1`; both walks must reproduce the oracle.
fn exhaust<K: Clone + Ord + std::fmt::Debug>(
    oracle: &[K],
    select: impl Fn(Option<(&K, Direction)>, usize) -> Selection<K>,
) {
    for limit in 1..=oracle.len() + 1 {
        let mut collected = Vec::new();
        let mut boundary: Option<K> = None;
        let mut pages = 0;
        loop {
            let page = select(
                boundary.as_ref().map(|key| (key, Direction::Forward)),
                limit,
            );
            assert!(page.keys.len() <= limit);
            assert!(page.keys.windows(2).all(|pair| pair[0] < pair[1]));
            assert_eq!(
                page.more_before,
                boundary.is_some(),
                "limit {limit}: more_before"
            );
            collected.extend(page.keys.iter().cloned());
            pages += 1;
            if !page.more_after {
                break;
            }
            assert!(
                !page.keys.is_empty(),
                "a page with more_after must make progress"
            );
            boundary = page.keys.last().cloned();
        }
        assert_eq!(collected, oracle, "forward limit {limit}");
        assert_eq!(pages, oracle.len().div_ceil(limit).max(1));
        // Backward from the end: start with a boundary beyond the last key.
        let mut collected = Vec::new();
        let mut boundary: Option<K> = None;
        let mut first = true;
        loop {
            let page = if first {
                first = false;
                // The last forward page is the initial backward seed.
                let mut cursor: Option<K> = None;
                loop {
                    let page = select(cursor.as_ref().map(|key| (key, Direction::Forward)), limit);
                    if !page.more_after {
                        break page;
                    }
                    cursor = page.keys.last().cloned();
                }
            } else {
                select(
                    boundary.as_ref().map(|key| (key, Direction::Backward)),
                    limit,
                )
            };
            assert!(page.keys.windows(2).all(|pair| pair[0] < pair[1]));
            let mut keys = page.keys.clone();
            keys.extend(collected);
            collected = keys;
            if !page.more_before {
                break;
            }
            assert!(!page.keys.is_empty());
            boundary = page.keys.first().cloned();
        }
        assert_eq!(collected, oracle, "backward limit {limit}");
    }
}

fn exhaust_slices<T: Clone + PartialEq + std::fmt::Debug>(
    total: &[T],
    read: impl Fn(usize, usize) -> Slice<T>,
) {
    for limit in 1..=total.len() + 1 {
        let mut collected = Vec::new();
        let mut offset = 0;
        loop {
            let page = read(offset, limit);
            assert_eq!(page.total, total.len());
            assert_eq!(page.offset, offset);
            assert!(page.items.len() <= limit);
            collected.extend(page.items.iter().cloned());
            offset += page.items.len();
            if offset >= page.total {
                assert!(page.items.len() < limit || offset == page.total);
                break;
            }
        }
        assert_eq!(collected, total, "slice limit {limit}");
    }
    let beyond = read(total.len() + 5, 3);
    assert!(beyond.items.is_empty());
    assert_eq!(beyond.total, total.len());
}

#[test]
fn exhaustive_paging_of_every_collection_matches_whole_history_oracles() {
    let g = graph();
    let history = g.pool.repo.history_tree_id.clone();
    let scope = g.pool.scope(&g.file);
    let state = g.pool.engine.state();
    let nodes = oracle_nodes(&g.pool.engine, scope.owners, &g.file, &history);
    // Five bodies, one rename, one tags node and one merge node.
    assert_eq!(nodes.len(), 8, "{nodes:?}");
    exhaust(&nodes, |boundary, limit| {
        scope.select_nodes(boundary, limit).unwrap()
    });
    let merges = oracle_merges(state, &g.file, &history);
    assert_eq!(merges.len(), 2);
    exhaust(&merges, |boundary, limit| {
        scope.select_merges(boundary, limit)
    });
    let versions = oracle_versions(state, &g.file, &history);
    assert_eq!(versions.len(), 5);
    exhaust(&versions, |boundary, limit| {
        scope.select_versions(boundary, limit)
    });
    let branches = oracle_branches(state, &g.file, &history);
    assert!(branches.len() >= 3, "{branches:?}");
    exhaust(&branches, |boundary, limit| {
        scope
            .branch_window(
                boundary.map(|(id, direction)| (id.as_str(), direction)),
                limit,
            )
            .unwrap()
    });
    for limit in 1..=branches.len() + 1 {
        let mut collected = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let page = scope.select_branches(after.as_deref(), limit).unwrap();
            assert!(page.keys.len() <= limit);
            collected.extend(page.keys.iter().cloned());
            if !page.more_after {
                break;
            }
            after = page.keys.last().cloned();
        }
        assert_eq!(collected, branches);
    }
    for branch in &branches {
        let record = scope.branch_record(branch).unwrap();
        let stored = &state.branches[branch];
        assert_eq!(
            record.read_only,
            stored.state != BranchState::Active || state.file_branch_aliases.contains_key(branch)
        );
        assert_eq!(
            record.alias_of,
            state.file_branch_aliases.get(branch).cloned()
        );
        let head = &state.commits[&stored.head_commit_id];
        assert_eq!(
            record.pending_count,
            head.pending_file_versions
                .keys()
                .chain(stored.pending_file_versions.keys())
                .collect::<BTreeSet<_>>()
                .len()
        );
    }
    // Parents of every node, paged with every limit, equal the engine's
    // single-node projection filtered to this file history.
    for key in &nodes {
        let expected = oracle_parents(&g.pool.engine, &g.file, &history, &key.node_id);
        for limit in 1..=expected.len() + 1 {
            let mut collected = Vec::new();
            let mut after: Option<String> = None;
            loop {
                let page = scope
                    .node_parents(&key.node_id, after.as_deref(), limit)
                    .unwrap();
                assert!(page.keys.len() <= limit);
                collected.extend(page.keys.iter().cloned());
                if !page.more_after {
                    break;
                }
                after = page.keys.last().cloned();
            }
            assert_eq!(collected, expected, "parents of {}", key.node_id);
        }
    }
    let merge_node_parents = oracle_parents(&g.pool.engine, &g.file, &history, &g.merge_node);
    assert_eq!(
        merge_node_parents.len(),
        3,
        "three-way merge keeps every input and the chosen body"
    );
    // Merge inputs and stored parents by index, versions' sources by index.
    for merge in &merges {
        let stored = &state.file_branch_merges[&merge.merge_id];
        let pairs = stored
            .input_branch_ids
            .iter()
            .cloned()
            .zip(stored.input_node_ids.iter().cloned())
            .collect::<Vec<_>>();
        exhaust_slices(&pairs, |offset, limit| {
            scope.merge_inputs(&merge.merge_id, offset, limit).unwrap()
        });
        exhaust_slices(&stored.parent_node_ids, |offset, limit| {
            scope.merge_parents(&merge.merge_id, offset, limit).unwrap()
        });
        let record = scope.merge_record(merge).unwrap();
        assert_eq!(record.input_count, pairs.len());
        assert_eq!(record.parent_count, stored.parent_node_ids.len());
        assert_eq!(record.reused_node, merge.merge_id == g.reused_merge_id);
    }
    for version in &versions {
        let stored = scope.version_record(version).unwrap();
        exhaust_slices(&stored.source_version_ids, |offset, limit| {
            scope
                .version_sources(&version.version_id, offset, limit)
                .unwrap()
        });
    }
    // The other file and a foreign node are not visible through this scope.
    let other = g.pool.scope(&g.other_file);
    let other_nodes = oracle_nodes(&g.pool.engine, other.owners, &g.other_file, &history);
    assert_eq!(other_nodes.len(), 2);
    exhaust(&other_nodes, |boundary, limit| {
        other.select_nodes(boundary, limit).unwrap()
    });
    assert_eq!(
        scope.node_key(&other_nodes[0].node_id).unwrap_err().code(),
        storage_contract::ErrorCode::FileNodeNotFound
    );
    assert_eq!(
        scope
            .node_parents(&other_nodes[0].node_id, None, 5)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileNodeNotFound
    );
    assert_eq!(
        other
            .merge_inputs(&merges[0].merge_id, 0, 5)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::NotFound
    );
    assert_eq!(
        other
            .version_sources(&versions[0].version_id, 0, 5)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::VersionNotFound
    );
    assert_eq!(
        other.branch_record(&g.left).unwrap_err().code(),
        storage_contract::ErrorCode::FileBranchNotFound
    );
    assert!(other.select_merges(None, 10).keys.is_empty());
    assert_eq!(
        other.select_branches(None, 10).unwrap().keys,
        oracle_branches(state, &g.other_file, &history)
    );
    assert!(
        !other
            .select_branches(None, 10)
            .unwrap()
            .keys
            .contains(&g.left)
    );
    // Empty sets and an unknown history: no keys, no continuation.
    let empty = g
        .pool
        .engine
        .file_history_scope("file_missing", &history)
        .unwrap();
    assert_eq!(
        empty.select_nodes(None, 3).unwrap(),
        Selection {
            keys: vec![],
            more_before: false,
            more_after: false
        }
    );
    assert_eq!(
        empty.select_versions(None, 3),
        Selection {
            keys: vec![],
            more_before: false,
            more_after: false
        }
    );
    let _ = (&g.right, &g.third);
}

#[test]
fn node_records_carry_bounded_fields_and_match_the_previous_projection_rules() {
    let g = graph();
    let history = g.pool.repo.history_tree_id.clone();
    let scope = g.pool.scope(&g.file);
    let state = g.pool.engine.state();
    let mut reader = scope.node_reader();
    for key in oracle_nodes(&g.pool.engine, scope.owners, &g.file, &history) {
        let record = reader.record(&key).unwrap();
        assert_eq!(scope.node_key(&key.node_id).unwrap(), key);
        assert_eq!(record.sequence, key.sequence);
        assert_eq!(record.node_id, key.node_id);
        assert_eq!(
            record.tags,
            g.pool.engine.file_node_tags(&g.file, &key.node_id).unwrap()
        );
        assert_eq!(
            record.version_id,
            g.pool
                .engine
                .file_node_version(&g.file, &key.node_id)
                .unwrap()
        );
        let body = &state.versions[&record.version_id];
        assert_eq!(record.size, body.logical_size);
        assert_eq!(record.content_hash, body.content_hash);
        assert_eq!(record.storage_name, body.storage_name);
        assert!(record.author.inline.is_some() && record.message.inline.is_some());
        match record.kind {
            NodeKind::Content => {
                assert_eq!(
                    record.created_at,
                    NodeTime::Rfc3339(body.created_at.clone())
                );
                assert!(record.operation_id.is_none());
                assert!(record.previous_path.is_none());
                let commit = scope.first_commit(&key.node_id).unwrap();
                assert_eq!(
                    record.message.inline.as_deref(),
                    Some(commit.message.as_str())
                );
                assert_eq!(record.author.inline.as_deref(), Some("admin"));
                assert!(record.path.ends_with(".txt"));
            }
            NodeKind::Rename => {
                let node = &state.file_path_nodes[&key.node_id];
                assert_eq!(
                    record.previous_path.as_deref(),
                    Some(node.previous_path.as_str())
                );
                assert_eq!(record.path, node.path);
                assert_eq!(record.created_at, NodeTime::UnixSeconds(node.created_at));
                assert_eq!(record.message.bytes, 0);
            }
            NodeKind::Tags | NodeKind::Merge => {
                let node = &state.file_metadata_nodes[&key.node_id];
                assert_eq!(record.created_at, NodeTime::UnixSeconds(node.created_at));
                assert_eq!(record.branch_id.as_deref(), Some(node.branch_id.as_str()));
                assert_eq!(record.tags, node.tags);
                assert!(record.path == "docs/report.txt" || record.path == "docs/renamed.txt");
            }
        }
    }
    let merge = reader
        .record(&scope.node_key(&g.merge_node).unwrap())
        .unwrap();
    assert_eq!(merge.kind, NodeKind::Merge);
    assert_eq!(
        merge.path, "docs/report.txt",
        "merge node resolves its path through the chosen node"
    );
    let tagged = state
        .file_metadata_nodes
        .values()
        .find(|node| node.kind == FileMetadataKind::Tags)
        .unwrap();
    assert_eq!(
        reader
            .record(&scope.node_key(&tagged.node_id).unwrap())
            .unwrap()
            .tags,
        BTreeSet::from(["review".to_owned(), "草稿".to_owned()])
    );
    // The version saved after the tag on the right branch inherits its tags.
    let right_head = g.pool.engine.file_branch_head(&g.right).unwrap();
    assert_eq!(right_head, tagged.node_id);
    assert_eq!(
        reader
            .record(&scope.node_key(&g.merge_node).unwrap())
            .unwrap()
            .tags,
        state.file_metadata_nodes[&g.merge_node].tags
    );
    assert_eq!(
        scope
            .node_record(&NodeKey {
                sequence: 0,
                node_id: "missing".into()
            })
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileNodeNotFound
    );
}

#[test]
fn anchor_boundaries_and_u64_max_sequences_page_without_loss_or_cycles() {
    let mut pool = Pool::new();
    let first = pool.save("a.txt", b"1", "first");
    let second = pool.save("a.txt", b"2", "second");
    let third = pool.save("a.txt", b"3", "third");
    let file = first.file_id.clone();
    let history = pool.repo.history_tree_id.clone();
    // Force extreme and tied sequences through the derived commit index by
    // rewriting commits of the admitted State in place (test-only).
    let mut state = pool.engine.state().clone();
    let commit_of = |state: &State, version: &str| {
        state
            .commits
            .values()
            .find(|c| {
                pool.engine
                    .commit_namespace(&c.commit_id)
                    .unwrap()
                    .entries()
                    .unwrap()
                    .iter()
                    .any(|e| e.version_id == version)
            })
            .unwrap()
            .commit_id
            .clone()
    };
    let max_commit = commit_of(&state, &third.version_id);
    let tie_commit = commit_of(&state, &second.version_id);
    let base_commit = commit_of(&state, &first.version_id);
    state.commits.get_mut(&max_commit).unwrap().sequence = u64::MAX;
    let tie = state.commits[&base_commit].sequence;
    state.commits.get_mut(&tie_commit).unwrap().sequence = tie;
    pool.engine.state = std::sync::Arc::new(state);
    // Replacing the State detaches the admitted catalog; a scope is refused
    // until a catalog binds the new allocation again (test-only rebinding).
    assert_eq!(
        pool.engine
            .file_history_scope(&file, &history)
            .map(|_| ())
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
    let (catalog, _) = pool
        .engine
        .with_namespace_source(|source| {
            crate::history_package::prepare_history_manifests(
                pool.engine.state(),
                crate::disk::MAX_METADATA,
                source,
            )
        })
        .unwrap();
    pool.engine.history_catalog = Some(crate::engine::HistoryCatalog::published(
        &pool.engine.state,
        catalog,
    ));
    // The cold scope rebuilds the owner index for the replaced State.
    let scope = pool.engine.file_history_scope(&file, &history).unwrap();
    let nodes = oracle_nodes(&pool.engine, scope.owners, &file, &history);
    assert_eq!(nodes[0].sequence, u64::MAX);
    assert_eq!(nodes[1].sequence, nodes[2].sequence);
    assert!(nodes[1].node_id < nodes[2].node_id);
    exhaust(&nodes, |boundary, limit| {
        scope.select_nodes(boundary, limit).unwrap()
    });
    // Anchor: a page that starts at the anchor. The server positions the page
    // by selecting forward from the key preceding the anchor; model it here.
    for anchor in &nodes {
        let before = scope
            .select_nodes(Some((anchor, Direction::Backward)), 1)
            .unwrap();
        let start = before.keys.first();
        let page = scope
            .select_nodes(start.map(|key| (key, Direction::Forward)), 2)
            .unwrap();
        assert_eq!(page.keys.first(), Some(anchor));
        assert_eq!(page.more_before, start.is_some());
    }
    let last = nodes.last().unwrap();
    let after_last = scope
        .select_nodes(Some((last, Direction::Forward)), 5)
        .unwrap();
    assert!(after_last.keys.is_empty() && !after_last.more_after && after_last.more_before);
    let before_first = scope
        .select_nodes(Some((&nodes[0], Direction::Backward)), 5)
        .unwrap();
    assert!(before_first.keys.is_empty() && !before_first.more_before && before_first.more_after);
}

#[test]
fn field_slices_respect_utf8_boundaries_and_report_completion_exactly() {
    let mut pool = Pool::new();
    let message = format!("{}é中文𝄞{}", "a".repeat(10), "z".repeat(300));
    let entry = pool.save("m.txt", b"m", &message);
    let history = pool.repo.history_tree_id.clone();
    let scope = pool
        .engine
        .file_history_scope(&entry.file_id, &history)
        .unwrap();
    let node = scope.node_key(&entry.version_id).unwrap();
    let record = scope.node_record(&node).unwrap();
    assert_eq!(record.message.bytes, message.len());
    assert!(
        record.message.inline.is_none(),
        "messages above MAX_INLINE_TEXT_BYTES are not inlined"
    );
    assert_eq!(record.author.inline.as_deref(), Some("admin"));
    // Every successful non-final fragment advances. A request that cannot
    // hold the next scalar fails explicitly, rather than returning its cursor.
    for max_bytes in 1..=message.len() + 1 {
        let mut assembled = String::new();
        let mut offset = 0;
        loop {
            let result = scope.node_field(&entry.version_id, NodeField::Message, offset, max_bytes);
            if message[offset..]
                .chars()
                .next()
                .is_some_and(|ch| ch.len_utf8() > max_bytes)
            {
                assert_eq!(
                    result.unwrap_err().code(),
                    storage_contract::ErrorCode::InvalidRequest
                );
                break;
            }
            let slice = result.unwrap();
            assert_eq!(slice.total_bytes, message.len());
            assert_eq!(slice.offset, offset);
            assert!(slice.text.len() <= max_bytes);
            assembled.push_str(&slice.text);
            offset += slice.text.len();
            if slice.complete {
                assert_eq!(offset, message.len());
                break;
            }
            assert!(!slice.text.is_empty());
        }
        if max_bytes >= 4 {
            assert_eq!(assembled, message, "max_bytes {max_bytes}");
        }
    }
    assert_eq!(
        scope
            .node_field(&entry.version_id, NodeField::Message, 11, 64)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::InvalidRequest
    );
    let author = scope
        .node_field(&entry.version_id, NodeField::Author, 0, 64)
        .unwrap();
    assert_eq!(
        author,
        FieldSlice {
            total_bytes: 5,
            offset: 0,
            text: "admin".into(),
            complete: true
        }
    );
    let at_end = scope
        .node_field(&entry.version_id, NodeField::Message, message.len(), 8)
        .unwrap();
    assert!(at_end.text.is_empty() && at_end.complete);
    assert_eq!(
        scope
            .node_field(&entry.version_id, NodeField::Message, message.len() + 1, 8)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::InvalidRequest
    );
    pool.rename("m.txt", "n.txt");
    let scope = pool
        .engine
        .file_history_scope(&entry.file_id, &history)
        .unwrap();
    let rename = pool
        .engine
        .state()
        .file_path_nodes
        .values()
        .next()
        .unwrap()
        .node_id
        .clone();
    let empty = scope.node_field(&rename, NodeField::Message, 0, 8).unwrap();
    assert_eq!(
        empty,
        FieldSlice {
            total_bytes: 0,
            offset: 0,
            text: String::new(),
            complete: true
        }
    );
}

#[test]
fn current_context_follows_the_deterministic_owner_rule_and_survives_deletion() {
    let g = graph();
    let scope = g.pool.scope(&g.file);
    let resolved = scope.current_context(ContextLocation::Resolve).unwrap();
    assert_eq!(resolved.repository_id, g.pool.repo.repository_id);
    assert_eq!(resolved.path.as_deref(), Some("docs/renamed.txt"));
    assert_eq!(resolved.note, "file note");
    assert_eq!(
        resolved.selected_branch_id.as_deref(),
        Some(g.left.as_str())
    );
    let current = resolved.current.as_ref().unwrap();
    assert_eq!(current.name, "renamed.txt");
    assert_eq!(
        resolved.current_node_id.as_deref(),
        Some(current.version_id.as_str())
    );
    let explicit = scope
        .current_context(ContextLocation::PathKey("docs/renamed.txt"))
        .unwrap();
    assert_eq!(explicit, resolved);
    let missing = scope
        .current_context(ContextLocation::PathKey("docs/report.txt"))
        .unwrap();
    assert!(missing.path.is_none() && missing.current.is_none());
    assert!(missing.current_node_id.is_none());
    assert_eq!(missing.note, "file note");
    let mut g = g;
    g.pool
        .engine
        .file_namespace(
            &g.pool.repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Delete,
                "docs/renamed.txt",
                None,
            )
            .unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let scope = g.pool.scope(&g.file);
    let deleted = scope.current_context(ContextLocation::Resolve).unwrap();
    assert!(
        deleted.path.is_none() && deleted.current.is_none() && deleted.current_node_id.is_none()
    );
    assert_eq!(
        deleted.selected_branch_id,
        g.pool.engine.state().file_selections.get(&g.file).cloned()
    );
    let history = g.pool.repo.history_tree_id.clone();
    let nodes = oracle_nodes(&g.pool.engine, scope.owners, &g.file, &history);
    assert!(!nodes.is_empty(), "deleted files keep readable history");
    exhaust(&nodes, |boundary, limit| {
        scope.select_nodes(boundary, limit).unwrap()
    });
}

#[test]
fn history_root_is_the_exact_catalog_hash_stable_across_unrelated_writes_and_reopen() {
    let mut pool = Pool::new();
    let entry = pool.save("r.txt", b"r", "root");
    let file = entry.file_id.clone();
    let history = pool.repo.history_tree_id.clone();
    let root = pool.root();
    assert_eq!(
        pool.engine.file_history_owner(&file),
        Some(history.as_str())
    );
    assert_eq!(pool.engine.file_history_owner("file_missing"), None);
    assert_eq!(root.as_str().len(), 64);
    assert_eq!(HistoryRoot::parse(root.as_str()).unwrap(), root);
    for bad in ["", "abc", &"A".repeat(64), &"g".repeat(64), &"0".repeat(63)] {
        assert_eq!(
            HistoryRoot::parse(bad).unwrap_err().code(),
            storage_contract::ErrorCode::InvalidRequest
        );
    }
    // The root equals the manifest hash recorded in the published catalog.
    let (catalog, _) = pool
        .engine
        .with_namespace_source(|source| {
            crate::history_package::prepare_history_manifests(
                pool.engine.state(),
                crate::disk::MAX_METADATA,
                source,
            )
        })
        .unwrap();
    assert_eq!(
        catalog.catalog_binding(&history).unwrap().hash,
        root.as_str()
    );
    // An unrelated history (another repository) does not change this root.
    let other = pool
        .engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    pool.save_in(&other.main_branch_id, "o.txt", b"o", "other");
    assert_eq!(pool.root(), root);
    let other_root = pool.engine.history_root(&other.history_tree_id).unwrap();
    assert_ne!(other_root, root);
    // Note, tag and selection changes on this history each move the root.
    let mut previous = root.clone();
    pool.engine
        .set_file_note(
            &file,
            None,
            &storage_contract::Note::parse("n".into()).unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let after_note = pool.root();
    assert_ne!(after_note, previous);
    previous = after_note;
    let fork = pool.fork(&file, &entry.version_id, "fork");
    let after_fork = pool.root();
    assert_ne!(after_fork, previous);
    previous = after_fork;
    let head = pool.engine.file_branch_head(&fork).unwrap();
    pool.tag(&file, &fork, &head, &["t"]);
    let after_tag = pool.root();
    assert_ne!(after_tag, previous);
    previous = after_tag;
    pool.choose(&file, &fork, "r.txt");
    let after_select = pool.root();
    assert_ne!(after_select, previous);
    assert_eq!(
        pool.engine
            .history_root("history_missing")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::HistoryNotFound
    );
    // Cold reopen: recovery binds the same catalog without a warm cache.
    let final_root = pool.root();
    let Pool {
        _temporary,
        root: system,
        engine,
        repo,
        ..
    } = pool;
    drop(engine);
    let reopened = Engine::open(&system).unwrap();
    // Cold lookup and the later indexed lookup resolve the same owner.
    assert_eq!(reopened.file_history_owner(&file), Some(history.as_str()));
    assert_eq!(reopened.file_history_owner("file_missing"), None);
    assert_eq!(
        reopened.history_root(&repo.history_tree_id).unwrap(),
        final_root
    );
    assert_eq!(
        reopened.history_root(&other.history_tree_id).unwrap(),
        other_root
    );
    // Memoized: a second read returns the same value without member reads.
    assert_eq!(
        reopened.history_root(&repo.history_tree_id).unwrap(),
        final_root
    );
    assert_eq!(
        reopened.history_root("history_missing").unwrap_err().code(),
        storage_contract::ErrorCode::HistoryNotFound
    );
    let scope = reopened
        .file_history_scope(&file, &repo.history_tree_id)
        .unwrap();
    assert_eq!(reopened.file_history_owner(&file), Some(history.as_str()));
    assert_eq!(reopened.file_history_owner("file_missing"), None);
    let nodes = oracle_nodes(&reopened, scope.owners, &file, &repo.history_tree_id);
    exhaust(&nodes, |boundary, limit| {
        scope.select_nodes(boundary, limit).unwrap()
    });
    drop(_temporary);
}

#[test]
fn a_fresh_pool_without_publication_has_no_history_catalog() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("disk")]),
    )
    .unwrap();
    assert_eq!(
        engine.history_root("history_x").unwrap_err().code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert_eq!(engine.file_history_owner("file"), None);
    let scope = engine.file_history_scope("file", "history_x");
    assert_eq!(
        scope.map(|_| ()).unwrap_err().code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
}

#[test]
fn many_parent_nodes_page_their_parents_in_id_order() {
    let mut pool = Pool::new();
    let base = pool.save("p.txt", b"base", "base");
    let file = base.file_id.clone();
    let history = pool.repo.history_tree_id.clone();
    let mut forks = Vec::new();
    for index in 0..6 {
        let fork = pool.fork(&file, &base.version_id, &format!("fork{index}"));
        pool.choose(&file, &fork, "p.txt");
        pool.save("p.txt", format!("body {index}").as_bytes(), "fork save");
        forks.push(fork);
    }
    let survivor = forks[0].clone();
    pool.choose(&file, &survivor, "p.txt");
    let chosen = pool.engine.file_branch_head(&forks[5]).unwrap();
    let inputs = forks.iter().map(String::as_str).collect::<Vec<_>>();
    let merge = pool.merge(&file, &inputs, &chosen, &survivor);
    let scope = pool.engine.file_history_scope(&file, &history).unwrap();
    let expected = oracle_parents(&pool.engine, &file, &history, &merge);
    assert_eq!(expected.len(), 6);
    for limit in 1..=7 {
        let mut collected = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let page = scope.node_parents(&merge, after.as_deref(), limit).unwrap();
            collected.extend(page.keys.iter().cloned());
            if !page.more_after {
                break;
            }
            after = page.keys.last().cloned();
        }
        assert_eq!(collected, expected);
    }
    let inputs = scope
        .merge_inputs(
            pool.engine
                .state()
                .file_branch_merges
                .keys()
                .next()
                .unwrap(),
            0,
            100,
        )
        .unwrap();
    assert_eq!(inputs.total, 6);
    assert_eq!(
        inputs
            .items
            .iter()
            .map(|(branch, _)| branch.clone())
            .collect::<Vec<_>>(),
        forks
    );
}

fn observe_all_history_relations(
    engine: &Engine,
    file: &str,
    history: &str,
    node: &str,
    merge: &str,
) -> impl PartialEq + std::fmt::Debug + use<> {
    let scope = engine.file_history_scope(file, history).unwrap();
    let nodes = scope.select_nodes(None, 2).unwrap();
    let records = nodes
        .keys
        .iter()
        .map(|key| scope.node_record(key).unwrap())
        .collect::<Vec<_>>();
    let branches = scope.select_branches(None, 2).unwrap();
    let branch_records = branches
        .keys
        .iter()
        .map(|id| scope.branch_record(id).unwrap())
        .collect::<Vec<_>>();
    let merges = scope.select_merges(None, 2);
    let merge_records = merges
        .keys
        .iter()
        .map(|key| scope.merge_record(key).unwrap())
        .collect::<Vec<_>>();
    let versions = scope.select_versions(None, 2);
    let version = &versions.keys[0].version_id;
    (
        engine.history_root(history).unwrap(),
        (
            nodes,
            records,
            scope.node_parents(node, None, 2).unwrap(),
            branches,
            branch_records,
        ),
        (
            merges,
            merge_records,
            scope.merge_inputs(merge, 0, 2).unwrap(),
            scope.merge_parents(merge, 0, 2).unwrap(),
            versions.clone(),
            scope.version_sources(version, 0, 2).unwrap(),
            scope.node_field(version, NodeField::Message, 0, 8).unwrap(),
        ),
    )
}

#[test]
fn local_plan_preparation_preserves_exact_catalog_for_unique_and_shared_state_owners() {
    for shared in [false, true] {
        let mut g = graph();
        let old = g.pool.save("disposable.txt", b"old", "old");
        g.pool.save("disposable.txt", b"new", "new");
        // Historical-save plans have a directory-history scope, whereas the
        // observed history deliberately has independent file branches.
        let other = g
            .pool
            .engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Plan source").unwrap(),
                "P01",
                3,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let source = g.pool.engine.state.branches[&other.main_branch_id]
            .head_commit_id
            .clone();
        let history = g.pool.repo.history_tree_id.clone();
        let file = g.file.clone();
        let node = g.merge_node.clone();
        let merge = g.reused_merge_id.clone();
        let before = observe_all_history_relations(&g.pool.engine, &file, &history, &node, &merge);
        let old_address = std::sync::Arc::as_ptr(&g.pool.engine.state);
        let shared_state = shared.then(|| std::sync::Arc::clone(&g.pool.engine.state));
        let purge = g
            .pool
            .engine
            .create_purge_plan(&history, vec![old.version_id], "admin")
            .unwrap();
        assert_ne!(old_address, std::sync::Arc::as_ptr(&g.pool.engine.state));
        if let Some(old_state) = &shared_state {
            assert!(!old_state.plans.contains_key(&purge.plan_id));
        }
        assert_eq!(
            before,
            observe_all_history_relations(&g.pool.engine, &file, &history, &node, &merge)
        );
        let historical = g
            .pool
            .engine
            .create_historical_save_plan(
                &other.history_tree_id,
                crate::HistoricalSaveOptions {
                    source_commit_id: source,
                    name: "prepared".into(),
                    workspace_id: None,
                    primary_slot_id: None,
                    target_copies: None,
                },
                "admin",
            )
            .unwrap();
        assert_eq!(
            before,
            observe_all_history_relations(&g.pool.engine, &file, &history, &node, &merge)
        );
        for plan in [purge, historical] {
            g.pool
                .engine
                .cancel_plan(
                    &plan.plan_id,
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
            assert_eq!(
                before,
                observe_all_history_relations(&g.pool.engine, &file, &history, &node, &merge)
            );
        }
        drop(shared_state);
        let root_path = g.pool.root.clone();
        // Keep the fixture directory alive while replacing just the engine.
        drop(g.pool.engine);
        let mut reopened = Engine::open(&root_path).unwrap();
        assert_eq!(
            before,
            observe_all_history_relations(&reopened, &file, &history, &node, &merge)
        );
        let old_root = reopened.history_root(&history).unwrap();
        reopened
            .set_file_note(
                &file,
                None,
                &storage_contract::Note::parse("changed".into()).unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        assert_ne!(old_root, reopened.history_root(&history).unwrap());
    }
}

#[test]
fn file_history_owner_does_not_use_membership_from_another_state_allocation() {
    let mut pool = Pool::new();
    let entry = pool.save("owner.txt", b"body", "owner");
    let history = pool.repo.history_tree_id.clone();
    pool.engine.history_owners().unwrap();
    assert_eq!(
        pool.engine.file_history_owner(&entry.file_id),
        Some(history.as_str())
    );
    let original = std::sync::Arc::clone(&pool.engine.state);
    // Test-only replacement: leave the old catalog and owner index in place.
    let state = std::sync::Arc::make_mut(&mut pool.engine.state);
    state.versions.get_mut(&entry.version_id).unwrap().file_id = "file_rebound".into();
    assert_eq!(pool.engine.file_history_owner(&entry.file_id), None);
    assert_eq!(
        pool.engine.file_history_owner("file_rebound"),
        Some(history.as_str())
    );
    assert_eq!(
        pool.engine
            .file_history_scope("file_rebound", &history)
            .map(|_| ())
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
    pool.engine.state = original;
    assert_eq!(
        pool.engine.file_history_owner(&entry.file_id),
        Some(history.as_str())
    );
    assert_eq!(pool.engine.file_history_owner("file_rebound"), None);
    // A normal publication installs a new bound owner index.
    let other = pool.save("other.txt", b"other", "another file");
    assert_eq!(
        pool.engine.file_history_owner(&entry.file_id),
        Some(history.as_str())
    );
    assert_eq!(
        pool.engine.file_history_owner(&other.file_id),
        Some(history.as_str())
    );
}

#[test]
fn branch_windows_keep_history_and_migrated_filters_without_requiring_a_version() {
    let pool = Pool::new();
    let template = &pool.engine.state().branches[&pool.repo.main_branch_id];
    let mut state = State::default();
    for (id, file, history, migrated) in [
        ("a", Some("file-a"), "h1", false),
        ("b", Some("file-a"), "h2", false),
        ("c", Some("file-a"), "h1", true),
        ("d", Some("file-a"), "h1", false),
        ("e", None, "h1", false),
        ("f", Some("file-b"), "h1", false),
    ] {
        let mut branch = template.clone();
        branch.branch_id = id.into();
        branch.file_id = file.map(str::to_owned);
        branch.history_tree_id = history.into();
        branch.state = if migrated {
            BranchState::Migrated
        } else {
            BranchState::Active
        };
        branch
            .pending_file_versions
            .insert("file-a".into(), "inherited".into());
        state.branches.insert(id.into(), branch);
    }
    let owners =
        DerivedOwners::from_state(&state, &crate::namespace::fixture_store::FixtureSource).unwrap();
    assert_eq!(owners.file_rows("file-a").unwrap().branches.len(), 4);
    assert_eq!(
        owners.file_rows("file-a").unwrap().pending_branches.len(),
        6
    );
    assert!(state.versions.is_empty());
    for file in ["file-a", "file-b", "absent"] {
        for history in ["h1", "h2", "absent"] {
            let scope = FileHistoryScope {
                resources: &pool.engine.disk_resources,
                caches: &pool.engine.segment_caches,
                members: std::borrow::Cow::Owned(pool.engine.members()),
                observe: None,
                state: &state,
                owners: &owners,
                file_id: file,
                history_id: history,
            };
            let expected = oracle_branches(&state, file, history);
            exhaust(&expected, |boundary, limit| {
                scope
                    .branch_window(
                        boundary.map(|(id, direction)| (id.as_str(), direction)),
                        limit,
                    )
                    .unwrap()
            });
        }
    }
}

#[test]
fn detached_version_pages_match_borrowed_pages_and_reject_invalid_boundaries() {
    let g = graph();
    let scope = g.pool.scope(&g.file);
    let oracle = oracle_versions(g.pool.engine.state(), &g.file, &g.pool.repo.history_tree_id);
    exhaust(&oracle, |boundary, limit| {
        let plan = scope
            .prepare_version_page(
                boundary.map(|(key, direction)| (key.clone(), direction)),
                limit,
            )
            .unwrap();
        assert!(
            plan.versions
                .shares_storage_with(&g.pool.engine.state().versions)
        );
        assert!(
            plan.membership
                .as_ref()
                .unwrap()
                .shares_storage_with(scope.owners.version_membership(&g.file).unwrap())
        );
        let (selection, records) = plan.read();
        assert_eq!(selection, scope.select_versions(boundary, limit));
        assert_eq!(records.len(), selection.keys.len());
        for (key, record) in selection.keys.iter().zip(records) {
            assert_eq!(record.as_ref(), scope.version_record(key).unwrap());
        }
        selection
    });
    for (key, expected) in [
        (
            VersionKey {
                version_sequence: 0,
                version_id: "missing".into(),
            },
            storage_contract::ErrorCode::VersionNotFound,
        ),
        (
            VersionKey {
                version_sequence: oracle[0].version_sequence + 1,
                version_id: oracle[0].version_id.clone(),
            },
            storage_contract::ErrorCode::InvalidCursor,
        ),
    ] {
        let Err(error) = scope.prepare_version_page(Some((key, Direction::Forward)), 3) else {
            panic!("invalid boundary was admitted");
        };
        assert_eq!(error.code(), expected);
    }
    let empty = g
        .pool
        .scope("missing")
        .prepare_version_page(None, 3)
        .unwrap()
        .read();
    assert!(empty.0.keys.is_empty());
    assert!(empty.1.is_empty());
}

#[test]
fn detached_version_page_keeps_only_its_captured_roots_across_publication() {
    let mut pool = Pool::new();
    let entry = pool.save("file.txt", b"first", "first");
    let plan = pool
        .scope(&entry.file_id)
        .prepare_version_page(None, 10)
        .unwrap();
    pool.save("file.txt", b"second", "second");
    assert!(
        !plan
            .versions
            .shares_storage_with(&pool.engine.state().versions)
    );
    let (selected, versions) = plan.read();
    assert_eq!(selected.keys.len(), 1);
    assert_eq!(versions[0].version_id, entry.version_id);
    assert_eq!(
        pool.scope(&entry.file_id)
            .select_versions(None, 10)
            .keys
            .len(),
        2
    );
    drop(pool);
    assert_eq!(versions[0].logical_size, 5);
}

#[test]
fn detached_merge_pages_match_borrowed_windows_and_reject_invalid_boundaries() {
    let g = graph();
    let original = g.pool.scope(&g.file);
    let mut state = g.pool.engine.state().clone();
    // Ties exercise merge-id ordering in both directions without changing membership.
    for id in state.file_branch_merges.keys().cloned().collect::<Vec<_>>() {
        state.file_branch_merges.get_mut(&id).unwrap().sequence = u64::MAX;
    }
    let scope = FileHistoryScope {
        state: &state,
        ..original
    };
    let oracle = oracle_merges(&state, &g.file, &g.pool.repo.history_tree_id);
    exhaust(&oracle, |boundary, limit| {
        let plan = scope
            .prepare_merge_page(
                boundary.map(|(key, direction)| (key.clone(), direction)),
                limit,
            )
            .unwrap();
        assert!(plan.merges.shares_storage_with(&state.file_branch_merges));
        assert!(
            plan.membership
                .as_ref()
                .unwrap()
                .shares_storage_with(&scope.file_rows().unwrap().merges)
        );
        let (selection, records) = plan.read();
        assert_eq!(selection, scope.select_merges(boundary, limit));
        assert_eq!(
            records,
            selection
                .keys
                .iter()
                .map(|key| scope.merge_record(key).unwrap())
                .collect::<Vec<_>>()
        );
        selection
    });
    for (key, expected) in [
        (
            MergeKey {
                sequence: 0,
                merge_id: "missing".into(),
            },
            storage_contract::ErrorCode::NotFound,
        ),
        (
            MergeKey {
                sequence: 0,
                merge_id: oracle[0].merge_id.clone(),
            },
            storage_contract::ErrorCode::InvalidCursor,
        ),
    ] {
        let Err(error) = scope.prepare_merge_page(Some((key, Direction::Forward)), 2) else {
            panic!("invalid merge boundary was admitted");
        };
        assert_eq!(error.code(), expected);
    }
    for (file_id, history_id) in [
        (g.other_file.as_str(), g.pool.repo.history_tree_id.as_str()),
        (g.file.as_str(), "foreign-history"),
    ] {
        let other = FileHistoryScope {
            file_id,
            history_id,
            members: scope.members.clone(),
            ..scope
        };
        let (selection, records) = other.prepare_merge_page(None, 2).unwrap().read();
        assert!(selection.keys.is_empty());
        assert!(records.is_empty());
        assert_eq!(
            other
                .prepare_merge_page(Some((oracle[0].clone(), Direction::Forward)), 2)
                .err()
                .unwrap()
                .code(),
            storage_contract::ErrorCode::NotFound
        );
    }
    let expected = scope
        .select_merges(None, 10)
        .keys
        .iter()
        .map(|key| scope.merge_record(key).unwrap())
        .collect::<Vec<_>>();
    let plan = scope.prepare_merge_page(None, 10).unwrap();
    let retained = std::sync::Arc::downgrade(
        &state
            .file_branch_merges
            .get_shared(&oracle[0].merge_id)
            .unwrap(),
    );
    state
        .file_branch_merges
        .get_mut(&oracle[0].merge_id)
        .unwrap()
        .parent_node_ids
        .clear();
    state.file_branch_merges.remove(&oracle[1].merge_id);
    assert!(!plan.merges.shares_storage_with(&state.file_branch_merges));
    drop(state);
    drop(g);
    assert!(retained.upgrade().is_some());
    let (_, records) = plan.read();
    assert!(retained.upgrade().is_none());
    assert_eq!(records, expected);
}

fn assert_history_commit_records(engine: &Engine, history: &str) {
    let expected = engine
        .state()
        .commits
        .shared_values()
        .filter(|commit| commit.history_tree_id == history)
        .collect::<Vec<_>>();
    let actual = engine.history_commit_records(history);
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(std::sync::Arc::ptr_eq(actual, expected));
    }
    assert!(
        actual
            .windows(2)
            .all(|pair| pair[0].commit_id < pair[1].commit_id)
    );
}

#[test]
fn history_commit_records_match_scan_with_bound_index_and_unavailable_fallback() {
    let mut pool = Pool::new();
    pool.save("file.txt", b"body", "commit");
    let other = pool
        .engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other graph").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert!(pool.engine.available_history_owners().is_some());
    for history in [
        &pool.repo.history_tree_id,
        &other.history_tree_id,
        "missing",
    ] {
        assert_history_commit_records(&pool.engine, history);
    }
    let catalog = pool.engine.history_catalog.take();
    assert!(pool.engine.available_history_owners().is_none());
    for history in [
        &pool.repo.history_tree_id,
        &other.history_tree_id,
        "missing",
    ] {
        assert_history_commit_records(&pool.engine, history);
    }
    assert!(
        pool.engine.history_catalog.is_none(),
        "read must not create an owner catalog"
    );
    pool.engine.history_catalog = catalog;
    assert!(pool.engine.available_history_owners().is_some());
    assert_history_commit_records(&pool.engine, &pool.repo.history_tree_id);
}

#[test]
fn history_commit_records_reject_stale_owner_binding_after_decoded_state_replacement() {
    let mut pool = Pool::new();
    pool.save("file.txt", b"body", "commit");
    let history = pool.repo.history_tree_id.clone();
    assert!(pool.engine.available_history_owners().is_some());
    let captured = pool.engine.history_commit_records(&history);
    let original = std::sync::Arc::clone(&pool.engine.state);
    // A decoded/replaced allocation must not use the previous publication's
    // owner membership, including after commit ownership changes.
    let mut decoded: State =
        json_codec::from_value(json_codec::to_value(pool.engine.state()).unwrap()).unwrap();
    let moved = captured[0].commit_id.clone();
    decoded.commits.get_mut(&moved).unwrap().history_tree_id = "another-history".into();
    decoded
        .branches
        .get_mut(&pool.repo.main_branch_id)
        .unwrap()
        .state = BranchState::Migrated;
    pool.engine.state = std::sync::Arc::new(decoded);
    assert!(pool.engine.available_history_owners().is_none());
    for history in [history.as_str(), "another-history", "missing"] {
        assert_history_commit_records(&pool.engine, history);
    }
    assert!(
        pool.engine.available_history_owners().is_none(),
        "fallback must not build an index"
    );
    assert_eq!(captured[0].history_tree_id, history);
    pool.engine.state = original;
    assert!(pool.engine.available_history_owners().is_some());
    assert_history_commit_records(&pool.engine, &history);
}

#[test]
fn observed_version_and_merge_plans_stop_membership_scans_and_preserve_errors() {
    use std::cell::Cell;
    let g = graph();
    let scope = g.pool.scope(&g.file);
    let calls = Cell::new(0);
    let observe = || {
        calls.set(calls.get() + 1);
        Err(
            Error::new(storage_contract::ErrorCode::StateInvalid, "observer source").context(
                storage_contract::ErrorCode::RequestTimeout,
                "plan scan cancelled",
            ),
        )
    };
    let error = scope
        .prepare_version_page(None, 2)
        .unwrap()
        .read_observed(&observe)
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::RequestTimeout);
    assert_eq!(error.message(), "plan scan cancelled");
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(calls.get(), 1);
    calls.set(0);
    let error = scope
        .prepare_merge_page(None, 2)
        .unwrap()
        .read_observed(&observe)
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::RequestTimeout);
    assert_eq!(error.message(), "plan scan cancelled");
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(calls.get(), 1);
    let proceed = || {
        calls.set(calls.get() + 1);
        Ok(())
    };
    calls.set(0);
    let observed = scope
        .prepare_version_page(None, 2)
        .unwrap()
        .read_observed(&proceed)
        .unwrap();
    assert_eq!(
        observed,
        scope.prepare_version_page(None, 2).unwrap().read()
    );
    assert_eq!(calls.get(), scope.select_versions(None, 1000).keys.len());
    calls.set(0);
    let observed = scope
        .prepare_merge_page(None, 2)
        .unwrap()
        .read_observed(&proceed)
        .unwrap();
    assert_eq!(observed, scope.prepare_merge_page(None, 2).unwrap().read());
    assert_eq!(calls.get(), scope.file_rows().unwrap().merges.len());
}
