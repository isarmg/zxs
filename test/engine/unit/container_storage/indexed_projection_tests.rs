//! Exact descriptor bytes and physical projection are compared with full scans.
use super::*;
use crate::state_projection::{DerivedOwners, ProjectionTransition};
use json_codec::Value;

fn id(prefix: &str, number: u64) -> String {
    format!("{prefix}_{number:032x}")
}

fn body_version(number: u64, history: &str, file: &str, bytes: &[u8]) -> FileVersion {
    let hash = blake3::hash(bytes);
    let checksums = ChecksumManifest {
        logical_size: bytes.len() as u64,
        chunk_bytes: 262_144,
        chunks: bytes
            .chunks(262_144)
            .map(|chunk| *blake3::hash(chunk).as_bytes())
            .collect(),
    }
    .encode()
    .unwrap();
    FileVersion {
        history_tree_id: history.into(),
        version_id: id("ver", number),
        file_id: file.into(),
        creation_primary_slot_id: "P01".into(),
        logical_size: bytes.len() as u64,
        version_sequence: number,
        content_hash: ContentHash::from_digest(hash),
        checksum_chunk_bytes: 262_144,
        checksum_manifest_hash: blake3::hash(&checksums).to_hex().to_string(),
        creation_name: "seed.txt".into(),
        // Deliberately reverse sequence order relative to version-ID order.
        storage_name: format!(
            "v{:06}_20261027T000000Z_{}.txt",
            100 - number,
            &hash.to_hex()[..8]
        ),
        created_at: "2026-10-27T00:00:00Z".into(),
        source_version_ids: Vec::new(),
        created_as_full_checkpoint: true,
    }
}

fn branch(number: u64, history: &str, file: Option<&str>, version: &str) -> Branch {
    Branch {
        branch_id: id("br", number),
        history_tree_id: history.into(),
        name: format!("branch-{number}"),
        head_commit_id: id("commit", number),
        fork_commit_id: id("commit", number),
        branch_kind: if file.is_some() {
            BranchKind::File
        } else {
            BranchKind::Full
        },
        file_id: file.map(str::to_owned),
        file_node_id: Some(version.into()),
        pending_file_versions: BTreeMap::new(),
        generation: 1,
        state: BranchState::Active,
    }
}

