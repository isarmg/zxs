use super::*;

#[test]
fn placement_preview_has_an_explicit_clean_repository_and_preserves_files() {
    let source = tempfile::tempdir().unwrap();
    crate::fixture::initialize_management(&source, "Placement fixture password 2026!").unwrap();
    let fixture = Fixture::new(source.path(), &source.path().join("pool")).unwrap();
    let mut engine = Engine::open(&fixture.pool).unwrap();
    let status = engine
        .directory_git_status(&fixture.repository.repository_id, "future")
        .unwrap();
    assert!(status.initialized && status.head_tree_id.is_some());
    assert!(!status.staged && !status.needs_restage);
    assert!(
        !engine
            .directory_git_status(&fixture.repository.repository_id, "protected")
            .unwrap()
            .initialized,
        "the independent backup policy must not implicitly initialize Git"
    );
    let path = engine
        .list_tree(&fixture.repository.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.entry_id == fixture.future_directory)
        .unwrap()
        .path
        .into();
    engine
        .create_directory_branches(
            &fixture.repository.repository_id,
            Some(&path),
            &storage_engine::BranchName::parse("placement-preview").unwrap(),
            PRINCIPAL,
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let branch = engine
        .state()
        .directory_branches
        .values()
        .find(|branch| branch.name == "placement-preview")
        .unwrap();
    assert_eq!(branch.directory_id, fixture.future_directory);
    let preview = engine
        .preview_directory_branch(
            &fixture.repository.repository_id,
            &branch.directory_branch_id,
            &branch.head_tree_id,
        )
        .unwrap();
    assert_eq!(preview.path(), "future");
    assert_eq!(fixture.originals().len(), 2);
    assert_eq!(engine.state().versions.len(), fixture.originals.len());
    fixture.preserved(&engine).unwrap();
}
