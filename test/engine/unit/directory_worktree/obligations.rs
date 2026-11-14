use super::*;
use crate::directory_worktree::projection::namespace::NamespaceChange;
use crate::directory_worktree::projection::{
    ContainerPresence, ContainerPublication, FileEdit, FileResult, Transfer,
};
use crate::working_tree::snapshot::{AdmittedSnapshotManifest, SnapshotEntry};
use name_policy::LookupPath;
use platform_io::fs_safety::EntryName;
use storage_contract::working_tree::{
    LocalEntityIdentity, SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::{BranchId, ContentHash, DiskId, FileId, FileNodeId, OperationId, VersionId};

fn path(value: &str) -> PortablePath { PortablePath::parse(value).unwrap() }
fn lookup(value: &str) -> LookupPath { LookupPath::parse(value).unwrap() }
fn disk() -> DiskId { DiskId::from_random([80; 16]) }
fn material(file: u8, version: u8, body: &[u8]) -> SnapshotFile {
    SnapshotFile {
        file_id: FileId::from_random([file; 16]),
        version_id: VersionId::from_random([version; 16]),
        size: body.len() as u64,
        content_hash: ContentHash::from_digest(blake3::hash(body)),
    }
}
fn pin(branch: u8, version: u8) -> SnapshotFileSource {
    SnapshotFileSource::Container {
        branch: BranchId::from_random([branch; 16]),
        node: FileNodeId::Version(VersionId::from_random([version; 16])),
    }
}
fn alias_pin() -> SnapshotFileSource {
    SnapshotFileSource::Container {
        branch: BranchId::from_random([71; 16]),
        node: FileNodeId::Metadata(storage_contract::Id::from_random([72; 16])),
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
    let original = material(7, 7, b"same source");
    manifest(2, vec![
        SnapshotEntry {
            path: path("A.txt"),
            content: SnapshotEntryContent::File { material: original, source: pin(70, 7) },
        },
        SnapshotEntry {
            path: path("B.txt"),
            content: SnapshotEntryContent::File { material: original, source: alias_pin() },
        },
        SnapshotEntry {
            path: path("Keep.txt"),
            content: SnapshotEntryContent::File {
                material: material(8, 8, b"unchanged"), source: pin(73, 8),
            },
        },
    ])
}
fn edit(name: &str, version: u8, body: &[u8]) -> FileEdit {
    FileEdit {
        path: path(name),
        material: WorkingFile {
            file: material(7, version, body),
            leaf: EntryName::new(format!("{version:015x}-{name}")).unwrap(),
            entity: LocalEntityIdentity::new(10, u64::from(version)),
        },
    }
}
fn saved(edit: &FileEdit, source: SnapshotFileSource) -> FileResult {
    FileResult { path: edit.path.clone(), material: edit.material.file, source }
}

#[test]
fn aliases_keep_exact_original_pins_after_one_is_removed_and_its_path_is_reused() {
    let mut entries = source().entries().to_vec();
    // Edited occurrences have nonconsecutive source indexes, not edit positions.
    entries.insert(1, SnapshotEntry {
        path: path("A0-empty"),
        content: SnapshotEntryContent::Directory {},
    });
    entries.insert(0, SnapshotEntry {
        path: path("0-empty"),
        content: SnapshotEntryContent::Directory {},
    });
    let source = manifest(2, entries);
    // Equal bodies do not collapse independent writes or occurrence baselines.
    let a = edit("A.txt", 11, b"same changed bytes");
    let b = edit("B.txt", 12, b"same changed bytes");
    let view = SparseProjection::admit(&source, vec![b.clone(), a.clone()]).unwrap()
        .namespace(&[
            NamespaceChange::RemoveSubtree { path: lookup("b.TXT") },
            NamespaceChange::Rename { source: lookup("a.TXT"), target: path("B.txt") },
        ]).unwrap();
    let inventory = view.file_obligations();
    let files = inventory.files().collect::<Vec<_>>();
    assert_eq!(files.len(), 2);
    assert_eq!(files.iter().map(|file| file.original().path().as_str()).collect::<Vec<_>>(),
        ["A.txt", "B.txt"]);
    for file in &files {
        assert_eq!(file.original().snapshot(), source.reference());
        assert_eq!(file.original().baseline().material(), material(7, 7, b"same source"));
    }
    assert_eq!(files[0].material(), &a.material);
    assert_eq!(files[0].original().baseline().source(), pin(70, 7));
    assert_eq!(files[0].body(), WorkingBodyDisposition::Visible { path: &path("B.txt") });
    assert_eq!(files[0].publication(), FilePublicationRequirement::Pending {
        baseline: files[0].original().baseline(),
    });
    assert_eq!(files[1].material(), &b.material);
    assert_eq!(files[1].original().baseline().source(), alias_pin());
    assert_eq!(files[1].body(), WorkingBodyDisposition::RetainedInWorktree);
    assert_eq!(files[1].publication(), FilePublicationRequirement::Pending {
        baseline: files[1].original().baseline(),
    });

    let mut located = Vec::new();
    let visible = view.plan(disk(), |selected_disk, file| {
        located.push((selected_disk, file));
        Ok::<_, ()>(ContainerPresence::Present {
            disk: selected_disk, entity: LocalEntityIdentity::new(10, 100),
        })
    }).unwrap();
    assert_eq!(located, [(disk(), a.material.file.file_id)]);
    assert_eq!(visible.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(),
        ["0-empty", "A0-empty", "B.txt", "Keep.txt"]);
    assert_eq!(visible[0].transfer, Transfer::Directory);
    assert_eq!(visible[1].transfer, Transfer::Directory);
    assert!(matches!(visible[2].transfer, Transfer::MoveWorking {
        material, container: ContainerPublication::Update { source_branch, source_node, .. },
    } if material == &a.material
        && source_branch == BranchId::from_random([70; 16])
        && source_node == FileNodeId::Version(VersionId::from_random([7; 16]))));
    assert_eq!(visible[3].transfer, Transfer::CopyOriginal {
        file: material(8, 8, b"unchanged"),
    });
}

#[test]
fn exact_saved_occurrence_suppresses_only_its_file_obligation_not_other_alias_or_body_move() {
    let source = source();
    let a = edit("A.txt", 11, b"saved alias");
    let b = edit("B.txt", 12, b"unsaved hidden alias");
    let result = saved(&a, pin(90, 11));
    let view = SparseProjection::admit_with_results(
        &source, vec![a.clone(), b.clone()], &[result.clone()],
    ).unwrap().namespace(&[
        NamespaceChange::RemoveSubtree { path: lookup("B.txt") },
        NamespaceChange::Rename { source: lookup("A.txt"), target: path("Saved.txt") },
    ]).unwrap();
    let inventory = view.file_obligations();
    let files = inventory.files().collect::<Vec<_>>();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].original().path(), &result.path);
    assert_eq!(files[0].body(), WorkingBodyDisposition::Visible { path: &path("Saved.txt") });
    assert_eq!(files[0].publication(), FilePublicationRequirement::Recorded {
        result: FileBaseline { material: result.material, source: result.source },
    });
    assert_eq!(files[1].original().path(), &b.path);
    assert_eq!(files[1].body(), WorkingBodyDisposition::RetainedInWorktree);
    assert_eq!(files[1].publication(), FilePublicationRequirement::Pending {
        baseline: files[1].original().baseline(),
    });
    let visible = view.plan(disk(), |_, _| -> Result<ContainerPresence, ()> {
        panic!("saved visible FILE versions and hidden bodies do not trigger this locator")
    }).unwrap();
    assert_eq!(visible.last().unwrap().transfer, Transfer::MoveWorking {
        material: &a.material,
        container: ContainerPublication::Retained { file: result.material, source: result.source },
    });
    assert_eq!(files[1].material(), &b.material);
}

