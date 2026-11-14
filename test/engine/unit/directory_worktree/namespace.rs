use super::*;
use crate::directory_worktree::projection::{
    ContainerPresence, ContainerPublication, FileEdit, FileResult, ProjectionError,
    PublishedFileSource, ReadRoute, Transfer, WorkingFile,
};
use crate::working_tree::snapshot::{AdmittedSnapshotManifest, SnapshotFile};
use platform_io::fs_safety::EntryName;
use storage_contract::working_tree::{
    LocalEntityIdentity, SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::{BranchId, ContentHash, DiskId, FileId, FileNodeId, OperationId, VersionId};

fn path(value: &str) -> PortablePath { PortablePath::parse(value).unwrap() }
fn lookup(value: &str) -> LookupPath { LookupPath::parse(value).unwrap() }
fn disk() -> DiskId { DiskId::from_random([80; 16]) }
fn file(id: u8, body: &[u8]) -> SnapshotFile {
    SnapshotFile {
        file_id: FileId::from_random([id; 16]),
        version_id: VersionId::from_random([id; 16]),
        size: body.len() as u64,
        content_hash: ContentHash::from_digest(blake3::hash(body)),
    }
}
fn content(material: SnapshotFile) -> SnapshotEntryContent {
    SnapshotEntryContent::File {
        material,
        source: SnapshotFileSource::Container {
            branch: BranchId::from_random([70; 16]),
            node: FileNodeId::Version(material.version_id),
        },
    }
}
fn manifest(node: u8, entries: Vec<SnapshotEntry>) -> AdmittedSnapshotManifest {
    AdmittedSnapshotManifest::prepare(
        SnapshotRepositoryId::from_random([1; 16]),
        SnapshotNodeId::from_random([node; 16]),
        OperationId::from_random([node; 16]), entries,
    ).unwrap()
}
fn source() -> AdmittedSnapshotManifest {
    manifest(2, vec![
        SnapshotEntry { path: path("A.txt"), content: content(file(3, b"A")) },
        SnapshotEntry { path: path("Dir"), content: SnapshotEntryContent::Directory {} },
        SnapshotEntry { path: path("Dir/B.txt"), content: content(file(4, b"B")) },
        SnapshotEntry { path: path("Dir/Empty"), content: SnapshotEntryContent::Directory {} },
        SnapshotEntry { path: path("Directory.txt"), content: content(file(5, b"C")) },
    ])
}
fn edit() -> FileEdit {
    let mut material = file(4, b"edited B");
    material.version_id = VersionId::from_random([6; 16]);
    FileEdit {
        path: path("Dir/B.txt"),
        material: WorkingFile {
            file: material,
            leaf: EntryName::new("0123456789abcde-B.txt").unwrap(),
            entity: LocalEntityIdentity::new(10, 6),
        },
    }
}
fn project<'a>(source: &'a AdmittedSnapshotManifest, changes: &[NamespaceChange])
    -> Result<SparseProjection<'a>, NamespaceError>
{
    SparseProjection::admit(source, vec![edit()]).unwrap().namespace(changes)
}

#[test]
fn reads_listing_and_transfer_plan_share_renamed_paths_and_original_backings() {
    let source = source();
    let changed = edit();
    let view = project(&source, &[
        NamespaceChange::AddDirectory { path: path("Target") },
        NamespaceChange::Rename { source: lookup("a.TXT"), target: path("Target/Renamed.txt") },
        NamespaceChange::Rename { source: lookup("dir"), target: path("Target/Sub") },
    ]).unwrap();
    assert_eq!(view.route(&lookup("target/renamed.TXT")),
        Some(ReadRoute::Original { file: file(3, b"A") }));
    assert_eq!(view.route(&lookup("Target/Sub/B.txt")),
        Some(ReadRoute::Working { material: &changed.material }));
    assert_eq!(view.route(&lookup("Target/Sub/Empty")), Some(ReadRoute::Directory));
    assert!(view.route(&lookup("Dir/B.txt")).is_none());
    let mut located = Vec::new();
    let plan = view.plan(disk(), |selected_disk, file| {
        located.push((selected_disk, file));
        Ok::<_, ()>(ContainerPresence::Present {
            disk: selected_disk, entity: LocalEntityIdentity::new(10, 20),
        })
    }).unwrap();
    assert_eq!(located, [(disk(), changed.material.file.file_id)]);
    assert_eq!(plan.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(),
        view.paths().map(PortablePath::as_str).collect::<Vec<_>>());
    assert!(matches!(plan[4].transfer, Transfer::MoveWorking {
        material, container: ContainerPublication::Update { source_version, source_branch, .. },
    } if material == &changed.material
        && source_version == VersionId::from_random([4; 16])
        && source_branch == BranchId::from_random([70; 16])));
    assert_eq!(source.entries()[2].path, path("Dir/B.txt"));
}