fn full_descriptor(
    state: &State,
    file: &str,
    history: &str,
    slot: &str,
    current_name: &str,
) -> Vec<u8> {
    let backup = state
        .file_backup_paths
        .get(file)
        .and_then(|paths| paths.get(slot));
    let location = backup.or_else(|| state.file_locations.get(file));
    let name = backup
        .and_then(|path| path.rsplit('/').next())
        .or_else(|| state.file_names.get(file).map(String::as_str))
        .unwrap_or(current_name);
    let mut versions = state.versions.values()
        .filter(|version| version.file_id == file && version.history_tree_id == history)
        .map(|version| json!({
            "version_id":version.version_id,
            "parent_node_ids":crate::business::file_nodes::node_parents(state, &version.version_id),
            "tags":crate::business::file_nodes::node_tags(state, &version.version_id),
            "storage_name":version.storage_name, "created_at":version.created_at,
            "size":version.logical_size.to_string(), "content_hash":version.content_hash,
            "note":state.version_notes.get(&version.version_id).map(String::as_str).unwrap_or(""),
            "content_state":state.content_states.get(&version.version_id)
        })).collect::<Vec<_>>();
    versions.sort_by(|left, right| {
        left["storage_name"]
            .as_str()
            .cmp(&right["storage_name"].as_str())
            .then_with(|| {
                left["version_id"]
                    .as_str()
                    .cmp(&right["version_id"].as_str())
            })
    });
    let inherited = state
        .branches
        .values()
        .filter(|branch| branch.history_tree_id == history)
        .filter_map(|branch| {
            branch
                .pending_file_versions
                .get(file)
                .map(|source| (branch, source))
        })
        .map(|(branch, source)| {
            json!({
                "branch_id":branch.branch_id, "name":branch.name, "state":branch.state,
                "source_node_id":branch.file_node_id.as_ref().unwrap_or(source),
                "source_version_id":source, "version_id":source, "own_version_id":null,
                "inherited":true, "materialized":false
            })
        })
        .collect::<Vec<_>>();
    let contexts = state
        .file_contexts
        .iter()
        .filter(|(_, context)| {
            state
                .branches
                .get(&context.branch_id)
                .and_then(|branch| branch.file_id.as_deref())
                == Some(file)
        })
        .collect::<BTreeMap<_, _>>();
    let aliases = state
        .file_branch_aliases
        .iter()
        .filter(|(source, _)| {
            state
                .branches
                .get(*source)
                .and_then(|branch| branch.file_id.as_deref())
                == Some(file)
        })
        .collect::<BTreeMap<_, _>>();
    let mut nodes = state
        .file_path_nodes
        .values()
        .filter(|node| node.file_id == file && node.history_tree_id == history)
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then_with(|| left.node_id.cmp(&right.node_id))
    });
    let path_nodes = nodes
        .into_iter()
        .map(|node| {
            json!({
                "node":node, "tags":crate::business::file_nodes::node_tags(state, &node.node_id),
                "note":state.file_path_notes.get(&node.node_id).map(String::as_str).unwrap_or("")
            })
        })
        .collect::<Vec<_>>();
    let metadata_nodes = state.file_metadata_nodes.values()
        .filter(|node| node.file_id == file && node.history_tree_id == history)
        .map(|node| json!({"node":node, "note":state.file_path_notes.get(&node.node_id).map(String::as_str).unwrap_or("")}))
        .collect::<Vec<_>>();
    let merges = state
        .file_branch_merges
        .values()
        .filter(|event| event.file_id == file && event.history_tree_id == history)
        .collect::<Vec<_>>();
    canonical_json(&json!({
        "cache_only":true, "file_id":file, "history_tree_id":history, "name":name,
        "location":location, "primary_location":state.file_locations.get(file),
        "backup_policy_id":state.file_backup_policies.get(file), "metadata_nodes":metadata_nodes,
        "branch_merges":merges, "branch_aliases":aliases,
        "note":state.file_notes.get(file).map(String::as_str).unwrap_or(""),
        "path_contexts":contexts, "path_nodes":path_nodes,
        "versions":versions, "inherited_branches":inherited
    }))
    .unwrap()
}

fn full_locations(before: &State, after: &State, file: &str) -> BTreeSet<(String, String)> {
    let mut expected = BTreeSet::new();
    for state in [before, after] {
        for version in state
            .versions
            .values()
            .filter(|version| version.file_id == file)
        {
            for slot in &state.placements[&version.version_id].route_slot_ids {
                expected.insert((slot.clone(), version.history_tree_id.clone()));
            }
        }
    }
    expected
}

type DescriptorBytes = BTreeMap<(String, String, String), Vec<u8>>;

fn staged_bytes(prepared: &mut PreparedNames) -> DescriptorBytes {
    let mut bytes = BTreeMap::new();
    if let Some(cache) = &mut prepared.cache {
        for item in &cache.items {
            cache
                .file
                .seek(std::io::SeekFrom::Start(item.offset))
                .unwrap();
            let mut content = vec![0; item.length];
            cache.file.read_exact(&mut content).unwrap();
            assert!(
                bytes
                    .insert(
                        (
                            item.disk.seal.slot_id.clone(),
                            item.history
                                .file_name()
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .to_owned(),
                            item.file.clone()
                        ),
                        content
                    )
                    .is_none()
            );
        }
    }
    bytes
}