#[test]
fn saved_removed_body_stays_in_the_same_worktree_without_a_final_snapshot_path() {
    let source = source();
    let changed = edit("A.txt", 11, b"retained saved bytes");
    let result = saved(&changed, pin(90, 11));
    let view = SparseProjection::admit_with_results(
        &source, vec![changed.clone()], &[result.clone()],
    ).unwrap().namespace(&[NamespaceChange::RemoveSubtree { path: lookup("A.txt") }]).unwrap();
    let inventory = view.file_obligations();
    assert_eq!(inventory.files().len(), 1);
    let retained = inventory.files().next().unwrap();
    assert_eq!(retained.original().path(), &changed.path);
    assert_eq!(retained.material(), &changed.material);
    assert_eq!(retained.body(), WorkingBodyDisposition::RetainedInWorktree);
    assert_eq!(retained.publication(), FilePublicationRequirement::Recorded {
        result: FileBaseline { material: result.material, source: result.source },
    });
    let visible = view.plan(disk(), |_, _| -> Result<ContainerPresence, ()> {
        panic!("a retained hidden body is not a final snapshot transfer")
    }).unwrap();
    assert_eq!(visible.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(),
        ["B.txt", "Keep.txt"]);
    assert!(visible.iter().all(|entry| matches!(entry.transfer, Transfer::CopyOriginal { .. })));
}