#[test]
fn case_only_subtree_rename_updates_descendants_and_keeps_empty_directories() {
    let source = source();
    let view = project(&source, &[NamespaceChange::Rename {
        source: lookup("DIR"), target: path("dir"),
    }]).unwrap();
    assert_eq!(view.paths().map(PortablePath::as_str).collect::<Vec<_>>(),
        ["A.txt", "dir", "dir/B.txt", "dir/Empty", "Directory.txt"]);
    assert_eq!(view.route(&lookup("Dir/Empty")), Some(ReadRoute::Directory));
}

#[test]
fn removal_excludes_descendants_from_plan_but_does_not_discard_sparse_bodies() {
    let source = source();
    let view = project(&source, &[NamespaceChange::RemoveSubtree { path: lookup("DIR") }]).unwrap();
    assert_eq!(view.paths().map(PortablePath::as_str).collect::<Vec<_>>(),
        ["A.txt", "Directory.txt"]);
    assert_eq!(view.edits.values().next().unwrap(), &edit().material);
    let plan = view.plan(disk(), |_, _| -> Result<ContainerPresence, ()> {
        panic!("a removed sparse occurrence must not be published by a directory plan")
    }).unwrap();
    assert_eq!(plan.len(), 2);
    assert!(plan.iter().all(|entry| matches!(entry.transfer, Transfer::CopyOriginal { .. })));
    let published = manifest(9, vec![source.entries()[0].clone(), source.entries()[4].clone()]);
    assert_eq!(view.published_view(&published, &[]).unwrap(), published.reference());
}

#[test]
fn publication_checks_exact_changed_namespace_and_renamed_file_result_paths() {
    let source = source();
    let changed = edit();
    let view = project(&source, &[
        NamespaceChange::RemoveSubtree { path: lookup("A.txt") },
        NamespaceChange::AddDirectory { path: path("Fresh") },
        NamespaceChange::Rename { source: lookup("Dir/B.txt"), target: path("Fresh/Saved.txt") },
    ]).unwrap();
    let published = manifest(9, vec![
        source.entries()[1].clone(), source.entries()[3].clone(), source.entries()[4].clone(),
        SnapshotEntry { path: path("Fresh"), content: SnapshotEntryContent::Directory {} },
        SnapshotEntry { path: path("Fresh/Saved.txt"), content: content(changed.material.file) },
    ]);
    let sources = [PublishedFileSource {
        path: path("Fresh/Saved.txt"),
        source: SnapshotFileSource::Container {
            branch: BranchId::from_random([70; 16]),
            node: FileNodeId::Version(changed.material.file.version_id),
        },
    }];
    assert_eq!(view.published_view(&published, &sources).unwrap(), published.reference());
    let wrong_sources = [PublishedFileSource {
        path: changed.path, source: sources[0].source,
    }];
    assert!(matches!(view.published_view(&published, &wrong_sources),
        Err(ProjectionError::PublishedView)));
    let unchanged = manifest(10, source.entries().to_vec());
    assert!(matches!(view.published_view(&unchanged, &sources),
        Err(ProjectionError::PublishedView)));
}

#[test]
fn saved_file_baseline_survives_rename_without_another_file_publication() {
    let source = source();
    let changed = edit();
    let result = FileResult {
        path: changed.path.clone(), material: changed.material.file,
        source: SnapshotFileSource::Container {
            branch: BranchId::from_random([90; 16]),
            node: FileNodeId::Version(changed.material.file.version_id),
        },
    };
    let view = SparseProjection::admit_with_results(&source, vec![changed], &[result.clone()])
        .unwrap().namespace(&[NamespaceChange::Rename {
            source: lookup("Dir/B.txt"), target: path("Saved.txt"),
        }]).unwrap();
    let plan = view.plan(disk(), |_, _| -> Result<ContainerPresence, ()> {
        panic!("the previously completed FILE version must not be published again")
    }).unwrap();
    assert!(matches!(plan.last().unwrap().transfer, Transfer::MoveWorking {
        container: ContainerPublication::Retained { file, source }, ..
    } if file == result.material && source == result.source));
}

#[test]
fn lookup_collisions_and_missing_sources_fail_without_changing_original_snapshot() {
    let source = source();
    for change in [
        NamespaceChange::AddDirectory { path: path("a.TXT") },
        NamespaceChange::Rename { source: lookup("Dir"), target: path("A.txt") },
    ] {
        assert!(matches!(project(&source, &[change]), Err(NamespaceError::Occupied)));
    }
    for change in [
        NamespaceChange::RemoveSubtree { path: lookup("Missing") },
        NamespaceChange::Rename { source: lookup("Missing"), target: path("Fresh") },
    ] {
        assert!(matches!(project(&source, &[change]), Err(NamespaceError::Missing)));
    }
    assert_eq!(source.entries().len(), 5);
    assert_eq!(source.entries()[0].path, path("A.txt"));
}