struct Fixture {
    _temporary: tempfile::TempDir,
    config: ValidatedConfig,
    state: State,
    file: String,
    bodies: Vec<(PathBuf, FileVersion, u64, Vec<u8>)>,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let config = ValidatedConfig::try_from(Config::development([
            temporary.path().join("p"),
            temporary.path().join("b"),
            temporary.path().join("c"),
        ]))
        .unwrap();
        drop(
            Engine::initialize_validated(&temporary.path().join("system"), config.clone()).unwrap(),
        );
        let file = id("file", 1);
        let other = id("file", 2);
        let h1 = id("history", 1);
        let h2 = id("history", 2);
        let mut state = State::default();
        for (number, history, owner, slots, content_state) in [
            (1, &h1, &file, vec!["P01", "B01"], ContentState::Available),
            (2, &h1, &file, vec!["B01", "C01"], ContentState::Purged),
            (3, &h2, &file, vec!["C01"], ContentState::Migrated),
            (4, &h1, &other, vec!["P01"], ContentState::Available),
        ] {
            let version = body_version(number, history, owner, b"independent physical body");
            state.placements.insert(
                version.version_id.clone(),
                Placement {
                    version_id: version.version_id.clone(),
                    primary_slot_id: slots[0].into(),
                    target_copies: slots.len() as u32,
                    route_slot_ids: slots.into_iter().map(str::to_owned).collect(),
                    topology_generation: 1,
                    binding_generations: BTreeMap::new(),
                },
            );
            state
                .content_states
                .insert(version.version_id.clone(), content_state);
            state.versions.insert(version.version_id.clone(), version);
        }
        state.file_names.insert(file.clone(), "current.txt".into());
        state
            .file_locations
            .insert(file.clone(), "primary/current.txt".into());
        state.file_backup_paths.insert(
            file.clone(),
            BTreeMap::from([
                ("B01".into(), "warm/current.txt".into()),
                ("C01".into(), "archive/kept.txt".into()),
            ]),
        );
        state
            .file_backup_policies
            .insert(file.clone(), id("policy", 1));
        state
            .file_notes
            .insert(file.clone(), "original file note".into());
        state
            .version_notes
            .insert(id("ver", 2), "second version note".into());
        let mut root = branch(1, &h1, None, &id("ver", 1));
        root.pending_file_versions
            .insert(file.clone(), id("ver", 1));
        state.branches.insert(root.branch_id.clone(), root);
        let mut file_branch = branch(2, &h2, Some(&file), &id("ver", 3));
        file_branch
            .pending_file_versions
            .insert(file.clone(), id("ver", 3));
        state
            .branches
            .insert(file_branch.branch_id.clone(), file_branch);
        let target = branch(3, &h1, Some(&other), &id("ver", 4));
        state.branches.insert(target.branch_id.clone(), target);
        // Source ownership and file-global contexts are independent of the
        // current container history. These are projection-only relation fixtures.
        state.file_branch_aliases.insert(id("br", 2), id("br", 3));
        state.file_contexts.insert(
            "repository/old-alias".into(),
            FileContext {
                node_id: Some(id("ver", 3)),
                selection_generation: 8,
                branch_id: id("br", 2),
                inherited_from: Some(id("br", 1)),
            },
        );
        let metadata = FileMetadataNode {
            node_id: id("fmeta", 1),
            history_tree_id: h1.clone(),
            file_id: file.clone(),
            version_id: id("ver", 1),
            parent_node_ids: vec![id("ver", 1)],
            branch_id: id("br", 1),
            kind: FileMetadataKind::Tags,
            tags: BTreeSet::from(["blue".into(), "重要".into()]),
            operation_id: OperationId::generate(),
            author: "author".into(),
            sequence: 10,
            created_at: 10,
        };
        state
            .file_path_notes
            .insert(metadata.node_id.clone(), "metadata note".into());
        state
            .file_metadata_nodes
            .insert(metadata.node_id.clone(), metadata);
        for (number, sequence) in [(1, 30), (2, 20)] {
            let node = FilePathNode {
                node_id: id("fpath", number),
                history_tree_id: h1.clone(),
                file_id: file.clone(),
                version_id: id("ver", 1),
                parent_node_ids: vec![id("fmeta", 1)],
                branch_id: id("br", 1),
                path: format!("new-{number}.txt"),
                previous_path: "seed.txt".into(),
                operation_id: OperationId::generate(),
                author: "author".into(),
                sequence,
                created_at: sequence,
            };
            state
                .file_path_notes
                .insert(node.node_id.clone(), format!("path note {number}"));
            state.file_path_nodes.insert(node.node_id.clone(), node);
        }
        state
            .file_node_parents
            .insert(id("ver", 2), vec![id("fpath", 2)]);
        for number in [2, 1] {
            let event = FileBranchMerge {
                merge_id: id("fmerge", number),
                history_tree_id: h1.clone(),
                file_id: file.clone(),
                input_branch_ids: vec![id("br", 1), id("br", 2)],
                input_node_ids: vec![id("ver", 1), id("ver", 2)],
                chosen_node_id: id("ver", 2),
                result_node_id: id("ver", 2),
                surviving_branch_id: id("br", 1),
                reused_node: true,
                parent_node_ids: vec![id("ver", 1), id("fpath", number)],
                operation_id: OperationId::generate(),
                sequence: number,
            };
            state
                .file_branch_merges
                .insert(event.merge_id.clone(), event);
        }
        let mut bodies = Vec::new();
        for version in state.versions.values() {
            for slot in &state.placements[&version.version_id].route_slot_ids {
                let disk = config
                    .disks
                    .iter()
                    .find(|disk| &disk.slot_id == slot)
                    .unwrap();
                let directory = disk
                    .root
                    .join("histories")
                    .join(&version.history_tree_id)
                    .join("versions")
                    .join(&version.version_id);
                private_dir(&directory.join("content")).unwrap();
                let body = b"independent physical body";
                write_atomic(
                    &directory.join("content").join(&version.creation_name),
                    body,
                    false,
                )
                .unwrap();
                let checksums = ChecksumManifest {
                    logical_size: body.len() as u64,
                    chunk_bytes: version.checksum_chunk_bytes,
                    chunks: vec![*blake3::hash(body).as_bytes()],
                }
                .encode()
                .unwrap();
                write_atomic(&directory.join("checksums.bin"), &checksums, false).unwrap();
                let installed = install_version_content(
                    &directory,
                    PreparedContent::Verified(VerifiedExisting::read(&directory, version).unwrap()),
                )
                .unwrap();
                bodies.push((
                    directory,
                    version.clone(),
                    fs::metadata(installed).unwrap().ino(),
                    body.to_vec(),
                ));
            }
        }
        Self {
            _temporary: temporary,
            config,
            state,
            file,
            bodies,
        }
    }

    fn expected(&self, before: &State, after: &State) -> DescriptorBytes {
        full_locations(before, after, &self.file)
            .into_iter()
            .filter_map(|(slot, history)| {
                let disk = self
                    .config
                    .disks
                    .iter()
                    .find(|disk| disk.slot_id == slot)
                    .unwrap();
                let current =
                    container(&disk.root.join("histories").join(&history), &self.file).unwrap()?;
                let name = current.file_name().unwrap().to_str().unwrap();
                let bytes = full_descriptor(after, &self.file, &history, &slot, name);
                Some(((slot, history, self.file.clone()), bytes))
            })
            .collect()
    }

    fn assert_bodies_unchanged(&self) {
        for (directory, version, inode, bytes) in &self.bodies {
            let path = content_path(directory, version).unwrap();
            assert_eq!(fs::metadata(&path).unwrap().ino(), *inode);
            assert_eq!(fs::metadata(&path).unwrap().nlink(), 1);
            assert_eq!(fs::read(path).unwrap(), *bytes);
        }
    }
}

