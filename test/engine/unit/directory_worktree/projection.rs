use super::*;
use storage_contract::{ContentHash, OperationId};
use storage_contract::working_tree::{SnapshotNodeId, SnapshotRepositoryId};

fn file(id: u8, body: &[u8]) -> SnapshotFile {
    SnapshotFile {
        file_id: FileId::from_random([id; 16]),
        version_id: VersionId::from_random([id; 16]),
        size: body.len() as u64,
        content_hash: ContentHash::from_digest(blake3::hash(body)),
    }
}
fn manifest(node: u8, entries: Vec<SnapshotEntry>) -> AdmittedSnapshotManifest {
    AdmittedSnapshotManifest::prepare(
        SnapshotRepositoryId::from_random([1; 16]),
        SnapshotNodeId::from_random([node; 16]),
        OperationId::from_random([node; 16]),
        entries,
    ).unwrap()
}
fn source() -> AdmittedSnapshotManifest {
    manifest(2, vec![
        SnapshotEntry {
            path: PortablePath::parse("A.txt").unwrap(),
            content: content(file(3, b"original A")),
        },
        SnapshotEntry {
            path: PortablePath::parse("B.txt").unwrap(),
            content: content(file(4, b"original B")),
        },
        SnapshotEntry {
            path: PortablePath::parse("Empty").unwrap(),
            content: SnapshotEntryContent::Directory {},
        },
    ])
}
fn edit(body: &[u8]) -> FileEdit {
    let mut changed = file(3, body);
    changed.version_id = VersionId::from_random([5; 16]);
    FileEdit {
        path: PortablePath::parse("A.txt").unwrap(),
        material: WorkingFile {
            file: changed,
            leaf: EntryName::new("0123456789abcde-A.txt").unwrap(),
            entity: LocalEntityIdentity::new(10, 20),
        },
    }
}
fn route<'a>(view: &'a SparseProjection<'_>, path: &str) -> ReadRoute<'a> {
    view.route(&LookupPath::parse(path).unwrap()).unwrap()
}

#[test]
fn empty_sparse_view_reads_original_files_and_empty_directories_without_materializing_them() {
    let source = source();
    let view = SparseProjection::admit(&source, vec![]).unwrap();
    assert_eq!(route(&view, "A.txt"), ReadRoute::Original { file: file(3, b"original A") });
    assert_eq!(route(&view, "b.TXT"), ReadRoute::Original { file: file(4, b"original B") });
    assert_eq!(route(&view, "Empty"), ReadRoute::Directory);
    assert!(view.route(&LookupPath::parse("Missing").unwrap()).is_none());
}

#[test]
fn modified_files_move_and_untouched_files_copy_without_creating_untouched_file_nodes() {
    let source = source();
    let changed = edit(b"edited A");
    let view = SparseProjection::admit(&source, vec![changed.clone()]).unwrap();
    assert_eq!(route(&view, "A.txt"), ReadRoute::Working { material: &changed.material });
    let mut looked_up = Vec::new();
    let entity = LocalEntityIdentity::new(30, 40);
    let plan = view.plan(|identity| {
        looked_up.push(identity);
        Ok::<_, ()>(ContainerPresence::Present { entity })
    }).unwrap();
    assert_eq!(looked_up, [changed.material.file.file_id]);
    assert_eq!(plan[0].transfer, Transfer::MoveWorking {
        material: &changed.material,
        container: ContainerPublication::Update {
            file: changed.material.file.file_id,
            source_version: VersionId::from_random([3; 16]),
            source_branch: storage_contract::BranchId::from_random([70; 16]),
            source_node: storage_contract::FileNodeId::Version(VersionId::from_random([3; 16])),
            entity,
        },
    });
    assert_eq!(plan[1].transfer, Transfer::CopyOriginal { file: file(4, b"original B") });
    assert_eq!(plan[2].transfer, Transfer::Directory);
    assert_eq!(plan.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(),
        ["A.txt", "B.txt", "Empty"]);
}

#[test]
fn absent_container_keeps_snapshot_only_and_never_requests_a_recreated_container() {
    let source = source();
    let changed = edit(b"edited A");
    let view = SparseProjection::admit(&source, vec![changed.clone()]).unwrap();
    let plan = view.plan(|_| Ok::<_, ()>(ContainerPresence::Absent)).unwrap();
    assert_eq!(plan[0].transfer, Transfer::MoveWorking {
        material: &changed.material,
        container: ContainerPublication::SnapshotOnly { file: changed.material.file.file_id },
    });
}

#[test]
fn temporary_disk_failure_is_an_error_not_a_missing_container() {
    let source = source();
    let changed = edit(b"edited A");
    let view = SparseProjection::admit(&source, vec![changed.clone()]).unwrap();
    let failed = view.plan(|_| Err("source disk temporarily offline"));
    assert_eq!(failed.unwrap_err(), "source disk temporarily offline");
    assert_eq!(route(&view, "A.txt"), ReadRoute::Working { material: &changed.material });
}

#[test]
fn matching_bytes_do_not_erase_an_explicit_write_or_infer_its_source() {
    let source = source();
    let changed = edit(b"original A");
    let view = SparseProjection::admit(&source, vec![changed.clone()]).unwrap();
    let mut identities = Vec::new();
    let plan = view.plan(|identity| {
        identities.push(identity);
        Ok::<_, ()>(ContainerPresence::Absent)
    }).unwrap();
    assert_eq!(identities, [changed.material.file.file_id]);
    assert_eq!(plan[0].transfer, Transfer::MoveWorking {
        material: &changed.material,
        container: ContainerPublication::SnapshotOnly { file: changed.material.file.file_id },
    });
    assert_eq!(route(&view, "A.txt"), ReadRoute::Working { material: &changed.material });
}