#[test]
fn renaming_into_descendants_or_over_transient_child_collisions_is_rejected() {
    let source = source();
    assert!(matches!(project(&source, &[NamespaceChange::Rename {
        source: lookup("Dir"), target: path("DIR/Empty/Nested"),
    }]), Err(NamespaceError::Descendant)));
    assert!(matches!(project(&source, &[
        NamespaceChange::AddDirectory { path: path("Target/Empty") },
        NamespaceChange::Rename { source: lookup("Dir"), target: path("Target") },
    ]), Err(NamespaceError::Occupied)));
}

#[test]
fn complete_view_reuses_snapshot_parent_and_management_admission() {
    let source = source();
    for target in ["Absent/New", "A.txt/New", "dir/New"] {
        assert!(matches!(project(&source, &[NamespaceChange::AddDirectory {
            path: path(target),
        }]), Err(NamespaceError::Material(SnapshotManifestError::Parent))));
    }
    assert!(matches!(project(&source, &[NamespaceChange::AddDirectory {
        path: path(".ZXS"),
    }]), Err(NamespaceError::Material(SnapshotManifestError::Management))));
}

#[test]
fn atomic_batch_admits_final_parent_closure_not_an_exposed_intermediate_view() {
    let source = source();
    let view = project(&source, &[
        NamespaceChange::Rename { source: lookup("A.txt"), target: path("Target/A.txt") },
        NamespaceChange::AddDirectory { path: path("Target") },
    ]).unwrap();
    assert_eq!(view.route(&lookup("Target/A.txt")),
        Some(ReadRoute::Original { file: file(3, b"A") }));
    assert_eq!(view.route(&lookup("Target")), Some(ReadRoute::Directory));
}

#[test]
fn fresh_worktree_projection_never_inherits_another_views_namespace_changes() {
    let source = source();
    let changed = project(&source, &[
        NamespaceChange::RemoveSubtree { path: lookup("Dir") },
        NamespaceChange::Rename { source: lookup("A.txt"), target: path("Moved.txt") },
    ]).unwrap();
    let fresh = SparseProjection::admit(&source, vec![]).unwrap();
    assert!(changed.route(&lookup("Dir/B.txt")).is_none());
    assert_eq!(fresh.route(&lookup("Dir/B.txt")),
        Some(ReadRoute::Original { file: file(4, b"B") }));
    assert!(fresh.route(&lookup("Moved.txt")).is_none());
    assert_eq!(fresh.paths().count(), source.entries().len());
}

#[test]
fn operation_inventory_and_rebased_path_work_are_bounded_before_exposure() {
    let source = source();
    let changes = vec![NamespaceChange::AddDirectory { path: path("New") }; MAX_CHANGES + 1];
    assert!(matches!(project(&source, &changes), Err(NamespaceError::Budget)));
    let long_target = path(&"x".repeat(200));
    assert!(matches!(project(&source, &[NamespaceChange::Rename {
        source: lookup("Dir"), target: long_target,
    }]), Err(NamespaceError::Path(_))));
}

#[test]
fn final_entry_inventory_cannot_exceed_the_snapshot_authoritys_limit() {
    let entries = (0..2048).map(|index| SnapshotEntry {
        path: path(&format!("D{index:04}")),
        content: SnapshotEntryContent::Directory {},
    }).collect();
    let source = manifest(2, entries);
    let sparse = SparseProjection::admit(&source, vec![]).unwrap();
    assert!(matches!(sparse.namespace(&[NamespaceChange::AddDirectory { path: path("Fresh") }]),
        Err(NamespaceError::Material(SnapshotManifestError::Budget))));
}

#[test]
fn maximum_batch_of_identity_preserving_renames_keeps_one_complete_view() {
    let source = source();
    let changes = (0..MAX_CHANGES).map(|index| NamespaceChange::Rename {
        source: lookup("Dir"),
        target: path(if index % 2 == 0 { "dir" } else { "Dir" }),
    }).collect::<Vec<_>>();
    let view = project(&source, &changes).unwrap();
    assert_eq!(view.paths().map(PortablePath::as_str).collect::<Vec<_>>(),
        source.entries().iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>());
}


#[test]
fn retained_namespace_proof_cannot_rebind_source_occurrences_to_another_snapshot() {
    let source = source();
    let namespace = project(&source, &[NamespaceChange::Rename {
        source: lookup("Dir"), target: path("Moved"),
    }]).unwrap().into_namespace();
    let other = manifest(20, source.entries().to_vec());
    assert!(matches!(SparseProjection::admit(&other, vec![]).unwrap().with_namespace(&namespace),
        Err(NamespaceError::Source)));
    let bound = SparseProjection::admit(&source, vec![edit()]).unwrap()
        .with_namespace(&namespace).unwrap();
    assert!(bound.route(&lookup("Dir/B.txt")).is_none());
    assert_eq!(bound.route(&lookup("Moved/B.txt")),
        Some(ReadRoute::Working { material: &edit().material }));
}
