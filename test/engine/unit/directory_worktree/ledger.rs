use crate::directory_worktree::projection::{ContainerPresence, ContainerPublication, FileResult, Transfer};
use super::*;
use crate::directory_worktree::lifecycle::{Mount, Phase, Publication, PublicationScope, WorktreeId};
use crate::directory_worktree::store::HeldSparseWorktree;
use crate::disk::DiskRoot;
use crate::persist::layout::worktree as layout;
use crate::working_tree::records::local_entity;
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, HeldSnapshotNode, SnapshotEntry, SnapshotEntryContent,
    SnapshotFile, SnapshotFileSource,
};
use name_policy::{LookupName, PortablePath};
use platform_io::{fs_safety::PrivateDirectory, payload::Expectation};
use std::{fs, io::Cursor};
use storage_contract::{ContentHash, FileId, OperationId, VersionId};
use storage_contract::working_tree::{DirectoryId, SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId};

struct Fixture {
    _temp: tempfile::TempDir,
    source: HeldSnapshotNode,
    instance: HeldSparseWorktree,
}
fn operation(byte: u8) -> OperationId { OperationId::from_random([byte; 16]) }
fn mount(name: &str) -> Mount {
    Mount {
        parent: DirectoryId::from_random([90; 16]),
        name: LookupName::parse(name).unwrap(),
    }
}
impl Fixture {
    fn new() -> Self {
        Self::with_origin(SnapshotFileSource::SnapshotOnly)
    }
    fn with_origin(origin: SnapshotFileSource) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repository");
        PrivateDirectory::create_all(&path).unwrap();
        let repository = DiskRoot::open(&path).unwrap();
        let manifest = AdmittedSnapshotManifest::prepare(
            SnapshotRepositoryId::from_random([1; 16]),
            SnapshotNodeId::from_random([2; 16]), operation(3),
            vec![SnapshotEntry {
                path: PortablePath::parse("file.txt").unwrap(),
                content: SnapshotEntryContent::File {
                    material: SnapshotFile {
                        file_id: FileId::from_random([4; 16]),
                        version_id: VersionId::from_random([5; 16]),
                        size: 8, content_hash: ContentHash::from_digest(blake3::hash(b"original")),
                    },
                    source: origin,
                },
            }, SnapshotEntry {
                path: PortablePath::parse("other.txt").unwrap(),
                content: SnapshotEntryContent::File {
                    material: SnapshotFile {
                        file_id: FileId::from_random([44; 16]),
                        version_id: VersionId::from_random([55; 16]),
                        size: 8, content_hash: ContentHash::from_digest(blake3::hash(b"original")),
                    },
                    source: SnapshotFileSource::SnapshotOnly,
                },
            }],
        ).unwrap();
        let source = HeldSnapshotNode::prepare(&repository, manifest, |_| {
            Ok(Cursor::new(b"original"))
        }).unwrap();
        let instance = HeldSparseWorktree::prepare(
            &source, WorktreeId::from_random([6; 16]), operation(6),
        ).unwrap();
        Self { _temp: temp, source, instance }
    }
    fn ledger(&self) -> HeldWorktreeLedger<'_> {
        HeldWorktreeLedger::prepare(
            &self.source, self.instance.binding(), SnapshotBranchId::from_random([7; 16]),
            mount("worktree-main-001"),
        ).unwrap()
    }
    fn edit(&self) -> EditRecord {
        self.write("file.txt", 4, 8)
    }
    fn write(&self, path: &str, file: u8, version: u8) -> EditRecord {
        let path = PortablePath::parse(path).unwrap();
        let material = SnapshotFile {
            file_id: FileId::from_random([file; 16]),
            version_id: VersionId::from_random([version; 16]),
            size: 6, content_hash: ContentHash::from_digest(blake3::hash(b"edited")),
        };
        let leaf = layout::edited_file(material.version_id, &path);
        let receipt = self.instance.directory().copy_new_entry_verified_synced(
            &leaf, &mut Cursor::new(b"edited"),
            Expectation { bytes: 6, blake3: Some(blake3::hash(b"edited")) },
        ).unwrap();
        EditRecord {
            path, material, leaf: LookupName::parse(leaf.as_os_str().to_str().unwrap()).unwrap(),
            entity: local_entity(receipt.identity),
        }
    }
    fn frame_path(&self, sequence: u64) -> std::path::PathBuf {
        self.source.worktrees_directory().path()
            .join(layout::ledger(self.instance.binding().worktree()).as_path())
            .join(layout::frame(storage_contract::Sequence::new(sequence)).as_path())
    }
}
fn trusted(_: &Publication, _: &[FileResult]) -> Result<(), LedgerError> { Ok(()) }
fn delete_event(ledger: &HeldWorktreeLedger<'_>, byte: u8) -> Event {
    Event::Delete { operation: operation(byte), expected: ledger.worktree().revision() }
}
fn completed(ledger: &HeldWorktreeLedger<'_>) -> Publication {
    let Phase::Publishing { intent, .. } = ledger.worktree().phase() else {
        panic!("delete intent must be durably pending")
    };
    Publication { intent, scope: PublicationScope::FilesOnly }
}

