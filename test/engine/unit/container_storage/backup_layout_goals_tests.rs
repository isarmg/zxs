//! Source-only pure goal fixtures built from temporary Engine publications.
//! Direct Target values below test geometry, not native Backup authorization,
//! actual target filesystem moves, writer handoff or crash recovery.
use super::*;
use crate::{
    CommitRequest, Config, Engine, NamespaceOperation, NamespaceOperationKind, OperationId,
    directory_git::DirectoryGitInitializeBody,
};

struct Fixture {
    _temporary: tempfile::TempDir,
    engine: Engine,
    repository: storage_contract::Repository,
    prefix: Vec<String>,
    paths: BTreeMap<String, BTreeMap<String, String>>,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let mut engine = Engine::initialize(
            &temporary.path().join("system"),
            Config::development([temporary.path().join("primary")]).unwrap(),
        )
        .unwrap();
        let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
        save(&mut engine, &repository, "Box/file.txt", b"first");
        for path in ["", "Box"] {
            let head = engine.state().branches[&repository.main_branch_id]
                .head_commit_id
                .clone();
            engine
                .initialize_directory_git(
                    &repository.repository_id,
                    &DirectoryGitInitializeBody {
                        path: path.into(),
                        expected_namespace_head: head,
                        operation_id: op(),
                    },
                    "admin",
                )
                .unwrap();
        }
        engine
            .reconcile_directory_layout(&repository.repository_id)
            .unwrap();
        let paths = engine
            .state()
            .versions
            .values()
            .map(|version| {
                (
                    version.file_id.clone(),
                    BTreeMap::from([("P02".into(), "Upstream/Original/Box/file.txt".into())]),
                )
            })
            .collect();
        Self {
            _temporary: temporary,
            engine,
            repository,
            prefix: vec!["Upstream".into(), "Original".into()],
            paths,
        }
    }
    fn state(&self) -> State {
        let mut state = self.engine.state().clone();
        for placement in state.placements.values_mut() {
            placement.route_slot_ids.push("P02".into());
            placement.binding_generations.insert("P02".into(), 1);
            placement.target_copies += 1;
        }
        state.file_backup_paths = self.paths.clone().into();
        state
    }
    fn goals(&self, state: &State) -> LayoutGoals {
        super::super::build_with_targets(
            state,
            None,
            &[Target {
                slot: "P02",
                prefix: &self.prefix,
                generation: 1,
            }],
        )
        .unwrap()
    }
    fn target_path(&self, goals: &LayoutGoals, file: &str) -> String {
        let history = &goals.histories[&("P02".into(), self.repository.history_tree_id.clone())];
        let goal = &history.files[file];
        let FileAddress::Directory {
            directory_id,
            relative_path,
        } = &goal.to
        else {
            panic!("selected Backup file lost its managed forest");
        };
        format!(
            "{}/{}",
            history.directories[directory_id].to.as_str(),
            relative_path.as_str()
        )
    }
}
fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn save(engine: &mut Engine, repository: &storage_contract::Repository, path: &str, bytes: &[u8]) {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: op(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Backup goal fixture".into(),
            request_digest: None,
        })
        .unwrap();
}

#[test]
fn selected_backup_has_exact_first_installed_forest_geometry() {
    let fixture = Fixture::new();
    let state = fixture.state();
    let goals = fixture.goals(&state);
    let primary = &goals.histories[&("P01".into(), fixture.repository.history_tree_id.clone())];
    let target = &goals.histories[&("P02".into(), fixture.repository.history_tree_id.clone())];
    assert_eq!(primary.directories.len(), target.directories.len());
    assert_eq!(primary.files.len(), target.files.len());
    for (id, original) in &primary.directories {
        assert_eq!(
            target.directories[id].to,
            crate::container_storage::backup_ordinary::target_directory(
                &original.to,
                &fixture.prefix
            )
            .unwrap()
        );
        assert_eq!(target.directories[id].parent, original.parent);
    }
    assert_eq!(state.file_backup_paths, fixture.paths);
}

#[test]
fn directory_rename_preserves_target_forest_and_original_backup_path_metadata() {
    let mut fixture = Fixture::new();
    let before = fixture.state();
    let file = before.versions.values().next().unwrap().file_id.clone();
    fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "Box", Some("Renamed"))
                .unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let next = fixture.state();
    let goals = fixture.goals(&next);
    assert_eq!(
        fixture.target_path(&goals, &file),
        format!(
            "directory-repositories/{}/Upstream/Original/Renamed/file.txt",
            fixture.repository.repository_id
        )
    );
    assert_eq!(next.file_backup_paths, before.file_backup_paths);
    assert!(
        !goals.histories[&("P02".into(), fixture.repository.history_tree_id.clone())]
            .files
            .is_empty()
    );
}

#[test]
fn later_commit_keeps_all_routed_versions_in_one_target_container_goal() {
    let mut fixture = Fixture::new();
    let first = fixture.state();
    let file = first.versions.values().next().unwrap().file_id.clone();
    save(
        &mut fixture.engine,
        &fixture.repository,
        "Box/file.txt",
        b"second",
    );
    let next = fixture.state();
    let goals = fixture.goals(&next);
    let target = &goals.histories[&("P02".into(), fixture.repository.history_tree_id.clone())];
    let expected = next
        .versions
        .values()
        .filter(|version| version.file_id == file)
        .map(|version| version.version_id.clone())
        .collect::<BTreeSet<_>>();
    assert!(expected.len() >= 2);
    assert_eq!(target.files[&file].version_ids, expected);
    assert_eq!(
        fixture.target_path(&goals, &file),
        format!(
            "directory-repositories/{}/Upstream/Original/Box/file.txt",
            fixture.repository.repository_id
        )
    );
}

#[test]
fn selected_backup_layout_rejects_a_rebound_route() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state
        .placements
        .values_mut()
        .next()
        .unwrap()
        .binding_generations
        .insert("P02".into(), 2);
    assert!(
        super::super::build_with_targets(
            &state,
            None,
            &[Target {
                slot: "P02",
                prefix: &fixture.prefix,
                generation: 1
            }]
        )
        .is_err()
    );
}

#[test]
fn ordinary_genesis_config_never_infers_target_layout_from_a_route_or_path() {
    let fixture = Fixture::new();
    let state = fixture.state();
    let goals = super::super::build_for_config(&fixture.engine.config, &state, None).unwrap();
    assert!(!goals.histories.keys().any(|(slot, _)| slot == "P02"));
}