#[test]
fn published_view_keeps_logical_paths_but_routes_all_files_to_the_new_node() {
    let source = source();
    let changed = edit(b"edited A");
    let view = SparseProjection::admit(&source, vec![changed.clone()]).unwrap();
    let mut entries = source.entries().to_vec();
    entries[0].content = content(changed.material.file);
    let published = manifest(9, entries);
    assert_eq!(view.published_view(&published, &published_sources(&changed)).unwrap(), published.reference());
    let after = SparseProjection::admit(&published, vec![]).unwrap();
    assert_eq!(route(&after, "A.txt"), ReadRoute::Original { file: changed.material.file });
    assert_eq!(route(&after, "B.txt"), ReadRoute::Original { file: file(4, b"original B") });
    assert_eq!(route(&after, "Empty"), ReadRoute::Directory);
}

#[test]
fn incomplete_or_stale_published_snapshot_does_not_replace_the_worktree_view() {
    let source = source();
    let view = SparseProjection::admit(&source, vec![edit(b"edited A")]).unwrap();
    let stale = manifest(9, source.entries().to_vec());
    assert!(matches!(view.published_view(&stale, &published_sources(&edit(b"edited A"))), Err(ProjectionError::PublishedView)));
    let incomplete = manifest(9, vec![source.entries()[0].clone()]);
    assert!(matches!(view.published_view(&incomplete, &published_sources(&edit(b"edited A"))), Err(ProjectionError::PublishedView)));
}

#[test]
fn overlay_rejects_duplicate_paths_foreign_file_identity_and_mutated_old_version_identity() {
    let source = source();
    let changed = edit(b"edited A");
    assert!(matches!(
        SparseProjection::admit(&source, vec![changed.clone(), changed.clone()]),
        Err(ProjectionError::Duplicate),
    ));
    let mut foreign = changed.clone();
    foreign.material.file.file_id = FileId::from_random([55; 16]);
    assert!(matches!(
        SparseProjection::admit(&source, vec![foreign]), Err(ProjectionError::Source),
    ));
    let mut mutation = changed;
    mutation.material.file.version_id = VersionId::from_random([3; 16]);
    assert!(matches!(
        SparseProjection::admit(&source, vec![mutation]), Err(ProjectionError::Version),
    ));
}

fn content(material: SnapshotFile) -> SnapshotEntryContent {
    SnapshotEntryContent::File {
        material,
        source: SnapshotFileSource::Container {
            branch: storage_contract::BranchId::from_random([70; 16]),
            node: storage_contract::FileNodeId::Version(material.version_id),
        },
    }
}

fn published_sources(edit: &FileEdit) -> Vec<PublishedFileSource> {
    vec![PublishedFileSource {
        path: edit.path.clone(),
        source: SnapshotFileSource::Container {
            branch: storage_contract::BranchId::from_random([70; 16]),
            node: storage_contract::FileNodeId::Version(edit.material.file.version_id),
        },
    }]
}

#[test]
fn snapshot_only_edits_never_query_or_guess_a_container_even_when_identity_matches() {
    let mut entries = source().entries().to_vec();
    entries[0].content = SnapshotEntryContent::File {
        material: file(3, b"original A"), source: SnapshotFileSource::SnapshotOnly,
    };
    let source = manifest(2, entries);
    let changed = edit(b"edited A");
    let view = SparseProjection::admit(&source, vec![changed.clone()]).unwrap();
    let plan = view.plan(|_| -> Result<ContainerPresence, ()> {
        panic!("snapshot-only provenance must never invoke a container locator")
    }).unwrap();
    assert_eq!(plan[0].transfer, Transfer::MoveWorking {
        material: &changed.material,
        container: ContainerPublication::SnapshotOnly { file: changed.material.file.file_id },
    });
    let mut result = source.entries().to_vec();
    result[0].content = content(changed.material.file);
    let forged = manifest(9, result);
    assert!(matches!(view.published_view(&forged, &published_sources(&changed)),
        Err(ProjectionError::PublishedView)));
}

#[test]
fn alias_edits_use_the_frozen_selected_branch_and_node_not_a_current_default() {
    let body = file(3, b"original A");
    let selected = SnapshotFileSource::Container {
        branch: storage_contract::BranchId::from_random([90; 16]),
        node: storage_contract::FileNodeId::Metadata(storage_contract::Id::from_random([91; 16])),
    };
    let source = manifest(2, vec![SnapshotEntry {
        path: PortablePath::parse("A.txt").unwrap(),
        content: SnapshotEntryContent::File { material: body, source: selected },
    }]);
    let view = SparseProjection::admit(&source, vec![edit(b"changed")]).unwrap();
    let plan = view.plan(|_| Ok::<_, ()>(ContainerPresence::Present {
        entity: LocalEntityIdentity::new(1, 2),
    })).unwrap();
    let Transfer::MoveWorking { container: ContainerPublication::Update {
        source_branch, source_node, ..
    }, .. } = plan[0].transfer else { panic!("expected known-container change") };
    assert_eq!(SnapshotFileSource::Container { branch: source_branch, node: source_node }, selected);
}