#[test]
fn genesis_is_durable_beside_an_empty_instance_and_requires_its_external_anchor() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    assert_eq!(fixture.instance.directory().root_entries().unwrap().count(), 0);
    let binding: Binding = storage_contract::decode_strict(
        &storage_contract::canonical_json(&ledger.binding()).unwrap(),
    ).unwrap();
    let checkpoint = ledger.checkpoint();
    let initial = ledger.worktree().clone();
    drop(ledger);
    let restored = HeldWorktreeLedger::reopen(&fixture.source, binding, checkpoint, trusted).unwrap();
    assert_eq!(restored.worktree(), &initial);
    assert_eq!(restored.instance(), fixture.instance.binding());
    assert!(restored.edits().is_empty());
    assert!(matches!(HeldWorktreeLedger::prepare(
        &fixture.source, fixture.instance.binding(), SnapshotBranchId::from_random([7; 16]),
        mount("worktree-main-002"),
    ), Err(LedgerPreparationError::Rejected(_))));
}

#[test]
fn sealed_edit_bytes_identity_and_revision_survive_cold_reopen() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let edit = fixture.edit();
    let event = Event::Edits {
        operation: operation(10), expected: ledger.worktree().revision(), edits: vec![edit.clone()],
    };
    ledger.append(event.clone(), trusted).unwrap();
    let binding = ledger.binding();
    let checkpoint = ledger.checkpoint();
    let state = ledger.worktree().clone();
    drop(ledger);
    let mut restored = HeldWorktreeLedger::reopen(
        &fixture.source, binding, checkpoint, trusted,
    ).unwrap();
    assert_eq!(restored.worktree(), &state);
    assert_eq!(restored.edits(), [edit]);
    assert_eq!(restored.append(event, trusted).unwrap(), checkpoint);
    assert!(!fixture.frame_path(2).exists());
}

#[test]
fn delete_restore_and_repeat_delete_retain_the_original_file_publication_once() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let edit = fixture.edit();
    ledger.append(Event::Edits {
        operation: operation(10), expected: ledger.worktree().revision(), edits: vec![edit],
    }, trusted).unwrap();
    let delete = delete_event(&ledger, 11);
    ledger.append(delete.clone(), trusted).unwrap();
    let publication = completed(&ledger);
    let files = result_files(&ledger);
    let mut verified = 0;
    ledger.append(Event::Completed { publication, files: files.clone() }, |_, _| {
        verified += 1;
        Ok(())
    }).unwrap();
    assert_eq!(verified, 1);
    ledger.append(Event::Restored {
        operation: operation(12), mount: mount("worktree-main-002"),
    }, trusted).unwrap();
    ledger.append(delete_event(&ledger, 13), trusted).unwrap();
    assert!(matches!(ledger.worktree().phase(), Phase::Deleted { publication: saved, .. }
        if saved == publication));
    let latest = ledger.checkpoint();
    assert_eq!(ledger.append(delete, trusted).unwrap(), latest);
    assert_eq!(ledger.append(Event::Completed { publication, files: files.clone() }, |_, _| {
        panic!("recorded completion must not be republished")
    }).unwrap(), latest);
    let mut reverified = 0;
    let restored = HeldWorktreeLedger::reopen(
        &fixture.source, ledger.binding(), latest, |_, _| { reverified += 1; Ok(()) },
    ).unwrap();
    assert_eq!(reverified, 1);
    assert_eq!(restored.worktree(), ledger.worktree());
}

