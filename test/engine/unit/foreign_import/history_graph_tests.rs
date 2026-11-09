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
        let mapped = mapper.manifest(original.manifest()).unwrap();
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
        encoded_length(&value, expected - 1).unwrap_err().code,
        ErrorCode::ResourceLimit
    );
}