#[test]
fn warm_and_cold_container_bytes_match_full_scans_before_and_after_physical_apply() {
    let fixture = Fixture::new();
    let resources = Arc::new(DiskResources::open(&fixture.config, true).unwrap());
    let before = &fixture.state;
    let mut after = before.clone();
    after.begin_tracking();
    after
        .file_notes
        .insert(fixture.file.clone(), "changed file note".into());
    let owners =
        DerivedOwners::from_state(before, &crate::namespace::fixture_store::FixtureSource).unwrap();
    let transition = ProjectionTransition::new(
        before,
        &after,
        Some(&owners),
        &crate::namespace::fixture_store::FixtureSource,
    )
    .unwrap();
    validate_new_projection_paths(&transition).unwrap();
    assert_eq!(
        projection_locations(transition.previous(), transition.next(), &fixture.file),
        full_locations(before, &after, &fixture.file)
    );
    let expected = fixture.expected(before, &after);
    let mut prepared =
        prepare_names(&fixture.config, &resources, &transition, MAX_METADATA).unwrap();
    assert_eq!(staged_bytes(&mut prepared), expected);
    fixture.assert_bodies_unchanged();
    prepared.apply().unwrap();
    for ((slot, history, file), bytes) in &expected {
        let disk = fixture
            .config
            .disks
            .iter()
            .find(|disk| &disk.slot_id == slot)
            .unwrap();
        let current = container(&disk.root.join("histories").join(history), file)
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(current.join("container.json")).unwrap(), *bytes);
    }
    fixture.assert_bodies_unchanged();
    let cold = ProjectionTransition::new(
        before,
        &after,
        None,
        &crate::namespace::fixture_store::FixtureSource,
    )
    .unwrap();
    let mut replay = prepare_names(&fixture.config, &resources, &cold, MAX_METADATA).unwrap();
    assert_eq!(staged_bytes(&mut replay), expected);
    replay.apply().unwrap();
    fixture.assert_bodies_unchanged();
}