#[test]
fn competing_append_cannot_overwrite_and_loser_must_reconcile_before_retrying() {
    let fixture = Fixture::new();
    let mut first = fixture.ledger();
    let initial = first.checkpoint();
    let mut second = HeldWorktreeLedger::reopen(
        &fixture.source, first.binding(), initial, trusted,
    ).unwrap();
    let winner = first.append(delete_event(&first, 10), trusted).unwrap();
    assert!(matches!(second.append(delete_event(&second, 11), trusted),
        Err(LedgerError::Append { .. } | LedgerError::Chain)));
    assert!(matches!(second.append(delete_event(&second, 11), trusted),
        Err(LedgerError::Uncertain)));
    assert!(HeldWorktreeLedger::reopen(&fixture.source, first.binding(), initial, trusted).is_err());
    let RecoveredLedger::Publishing(recovered) = HeldWorktreeLedger::recover(
        &fixture.source, first.binding(), winner, trusted,
    ).unwrap() else { panic!("pending intent cannot become a ready ledger") };
    assert_eq!(recovered.worktree(), first.worktree());
}

#[test]
fn missing_extra_or_changed_frames_are_never_silently_repaired_or_adopted() {
    for mode in 0..3 {
        let fixture = Fixture::new();
        let mut ledger = fixture.ledger();
        ledger.append(delete_event(&ledger, 10), trusted).unwrap();
        let before = fs::read(fixture.frame_path(0)).unwrap();
        match mode {
            0 => fs::remove_file(fixture.frame_path(1)).unwrap(),
            1 => fs::write(fixture.frame_path(2), b"unregistered tail").unwrap(),
            2 => fs::write(fixture.frame_path(1), b"{}").unwrap(),
            _ => panic!("bounded fixture modes"),
        }
        assert!(HeldWorktreeLedger::reopen(
            &fixture.source, ledger.binding(), ledger.checkpoint(), trusted,
        ).is_err());
        assert_eq!(fs::read(fixture.frame_path(0)).unwrap(), before);
    }
}

#[test]
fn completed_json_does_not_replace_independent_publication_verification() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    ledger.append(delete_event(&ledger, 10), trusted).unwrap();
    let publication = completed(&ledger);
    assert!(matches!(ledger.append(Event::Completed { publication, files: result_files(&ledger) }, |_, _| {
        Err(LedgerError::Binding)
    }), Err(LedgerError::Binding)));
    assert!(!fixture.frame_path(2).exists());
    ledger.append(Event::Completed { publication, files: result_files(&ledger) }, trusted).unwrap();
    assert!(HeldWorktreeLedger::reopen(
        &fixture.source, ledger.binding(), ledger.checkpoint(), |_, _| Err(LedgerError::Binding),
    ).is_err());
}

#[test]
fn stale_or_reused_operations_fail_before_persisting_a_frame() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let old = ledger.worktree().revision();
    let edit = fixture.edit();
    ledger.append(Event::Edits {
        operation: operation(10), expected: old, edits: vec![edit],
    }, trusted).unwrap();
    assert!(matches!(ledger.append(Event::Delete {
        operation: operation(11), expected: old,
    }, trusted), Err(LedgerError::Stale)));
    assert!(matches!(ledger.append(delete_event(&ledger, 10), trusted),
        Err(LedgerError::OperationConflict)));
    assert!(!fixture.frame_path(2).exists());
}

#[test]
fn body_substitution_or_incomplete_write_cannot_become_a_durable_edit_fact() {
    for substitute in [false, true] {
        let fixture = Fixture::new();
        let mut ledger = fixture.ledger();
        let edit = fixture.edit();
        let path = fixture.instance.directory().path().join(edit.leaf.as_str());
        if substitute {
            fs::rename(&path, path.with_extension("retained")).unwrap();
            fs::write(&path, b"edited").unwrap();
        } else {
            fs::write(&path, b"edit").unwrap();
        }
        assert!(ledger.append(Event::Edits {
            operation: operation(10), expected: ledger.worktree().revision(), edits: vec![edit],
        }, trusted).is_err());
        assert!(!fixture.frame_path(1).exists());
        assert_eq!(ledger.worktree().revision().generation.get(), 0);
    }
}

