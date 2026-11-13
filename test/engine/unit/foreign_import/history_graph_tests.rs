use super::super::catalog::{Catalog, Domain};
use super::*;

fn names(state: &storage_contract::State) -> BTreeMap<Domain, BTreeMap<String, String>> {
    let catalog = Catalog::capture(state).unwrap();
    let mut occupied = catalog.all_objects();
    catalog
        .objects
        .iter()
        .map(|(domain, ids)| {
            (
                *domain,
                assign(
                    "graph-test",
                    domain.prefix(),
                    ids.iter().map(String::as_str),
                    &mut occupied,
                )
                .unwrap(),
            )
        })
        .collect()
}

#[test]
fn full_graph_identity_remap_is_lossless_and_keeps_source_audit_domains() {
    let fixture = crate::foreign_import::tests::fixture();
    let source = crate::foreign_import::tests::inspect(&fixture.source).unwrap();
    let names = names(fixture.engine.state());
    let reverse = names
        .iter()
        .map(|(domain, ids)| {
            (
                *domain,
                ids.iter()
                    .map(|(old, new)| (new.clone(), old.clone()))
                    .collect(),
            )
        })
        .collect();
    let mapper = remap::Remap::new(&names);
    let back = remap::Remap::new(&reverse);
    for original in source.histories() {
        assert!(original.manifest().path_audit.is_empty());
        let mapped = mapper.manifest(original.manifest()).unwrap();
        assert!(mapped.path_audit.is_empty());
        mapped.validate().unwrap();
        assert_ne!(
            mapped.history.history_tree_id,
            original.manifest().history.history_tree_id
        );
        assert_eq!(back.manifest(&mapped).unwrap(), *original.manifest());
        for (old, record) in original.manifest().commits.iter() {
            let new = &mapped.commits[&names[&Domain::Commit][old]];
            assert_eq!(new.operation_id, record.operation_id);
            assert_eq!(new.author, record.author);
            assert_eq!(new.sequence, record.sequence);
            assert_eq!(new.namespace_deletions, record.namespace_deletions);
            assert_eq!(new.provenance, record.provenance);
        }
    }
}
#[test]
fn incomplete_name_closure_is_rejected_without_mutating_original() {
    let fixture = crate::foreign_import::tests::fixture();
    let source = crate::foreign_import::tests::inspect(&fixture.source).unwrap();
    let original = source.histories()[0].manifest();
    let before = canonical_json(original).unwrap();
    let mut names = names(fixture.engine.state());
    names.remove(&Domain::Version);
    assert!(remap::Remap::new(&names).manifest(original).is_err());
    assert_eq!(canonical_json(original).unwrap(), before);
}
#[test]
fn directory_git_index_boundaries_children_and_generations_remap_without_flattening() {
    use storage_contract::directory_git::*;
    let definitions = [
        (Domain::Repository, "repo"),
        (Domain::Entry, "entry"),
        (Domain::DirectoryBranch, "dbr"),
        (Domain::DirectoryTree, "dtree"),
    ];
    let mut names = BTreeMap::new();
    for (domain, prefix) in definitions {
        names.insert(
            domain,
            BTreeMap::from([(
                format!("{prefix}_{:032x}", 1),
                format!("{prefix}_{:032x}", 2),
            )]),
        );
    }
    let old = |prefix: &str| format!("{prefix}_{:032x}", 1);
    let new = |prefix: &str| format!("{prefix}_{:032x}", 2);
    let mut value = DirectoryGitRepository {
        deletions: std::collections::BTreeMap::new(),
        repository_id: old("repo"),
        directory_id: old("entry"),
        initialized_sequence: 10,
        main_branch_id: old("dbr"),
        current_branch_id: old("dbr"),
        branches: BTreeMap::from([(
            old("dbr"),
            DirectoryGitBranch {
                head_tree_id: Some(old("dtree")),
                index_tree_id: old("dtree"),
                index_generation: 19,
                index_boundaries: BTreeMap::from([(old("entry"), "child".into())]),
                index_children: BTreeMap::from([(
                    old("entry"),
                    DirectoryGitChildReference {
                        directory_branch_id: old("dbr"),
                        tree_id: Some(old("dtree")),
                    },
                )]),
            },
        )]),
    };
    remap::Remap::new(&names)
        .directory_git_repository(&mut value)
        .unwrap();
    assert_eq!(value.repository_id, new("repo"));
    assert_eq!(value.directory_id, new("entry"));
    assert_eq!(value.initialized_sequence, 10);
    let branch = &value.branches[&new("dbr")];
    assert_eq!(branch.index_generation, 19);
    assert_eq!(branch.head_tree_id, Some(new("dtree")));
    assert_eq!(
        branch.index_children[&new("entry")].tree_id,
        Some(new("dtree"))
    );
    assert_eq!(branch.index_boundaries[&new("entry")], "child");
}
#[test]
fn graph_budget_counts_before_allocating_output() {
    let value = BTreeMap::from([("a", "body"), ("b", "more")]);
    let expected = canonical_json(&value).unwrap().len();
    assert_eq!(encoded_length(&value, expected).unwrap(), expected);
    assert_eq!(
        encoded_length(&value, expected - 1).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
}

#[test]
fn real_path_audit_rejects_foreign_remap_without_rewriting_source_evidence() {
    let mut fixture = crate::foreign_import::tests::fixture();
    let repository = fixture.engine.state().repositories.values().next().unwrap().clone();
    let receipt = fixture.engine.file_namespace(
        &repository.repository_id,
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename, "document.txt", Some("renamed.txt"),
        ).unwrap(),
        "admin",
        OperationId::generate().unwrap(),
    ).unwrap();
    let source = HistoryManifest::from_state(
        fixture.engine.state(), &repository.history_tree_id,
    ).unwrap();
    source.validate().unwrap();
    assert!(!source.path_audit.is_empty());
    let before = canonical_json(&source).unwrap();
    let state_before = canonical_json(fixture.engine.state()).unwrap();
    let names = names(fixture.engine.state());
    let names_before = names.clone();
    let error = remap::Remap::new(&names).manifest(&source).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnsupportedOperation);
    assert!(matches!(error, Error::PathChange(crate::PathChangeError::ForeignSource)));
    assert_eq!(canonical_json(&source).unwrap(), before);
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), state_before);
    assert_eq!(names, names_before);
    assert_eq!(fixture.engine.state().operations[&receipt.operation_id], receipt);
}