#[test]
fn deleted_migrated_version_retains_old_only_history_route_without_recreating_content() {
    let fixture = Fixture::new();
    let resources = Arc::new(DiskResources::open(&fixture.config, true).unwrap());
    let mut after = fixture.state.clone();
    after.begin_tracking();
    after.versions.remove(&id("ver", 3));
    after.placements.remove(&id("ver", 3));
    after.content_states.remove(&id("ver", 3));
    let owners = DerivedOwners::from_state(
        &fixture.state,
        &crate::namespace::fixture_store::FixtureSource,
    )
    .unwrap();
    let transition = ProjectionTransition::new(
        &fixture.state,
        &after,
        Some(&owners),
        &crate::namespace::fixture_store::FixtureSource,
    )
    .unwrap();
    let expected = fixture.expected(&fixture.state, &after);
    let old_history = ("C01".into(), id("history", 2), fixture.file.clone());
    assert!(expected.contains_key(&old_history));
    let mut prepared =
        prepare_names(&fixture.config, &resources, &transition, MAX_METADATA).unwrap();
    assert_eq!(staged_bytes(&mut prepared), expected);
    let old: Value = json_codec::from_slice(&expected[&old_history]).unwrap();
    assert_eq!(old["versions"], json!([]));
    assert!(!old["path_contexts"].as_object().unwrap().is_empty());
    assert!(!old["branch_aliases"].as_object().unwrap().is_empty());
    prepared.apply().unwrap();
    fixture.assert_bodies_unchanged();
}

#[test]
fn per_object_budget_failure_leaves_physical_names_and_bodies_unmodified() {
    let fixture = Fixture::new();
    let resources = Arc::new(DiskResources::open(&fixture.config, true).unwrap());
    let mut after = fixture.state.clone();
    after.begin_tracking();
    after
        .file_notes
        .insert(fixture.file.clone(), "x".repeat(4096));
    let owners = DerivedOwners::from_state(
        &fixture.state,
        &crate::namespace::fixture_store::FixtureSource,
    )
    .unwrap();
    let transition = ProjectionTransition::new(
        &fixture.state,
        &after,
        Some(&owners),
        &crate::namespace::fixture_store::FixtureSource,
    )
    .unwrap();
    let mut names = Vec::new();
    for disk in &fixture.config.disks {
        for history in [id("history", 1), id("history", 2)] {
            if let Some(name) =
                container(&disk.root.join("histories").join(history), &fixture.file).unwrap()
            {
                names.push(name);
            }
        }
    }
    assert_eq!(
        prepare_names(&fixture.config, &resources, &transition, 512)
            .err()
            .unwrap()
            .code(),
        ErrorCode::ResourceLimit
    );
    for name in names {
        assert!(name.is_dir());
        assert!(!name.join("container.json").exists());
    }
    fixture.assert_bodies_unchanged();
}