#[test]
fn later_explicit_writes_merge_without_silently_discarding_earlier_file_edits() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let first = fixture.edit();
    ledger.append(Event::Edits {
        operation: operation(10), expected: ledger.worktree().revision(), edits: vec![first.clone()],
    }, trusted).unwrap();
    let second = fixture.write("other.txt", 44, 9);
    ledger.append(Event::Edits {
        operation: operation(11), expected: ledger.worktree().revision(), edits: vec![second.clone()],
    }, trusted).unwrap();
    assert_eq!(ledger.edits(), [first, second]);
    let checkpoint = ledger.checkpoint();
    assert!(ledger.append(Event::Edits {
        operation: operation(12), expected: ledger.worktree().revision(), edits: vec![],
    }, trusted).is_err());
    assert_eq!(ledger.checkpoint(), checkpoint);
    let recovered = HeldWorktreeLedger::reopen(
        &fixture.source, ledger.binding(), checkpoint, trusted,
    ).unwrap();
    assert_eq!(recovered.edits(), ledger.edits());
}

#[test]
fn pending_body_transfer_recovers_only_as_frozen_publisher_work_not_a_readable_view() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let edit = fixture.edit();
    ledger.append(Event::Edits {
        operation: operation(10), expected: ledger.worktree().revision(), edits: vec![edit.clone()],
    }, trusted).unwrap();
    ledger.append(Event::Commit {
        operation: operation(11), expected: ledger.worktree().revision(),
        current_head: fixture.source.manifest().node_id(),
        fork_branch: SnapshotBranchId::from_random([20; 16]),
    }, trusted).unwrap();
    let path = fixture.instance.directory().path().join(edit.leaf.as_str());
    fs::rename(&path, fixture._temp.path().join("frozen-transfer-evidence")).unwrap();
    assert!(matches!(HeldWorktreeLedger::reopen(
        &fixture.source, ledger.binding(), ledger.checkpoint(), trusted,
    ), Err(LedgerError::PublicationPending)));
    let RecoveredLedger::Publishing(mut pending) = HeldWorktreeLedger::recover(
        &fixture.source, ledger.binding(), ledger.checkpoint(), trusted,
    ).unwrap() else { panic!("an incomplete move cannot produce an ordinary ready view") };
    assert_eq!(pending.edits(), [edit]);
    let Phase::Publishing { intent, .. } = pending.worktree().phase() else {
        panic!("the original intent remains frozen")
    };
    let receipt = Publication {
        intent, scope: PublicationScope::Directory {
            branch: intent.source.branch, node: SnapshotNodeId::from_random([21; 16]),
        },
    };
    assert!(pending.finish(receipt, result_files(&ledger), |_, _| Err(LedgerError::Binding)).is_err());
    assert!(!fixture.frame_path(3).exists());
}

#[test]
fn replacing_a_held_ledger_parent_name_never_publishes_to_a_detached_directory() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let path = fixture.frame_path(0).parent().unwrap().to_path_buf();
    let retained = path.with_extension("retained");
    fs::rename(&path, &retained).unwrap();
    PrivateDirectory::create(&path).unwrap();
    assert!(ledger.append(delete_event(&ledger, 10), trusted).is_err());
    assert!(!retained.join(layout::frame(storage_contract::Sequence::new(1)).as_path()).exists());
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
}