#[test]
fn later_hidden_write_keeps_original_occurrence_and_uses_last_recorded_baseline() {
    let source = source();
    let prior = edit("A.txt", 11, b"previous saved bytes");
    let later = edit("A.txt", 13, b"latest explicit write");
    for selected in [pin(90, 11), SnapshotFileSource::SnapshotOnly] {
        let result = saved(&prior, selected);
        let view = SparseProjection::admit_with_results(
            &source, vec![later.clone()], &[result.clone()],
        ).unwrap().namespace(&[NamespaceChange::RemoveSubtree { path: lookup("A.txt") }]).unwrap();
        let inventory = view.file_obligations();
        assert_eq!(inventory.files().len(), 1);
        let file = inventory.files().next().unwrap();
        assert_eq!(file.original().snapshot(), source.reference());
        assert_eq!(file.original().path(), &later.path);
        assert_eq!(file.original().baseline(), FileBaseline {
            material: material(7, 7, b"same source"), source: pin(70, 7),
        });
        assert_eq!(file.material(), &later.material);
        assert_eq!(file.body(), WorkingBodyDisposition::RetainedInWorktree);
        assert_eq!(file.publication(), FilePublicationRequirement::Pending {
            baseline: FileBaseline { material: result.material, source: selected },
        });
    }
}

#[test]
fn snapshot_only_obligation_never_creates_container_presence_or_selects_a_current_branch() {
    let mut entries = source().entries().to_vec();
    entries.remove(1);
    entries[0].content = SnapshotEntryContent::File {
        material: material(7, 7, b"same source"), source: SnapshotFileSource::SnapshotOnly,
    };
    let source = manifest(2, entries);
    let changed = edit("A.txt", 11, b"snapshot only write");
    let view = SparseProjection::admit(&source, vec![changed.clone()]).unwrap();
    let inventory = view.file_obligations();
    let file = inventory.files().next().unwrap();
    assert_eq!(file.original().baseline().source(), SnapshotFileSource::SnapshotOnly);
    assert_eq!(file.publication(), FilePublicationRequirement::Pending {
        baseline: file.original().baseline(),
    });
    let visible = view.plan(disk(), |_, _| -> Result<ContainerPresence, ()> {
        panic!("snapshot-only source does not confer container identity authority")
    }).unwrap();
    assert_eq!(visible[0].transfer, Transfer::MoveWorking {
        material: &changed.material,
        container: ContainerPublication::SnapshotOnly { file: changed.material.file.file_id },
    });
}

#[test]
fn snapshot_reference_distinguishes_identical_occurrences_from_different_sources() {
    let first = source();
    let second = manifest(20, first.entries().to_vec());
    let changed = edit("A.txt", 11, b"explicit write");
    let first_view = SparseProjection::admit(&first, vec![changed.clone()]).unwrap();
    let second_view = SparseProjection::admit(&second, vec![changed]).unwrap();
    let first_plan = first_view.file_obligations();
    let second_plan = second_view.file_obligations();
    let first_file = first_plan.files().next().unwrap();
    let second_file = second_plan.files().next().unwrap();
    assert_eq!(first_file.original().path(), second_file.original().path());
    assert_eq!(first_file.original().baseline(), second_file.original().baseline());
    assert_eq!(first_file.material(), second_file.material());
    assert_ne!(first_file.original().snapshot(), second_file.original().snapshot());
    assert_ne!(first_file.original(), second_file.original());
}

#[test]
fn namespace_only_changes_do_not_invent_file_obligations_or_container_lookups() {
    let source = source();
    let view = SparseProjection::admit(&source, vec![]).unwrap().namespace(&[
        NamespaceChange::AddDirectory { path: path("Empty") },
        NamespaceChange::RemoveSubtree { path: lookup("B.txt") },
        NamespaceChange::Rename { source: lookup("A.txt"), target: path("Renamed.txt") },
    ]).unwrap();
    assert_eq!(view.file_obligations().files().len(), 0);
    let visible = view.plan(disk(), |_, _| -> Result<ContainerPresence, ()> {
        panic!("namespace-only intent never creates a FILE content node")
    }).unwrap();
    assert_eq!(visible.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(),
        ["Empty", "Keep.txt", "Renamed.txt"]);
    assert_eq!(visible[0].transfer, Transfer::Directory);
    assert!(visible[1..].iter().all(|entry| {
        matches!(entry.transfer, Transfer::CopyOriginal { .. })
    }));
}