#[test]
fn restored_path_audit_is_not_treated_as_an_empty_foreign_graph() {
    use std::os::unix::fs::PermissionsExt;

    let mut fixture = crate::foreign_import::tests::fixture();
    std::fs::set_permissions(
        fixture._temp.path(), std::fs::Permissions::from_mode(0o700),
    ).unwrap();
    let repository = fixture.engine.state().repositories.values().next().unwrap().clone();
    let renamed = fixture.engine.file_namespace(
        &repository.repository_id,
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename, "document.txt", Some("renamed.txt"),
        ).unwrap(),
        "admin",
        OperationId::generate().unwrap(),
    ).unwrap();
    let package = fixture._temp.path().join("audit-package");
    fixture.engine.export_history_package(&repository.history_tree_id, &package).unwrap();
    let mut restored = Engine::initialize(
        &fixture._temp.path().join("restored-system"),
        Config::development([fixture._temp.path().join("restored-disk")]).unwrap(),
    ).unwrap();
    restored.restore_history_package(
        &package, "P01", 1, "restorer", OperationId::generate().unwrap(),
    ).unwrap();
    assert!(!restored.state().operations.contains_key(&renamed.operation_id));
    let source = HistoryManifest::from_state(
        restored.state(), &repository.history_tree_id,
    ).unwrap();
    source.validate().unwrap();
    assert!(!source.path_audit.is_empty());
    let source_before = canonical_json(&source).unwrap();
    let state_before = canonical_json(restored.state()).unwrap();
    let names = names(restored.state());
    let error = remap::Remap::new(&names).manifest(&source).unwrap_err();
    assert!(matches!(error, Error::PathChange(crate::PathChangeError::ForeignSource)));
    assert_eq!(canonical_json(&source).unwrap(), source_before);
    assert_eq!(canonical_json(restored.state()).unwrap(), state_before);
}