#[test]
fn older_sealed_versions_cannot_be_redefined_after_a_later_edit_replaces_them() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let first = fixture.edit();
    ledger.append(Event::Edits {
        operation: operation(10), expected: ledger.worktree().revision(), edits: vec![first.clone()],
    }, trusted).unwrap();
    let second = fixture.write("file.txt", 4, 9);
    ledger.append(Event::Edits {
        operation: operation(11), expected: ledger.worktree().revision(), edits: vec![second],
    }, trusted).unwrap();
    let mut recovered = HeldWorktreeLedger::reopen(
        &fixture.source, ledger.binding(), ledger.checkpoint(), trusted,
    ).unwrap();
    assert!(matches!(recovered.append(Event::Edits {
        operation: operation(12), expected: recovered.worktree().revision(), edits: vec![first],
    }, trusted), Err(LedgerError::OperationConflict)));
    assert!(!fixture.frame_path(3).exists());
}

fn result_files(ledger: &HeldWorktreeLedger<'_>) -> Vec<FileResult> {
    ledger.dirty_edits().into_iter().map(|edit| FileResult {
        path: edit.path.clone(), material: edit.material, source: SnapshotFileSource::SnapshotOnly,
    }).collect()
}

#[test]
fn files_only_results_survive_restore_and_allow_new_explicit_edits_and_directory_commit() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let first = fixture.edit();
    ledger.append(Event::Edits {
        operation: operation(10), expected: ledger.worktree().revision(), edits: vec![first.clone()],
    }, trusted).unwrap();
    ledger.append(delete_event(&ledger, 11), trusted).unwrap();
    let files = result_files(&ledger);
    ledger.append(Event::Completed { publication: completed(&ledger), files: files.clone() }, trusted)
        .unwrap();
    ledger.append(Event::Restored {
        operation: operation(12), mount: mount("worktree-main-002"),
    }, trusted).unwrap();
    let mut recovered = HeldWorktreeLedger::reopen(
        &fixture.source, ledger.binding(), ledger.checkpoint(), trusted,
    ).unwrap();
    assert_eq!(recovered.file_results(), files);
    assert!(recovered.dirty_edits().is_empty());
    let view = recovered.projection().unwrap();
    let plan = view.plan(|_| -> Result<ContainerPresence, ()> {
        panic!("a verified saved version must not republish a FILE node")
    }).unwrap();
    assert!(matches!(plan[0].transfer, Transfer::MoveWorking {
        container: ContainerPublication::Retained { .. }, ..
    }));
    let second = fixture.write("file.txt", 4, 9);
    recovered.append(Event::Edits {
        operation: operation(13), expected: recovered.worktree().revision(), edits: vec![second],
    }, trusted).unwrap();
    assert_eq!(recovered.dirty_edits().len(), 1);
    assert_eq!(recovered.file_results(), files);
    recovered.append(Event::Commit {
        operation: operation(14), expected: recovered.worktree().revision(),
        current_head: fixture.source.manifest().node_id(),
        fork_branch: SnapshotBranchId::from_random([20; 16]),
    }, trusted).unwrap();
    assert!(matches!(recovered.worktree().phase(), Phase::Publishing { .. }));
}

#[test]
fn completion_rejects_missing_extra_wrong_occurrence_or_unverified_file_results() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let edit = fixture.edit();
    ledger.append(Event::Edits {
        operation: operation(10), expected: ledger.worktree().revision(), edits: vec![edit],
    }, trusted).unwrap();
    ledger.append(delete_event(&ledger, 11), trusted).unwrap();
    let publication = completed(&ledger);
    let files = result_files(&ledger);
    let mut wrong = files.clone();
    wrong[0].path = PortablePath::parse("other.txt").unwrap();
    let mut mismatched = files.clone();
    mismatched[0].material.version_id = VersionId::from_random([88; 16]);
    for bad in [vec![], vec![files[0].clone(), files[0].clone()], wrong, mismatched] {
        assert!(ledger.append(Event::Completed { publication, files: bad }, |_, _| {
            panic!("invalid results cannot reach authority verification")
        }).is_err());
    }
    assert!(ledger.append(Event::Completed { publication, files: files.clone() }, |_, _| {
        Err(LedgerError::Binding)
    }).is_err());
    assert!(!fixture.frame_path(3).exists());
    ledger.append(Event::Completed { publication, files: files.clone() }, |_, verified| {
        assert_eq!(verified, files);
        Ok(())
    }).unwrap();
    let mut observed = Vec::new();
    HeldWorktreeLedger::reopen(
        &fixture.source, ledger.binding(), ledger.checkpoint(), |_, verified| {
            observed.extend_from_slice(verified);
            Ok(())
        },
    ).unwrap();
    assert_eq!(observed, files);
}

#[test]
fn directory_completion_still_blocks_edits_until_the_material_read_view_is_rebound() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    ledger.append(Event::Commit {
        operation: operation(10), expected: ledger.worktree().revision(),
        current_head: fixture.source.manifest().node_id(),
        fork_branch: SnapshotBranchId::from_random([20; 16]),
    }, trusted).unwrap();
    let Phase::Publishing { intent, .. } = ledger.worktree().phase() else {
        panic!("directory publication intent")
    };
    ledger.append(Event::Completed {
        publication: Publication { intent, scope: PublicationScope::Directory {
            branch: intent.source.branch, node: SnapshotNodeId::from_random([30; 16]),
        }}, files: vec![],
    }, trusted).unwrap();
    assert!(matches!(ledger.append(Event::Edits {
        operation: operation(11), expected: ledger.worktree().revision(), edits: vec![fixture.edit()],
    }, trusted), Err(LedgerError::RebaseRequired)));
}

#[test]
fn cold_file_baseline_preserves_selected_branch_and_excludes_saved_siblings_from_dirty_results() {
    let fixture = Fixture::with_origin(SnapshotFileSource::Container {
        branch: storage_contract::BranchId::from_random([60; 16]),
        node: storage_contract::FileNodeId::Version(VersionId::from_random([5; 16])),
    });
    let mut ledger = fixture.ledger();
    let first = fixture.edit();
    let sibling = fixture.write("other.txt", 44, 9);
    ledger.append(Event::Edits {
        operation: operation(10), expected: ledger.worktree().revision(),
        edits: vec![first.clone(), sibling],
    }, trusted).unwrap();
    ledger.append(delete_event(&ledger, 11), trusted).unwrap();
    let branch = storage_contract::BranchId::from_random([61; 16]);
    let mut files = result_files(&ledger);
    files[0].source = SnapshotFileSource::Container {
        branch, node: storage_contract::FileNodeId::Version(first.material.version_id),
    };
    ledger.append(Event::Completed { publication: completed(&ledger), files }, trusted).unwrap();
    ledger.append(Event::Restored {
        operation: operation(12), mount: mount("worktree-main-002"),
    }, trusted).unwrap();
    let mut recovered = HeldWorktreeLedger::reopen(
        &fixture.source, ledger.binding(), ledger.checkpoint(), trusted,
    ).unwrap();
    let later = fixture.write("file.txt", 4, 13);
    recovered.append(Event::Edits {
        operation: operation(13), expected: recovered.worktree().revision(), edits: vec![later],
    }, trusted).unwrap();
    assert_eq!(recovered.dirty_edits().len(), 1);
    let view = recovered.projection().unwrap();
    let plan = view.plan(|file| {
        assert_eq!(file, first.material.file_id);
        Ok::<_, ()>(ContainerPresence::Present {
            entity: storage_contract::working_tree::LocalEntityIdentity::new(80, 81),
        })
    }).unwrap();
    assert!(matches!(plan[0].transfer, Transfer::MoveWorking {
        container: ContainerPublication::Update {
            source_branch, source_version, source_node, ..
        }, ..
    } if source_branch == branch && source_version == first.material.version_id
        && source_node == storage_contract::FileNodeId::Version(first.material.version_id)));
    assert!(matches!(plan[1].transfer, Transfer::MoveWorking {
        container: ContainerPublication::Retained { .. }, ..
    }));
    recovered.append(delete_event(&recovered, 14), trusted).unwrap();
    let mut only_dirty = result_files(&recovered);
    only_dirty[0].source = SnapshotFileSource::Container {
        branch, node: storage_contract::FileNodeId::Version(only_dirty[0].material.version_id),
    };
    let new_publication = completed(&recovered);
    recovered.append(Event::Completed { publication: new_publication, files: only_dirty }, |_, files| {
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.as_str(), "file.txt");
        Ok(())
    }).unwrap();
    assert!(recovered.dirty_edits().is_empty());
    assert_eq!(recovered.file_results().len(), 2);
}
