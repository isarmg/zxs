use super::*;
use crate::directory_worktree::session::ReadMaterial;
use crate::persist::snapshot_worktree::{
    self as codec, NodeAllocation, NodeCompletion, NodeReservation, Record,
};
use crate::working_tree::snapshot::{
    SnapshotEntryContent, SnapshotFileSource,
};
use crate::{OperationResult, State};
use name_policy::{LookupPath, Name, PortablePath};
use std::{fs, io::Read, num::NonZeroU16, sync::Arc};
use storage_contract::snapshot_worktree::{
    CommitWorktree, CreateWorktree, EditWorktree, InitializeSnapshot,
    SnapshotHandle, SnapshotHost, SnapshotNamespace,
    WorktreeHandle, WorktreeId,
};
use storage_contract::working_tree::{
    SnapshotBranchId, SnapshotNodeId,
    SnapshotRepositoryId,
};
use storage_contract::{
    CommitState, ContentHash, FileId, Generation, OperationId, Principal,
};

const BODY: &[u8] = b"complete source, never a copied worktree";

#[cfg(feature = "fault-injection")]
#[path = "snapshot_worktree/published_commit_tail.rs"]
mod published_commit_tail;

#[test]
fn actual_empty_roots_publish_reopen_and_never_adopt_an_uncompleted_node() {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use crate::{CommitRequest, Config, Engine};
    use storage_contract::Mode;

    fn inventory(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        fn visit(base: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let key = path.strip_prefix(base).unwrap().to_path_buf();
                if entry.file_type().unwrap().is_dir() {
                    out.insert(key, None);
                    visit(base, &path, out);
                } else {
                    out.insert(key, Some(fs::read(path).unwrap()));
                }
            }
        }
        let mut out = BTreeMap::new();
        visit(root, root, &mut out);
        out
    }

    fn file_material(
        bytes: &[u8],
        scope: &crate::container_storage::ObjectDatabaseScope,
    ) -> crate::container_storage::ObjectDatabaseMaterial {
        // Inspect a detached copy of these exact captured bytes, not an active
        // container while the Engine retains its writer lease.
        let detached = tempfile::tempdir().unwrap();
        fs::set_permissions(detached.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = detached.path().join(crate::container_storage::file_database::NAME);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        crate::container_storage::inspect_object_database_material(
            fs::File::open(detached.path()).unwrap(),
            detached.path().to_path_buf(),
            scope,
        ).unwrap()
    }

    fn assert_cold_material(
        before: &BTreeMap<PathBuf, Option<Vec<u8>>>,
        after: &BTreeMap<PathBuf, Option<Vec<u8>>>,
        database: &Path,
        scope: &crate::container_storage::ObjectDatabaseScope,
        final_sequence: u64,
    ) {
        // Current cold repair can rematerialize FILE at its admitted SYSTEM
        // sequence. This is not zero-write: only that header may advance.
        // The strict reader still verifies the complete format/scope/row hash.
        let before_file = file_material(before[database].as_ref().unwrap(), scope);
        let after_file = file_material(after[database].as_ref().unwrap(), scope);
        assert_eq!(after_file.scope(), before_file.scope());
        assert_eq!(after_file.records_hash(), before_file.records_hash());
        assert_eq!(after_file.records(), before_file.records());
        assert!(before_file.source_sequence() <= after_file.source_sequence());
        assert_eq!(after_file.source_sequence(), final_sequence);
        let mut expected = before.clone();
        *expected.get_mut(database).unwrap() = after[database].clone();
        // Every directory, BODY, manifest, ledger and other file remains exact.
        assert_eq!(after, &expected);
    }

    fn original_request(engine: &Engine, repository: &crate::Repository) -> InitializeSnapshot {
        let root = engine.namespace_host_root(
            repository.repository_id.parse().unwrap(),
            repository.main_branch_id.parse().unwrap(),
        ).unwrap();
        InitializeSnapshot {
            host: SnapshotHost {
                root_id: root.binding().root_id,
                directory_id: root.binding().roots.common_data.directory_id,
                expected_root_generation: root.binding().generation,
            },
            source: SnapshotNamespace {
                branch_id: repository.main_branch_id.parse().unwrap(),
                root: None,
                expected_branch_generation: Generation::new(
                    engine.state().branches[&repository.main_branch_id].generation,
                ),
            },
            operation_id: OperationId::generate().unwrap(),
        }
    }

    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disks = [temporary.path().join("first"), temporary.path().join("second")];
    for path in [&system, &disks[0], &disks[1]] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(fs::read_dir(path).unwrap().count(), 0);
    }
    let config = Config::independent_roots(Mode::Development, [
        (Name::parse("First").unwrap(), disks[0].clone()),
        (Name::parse("Second").unwrap(), disks[1].clone()),
    ]).unwrap();
    let slots = config.disks.iter().map(|disk| disk.slot_id.clone()).collect::<Vec<_>>();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let first = engine.create_disk_root(&slots[0], "admin", OperationId::generate().unwrap())
        .unwrap();
    let second = engine.create_disk_root(&slots[1], "admin", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine.create_workspace(&first.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "original.txt", BODY).unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: "actual snapshot original".into(),
        request_digest: None,
    }).unwrap();
    let entry = engine.list_tree(&first.main_branch_id).unwrap().into_iter()
        .find(|entry| entry.path() == "original.txt").unwrap();
    // The real file-manager save already creates its lazy FILE branch through
    // the shared directory snapshot producer. Preserve that selected identity.
    let selected = engine.file_context(&first.repository_id, "original.txt").unwrap();
    assert_ne!(selected.branch_id, first.main_branch_id);
    let request = original_request(&engine, &first);
    let handle = initialize_root(&mut engine, &request, &principal()).unwrap();
    assert_ne!(handle.publication, request.operation_id);
    let reserve = engine.operation(request.operation_id).unwrap();
    let complete = engine.operation(handle.publication).unwrap();
    assert_eq!(reserve.kind, codec::NODE_RESERVE);
    assert_eq!(complete.kind, codec::NODE_COMPLETE);
    {
        let index = engine.snapshot_receipts().unwrap();
        let original = index.reopen_node(&engine, handle).unwrap();
        let captured = &original.manifest().entries()[0];
        assert_eq!(original.manifest().entries().len(), 1);
        assert_eq!(captured.path.as_str(), "original.txt");
        let SnapshotEntryContent::File {
            material, source: SnapshotFileSource::Container { branch, node },
        } = captured.content else {
            panic!("actual FILE source must not become SnapshotOnly");
        };
        assert_eq!(material.file_id.to_string(), entry.file_id);
        assert_eq!(material.version_id.to_string(), entry.version_id);
        assert_ne!(branch.to_string(), first.main_branch_id);
        assert_eq!(branch.to_string(), selected.branch_id);
        assert_eq!(engine.state().branches[&branch.to_string()].file_id.as_deref(),
            Some(entry.file_id.as_str()));
        let context = engine.file_context(&first.repository_id, "original.txt").unwrap();
        assert_eq!(context.branch_id, branch.to_string());
        assert_eq!(engine.file_node_version(&entry.file_id, &node.to_string()).unwrap(),
            entry.version_id);
        let expected_node = context.node_id.unwrap_or_else(|| {
            engine.file_branch_head(&context.branch_id).unwrap()
        });
        assert_eq!(node.to_string(), expected_node);
        let mut bytes = Vec::new();
        original.open_payload(material.version_id).unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, BODY);
    }
    let first_material = inventory(&disks[0].join("common-data"));
    assert!(!first_material.is_empty());
    let container = engine.state().file_containers
        .get(&entry.file_id.parse::<FileId>().unwrap()).unwrap();
    let database = PathBuf::from(container.namespace.name.as_str())
        .join(crate::container_storage::file_database::NAME);
    let file_scope = crate::container_storage::ObjectDatabaseScope::File {
        file_id: entry.file_id.clone(),
        history_tree_id: container.history_tree_id.to_string(),
    };
    drop(engine);

    engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    let cold_material = inventory(&disks[0].join("common-data"));
    assert_cold_material(&first_material, &cold_material, &database, &file_scope,
        engine.state().sequence);
    assert_eq!(engine.state().sequence, complete.sequence);
    let before = Arc::clone(&engine.state);
    assert_eq!(initialize_root(&mut engine, &request, &principal()).unwrap(), handle);
    assert!(Arc::ptr_eq(&before, &engine.state));
    assert_eq!(engine.operation(request.operation_id).unwrap(), reserve);
    assert_eq!(engine.operation(handle.publication).unwrap(), complete);
    assert_eq!(inventory(&disks[0].join("common-data")), cold_material);

    // Extend the same genuine flow: reserve an independent named instance,
    // create its sparse directory and ledger, then independently complete it.
    let worktree_request = CreateWorktree {
        source: handle, parent_directory_id: request.host.directory_id,
        name: Name::parse("draft-one").unwrap(),
        expected_root_generation: request.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(),
    };
    let worktree = create_worktree(&mut engine, &worktree_request, &principal()).unwrap();
    assert_ne!(worktree.publication, worktree_request.operation_id);
    let worktree_reserve = engine.operation(worktree_request.operation_id).unwrap();
    let worktree_complete = engine.operation(worktree.publication).unwrap();
    assert_eq!(worktree_reserve.kind, codec::WORKTREE_RESERVE);
    assert_eq!(worktree_complete.kind, codec::WORKTREE_COMPLETE);
    let observed = with_worktree(&engine, worktree, |session| {
        assert_eq!(session.instance().operation(), worktree_request.operation_id);
        assert_eq!(session.worktree().source().node, handle.node_id);
        assert_eq!(session.worktree().source().branch, handle.branch_id);
        assert_eq!(session.worktree().mount().name.as_str(), "draft-one");
        assert_eq!(session.worktree().mount().parent, request.host.directory_id);
        let view = session.read(ReadMaterial::Original).unwrap();
        let page = view.directory(None, None, NonZeroU16::new(8).unwrap()).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].path.as_str(), "original.txt");
        assert!(page.next.is_none());
        let mut bytes = Vec::new();
        view.open_file(&LookupPath::parse("original.txt").unwrap()).unwrap()
            .read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, BODY);
        Ok((session.instance(), session.worktree().clone(), view.stamp()))
    }).unwrap();
    {
        let index = engine.snapshot_receipts().unwrap();
        let original = index.reopen_node(&engine, handle).unwrap();
        assert_eq!(original.manifest().reference(), observed.0.source());
        let instance = original.worktrees_directory().path().join(
            crate::persist::layout::worktree::instance(worktree.worktree_id).as_path(),
        );
        let ledger = original.worktrees_directory().path().join(
            crate::persist::layout::worktree::ledger(worktree.worktree_id).as_path(),
        );
        assert_eq!(fs::read_dir(instance).unwrap().count(), 0);
        assert!(ledger.is_dir());
    }
    let material = inventory(&disks[0].join("common-data"));
    let before = Arc::clone(&engine.state);
    let mut collision = worktree_request.clone();
    collision.operation_id = OperationId::generate().unwrap();
    collision.name = Name::parse("DRAFT-ONE").unwrap();
    assert!(matches!(create_worktree(&mut engine, &collision, &principal()),
        Err(RegistrationError::Mount)));
    assert!(Arc::ptr_eq(&before, &engine.state));
    assert_eq!(inventory(&disks[0].join("common-data")), material);
    let mut changed_request = worktree_request.clone();
    changed_request.name = Name::parse("not-the-original-name").unwrap();
    assert!(matches!(create_worktree(&mut engine, &changed_request, &principal()),
        Err(RegistrationError::Digest(operation)) if operation == worktree_request.operation_id));
    drop(engine);

    engine = Engine::open(&system).unwrap();
    let cold_worktree_material = inventory(&disks[0].join("common-data"));
    assert_cold_material(&material, &cold_worktree_material, &database, &file_scope,
        engine.state().sequence);
    let before = Arc::clone(&engine.state);
    assert_eq!(create_worktree(&mut engine, &worktree_request, &principal()).unwrap(), worktree);
    assert!(Arc::ptr_eq(&before, &engine.state));
    assert_eq!(engine.operation(worktree_request.operation_id).unwrap(), worktree_reserve);
    assert_eq!(engine.operation(worktree.publication).unwrap(), worktree_complete);
    assert_eq!(engine.operation(handle.publication).unwrap(), complete);
    assert_eq!(with_worktree(&engine, worktree, |session| {
        let view = session.read(ReadMaterial::Original).unwrap();
        let page = view.directory(None, None, NonZeroU16::new(8).unwrap()).unwrap();
        assert_eq!(page.entries.len(), 1);
        let mut bytes = Vec::new();
        view.open_file(&LookupPath::parse("original.txt").unwrap()).unwrap()
            .read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, BODY);
        Ok((session.instance(), session.worktree().clone(), view.stamp()))
    }).unwrap(), observed);
    assert_eq!(inventory(&disks[0].join("common-data")), cold_worktree_material);

    // A durable worktree edit is not a lease workspace and does not publish FILE.
    let edited_body = b"one explicit worktree change, not an automatic FILE save";
    let edit = EditWorktree {
        target: worktree,
        path: PortablePath::parse("original.txt").unwrap(),
        expected: observed.1.revision(),
        size: edited_body.len() as u64,
        content_hash: ContentHash::from_digest(blake3::hash(edited_body)),
        operation_id: OperationId::generate().unwrap(),
    };
    let file_head = engine.file_branch_head(&selected.branch_id).unwrap();
    let original_versions = engine.state().versions.clone();
    let (_edit_source_directory, mut edit_source) = held_edit_body(edited_body);
    let revision = edit_worktree(&mut engine, &edit, &mut edit_source, &principal()).unwrap();
    assert_eq!(revision.generation, edit.expected.generation.next().unwrap());
    assert_ne!(revision.manifest, edit.expected.manifest);
    assert_eq!(engine.file_branch_head(&selected.branch_id).unwrap(), file_head);
    assert_eq!(engine.state().versions, original_versions);
    let read_edit = |engine: &Engine| {
        with_worktree(engine, worktree, |session| {
            assert_eq!(session.instance(), observed.0);
            assert_eq!(session.worktree().source(), observed.1.source());
            assert_eq!(session.worktree().mount(), observed.1.mount());
            assert_eq!(session.worktree().revision(), revision);
            let view = session.read(ReadMaterial::Original).unwrap();
            let mut bytes = Vec::new();
            view.open_file(&LookupPath::parse("original.txt").unwrap()).unwrap()
                .read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, edited_body);
            Ok(())
        }).unwrap();
        let index = engine.snapshot_receipts().unwrap();
        let original = index.reopen_node(engine, handle).unwrap();
        let mut bytes = Vec::new();
        original.open_payload(entry.version_id.parse().unwrap()).unwrap()
            .read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, BODY);
    };
    read_edit(&engine);
    let edited_material = inventory(&disks[0].join("common-data"));
    let edited_receipts = engine.state().operations.clone();
    drop(engine);
    engine = Engine::open(&system).unwrap();
    let cold_edited_material = inventory(&disks[0].join("common-data"));
    assert_cold_material(&edited_material, &cold_edited_material, &database, &file_scope,
        engine.state().sequence);
    read_edit(&engine);
    let before = Arc::clone(&engine.state);
    assert_eq!(
        edit_worktree(&mut engine, &edit, &mut edit_source, &principal()).unwrap(), revision,
    );
    assert!(Arc::ptr_eq(&before, &engine.state));
    assert_eq!(engine.state().operations, edited_receipts);
    assert_eq!(inventory(&disks[0].join("common-data")), cold_edited_material);
    assert_eq!(engine.file_branch_head(&selected.branch_id).unwrap(), file_head);
    assert_eq!(engine.state().versions, original_versions);

    // Explicit commit alone publishes FILE and a successor directory node.
    // The edited inode must MOVE, not be copied back into the sparse instance.
    let (edited_version, edited_inode, edited_path) = {
        use std::os::unix::fs::MetadataExt;
        let index = engine.snapshot_receipts().unwrap();
        let (_, completed) = index.completed_edit(worktree).unwrap();
        let original = index.reopen_node(&engine, handle).unwrap();
        let path = original.worktrees_directory().path().join(
            crate::persist::layout::worktree::instance(worktree.worktree_id).as_path(),
        ).join(completed.edit.leaf.entry().as_path());
        let metadata = fs::metadata(&path).unwrap();
        (completed.edit.material.version_id, (metadata.dev(), metadata.ino()), path)
    };
    assert!(!engine.state().versions.contains_key(&edited_version.to_string()));
    let directory_commit = CommitWorktree {
        target: worktree, expected: revision,
        message: "explicit directory commit".into(),
        operation_id: OperationId::generate().unwrap(),
    };
    // Even the same actor, branch and bytes do not identify our adapter.
    // A real pre-existing workspace must keep its lease and complete evidence.
    {
        let foreign = engine.create_workspace(&selected.branch_id, principal().as_str()).unwrap();
        engine.stage_bytes(&foreign.workspace_id, edit.path.as_str(), edited_body).unwrap();
        let foreign = engine.workspace(&foreign.workspace_id).unwrap();
        let foreign_bytes = crate::disk::encode_metadata(&foreign, crate::disk::MAX_METADATA)
            .unwrap();
        let before = Arc::clone(&engine.state);
        let slot = engine.next_slot;
        let error = commit_worktree(&mut engine, &directory_commit, &principal()).unwrap_err();
        let RegistrationError::Publication(error) = error else {
            panic!("another workspace must still reject the transient adapter");
        };
        assert_eq!(error.code(), storage_contract::ErrorCode::FileBusy);
        assert!(Arc::ptr_eq(&before, &engine.state));
        assert_eq!(engine.next_slot, slot);
        assert_eq!(crate::disk::encode_metadata(
            &engine.workspace(&foreign.workspace_id).unwrap(), crate::disk::MAX_METADATA,
        ).unwrap(), foreign_bytes);
        let staged = foreign.files.values().next().unwrap();
        let mut stored = Vec::new();
        engine.planning_context().staged_source(&foreign, staged).unwrap().open().unwrap()
            .read_to_end(&mut stored).unwrap();
        assert_eq!(stored, edited_body);
        assert_eq!(inventory(&disks[0].join("common-data")), cold_edited_material);
        read_edit(&engine);
        // Only the original owner explicitly releases that unrelated lease.
        engine.abort_workspace(&foreign.workspace_id, &foreign.principal).unwrap();
    }
    #[cfg(feature = "fault-injection")]
    {
        use std::os::unix::fs::MetadataExt;

        // Initial workspace creation does not publish a namespace admission.
        // The first decision is this commit's SYSTEM reserve: this real cut
        // occurs before PREPARE and before the local ledger enters Publishing.
        assert!(engine.admitted_current_namespaces().unwrap().is_none());
        let before = Arc::clone(&engine.state);
        let slot = engine.next_slot;
        let snapshot = engine.snapshot.clone();
        let previous_workspaces: std::collections::BTreeSet<_> =
            engine.workspaces.keys().cloned().collect();
        engine.inject_fault_once("after_payloads");
        let error = commit_worktree(&mut engine, &directory_commit, &principal()).unwrap_err();
        let RegistrationError::Publication(error) = error else {
            panic!("the real SYSTEM reserve must reach the injected decision cut");
        };
        assert_eq!(error.code(), storage_contract::ErrorCode::InjectedFailure);
        assert_eq!(error.message().as_ref(), "interrupted at after_payloads");
        assert!(Arc::ptr_eq(&before, &engine.state));
        assert_eq!(engine.next_slot, slot);
        assert_eq!(engine.snapshot, snapshot);
        assert_eq!(engine.state().operations, edited_receipts);
        assert!(!engine.state().operations.contains_key(&directory_commit.operation_id));
        assert_eq!(engine.file_branch_head(&selected.branch_id).unwrap(), file_head);
        assert_eq!(engine.state().versions, original_versions);
        assert_eq!(inventory(&disks[0].join("common-data")), cold_edited_material);
        let metadata = fs::metadata(&edited_path).unwrap();
        assert_eq!((metadata.dev(), metadata.ino()), edited_inode);
        read_edit(&engine);

        let adapters: Vec<_> = engine.workspaces.values()
            .filter(|workspace| !previous_workspaces.contains(&workspace.workspace_id))
            .collect();
        assert_eq!(adapters.len(), 1);
        let adapter = (*adapters[0]).clone();
        assert_eq!(adapter.state, storage_contract::WorkspaceState::Aborted);
        assert_eq!(adapter.files.len(), 1);
        let adapter_bytes = crate::disk::encode_metadata(&adapter, crate::disk::MAX_METADATA)
            .unwrap();
        let require_released_adapter = |engine: &Engine| {
            let actual = engine.workspace(&adapter.workspace_id).unwrap();
            assert_eq!(crate::disk::encode_metadata(&actual, crate::disk::MAX_METADATA).unwrap(),
                adapter_bytes);
            let staged = actual.files.values().next().unwrap();
            let mut stored = Vec::new();
            engine.planning_context().staged_source(&actual, staged).unwrap().open().unwrap()
                .read_to_end(&mut stored).unwrap();
            assert_eq!(stored, edited_body);
        };
        require_released_adapter(&engine);

        // Returned pre-PREPARE failure durably closes only its own adapter.
        // Its upload and prepared SYSTEM objects remain, but its lease cannot
        // block the same explicit request after a real cold reopen.
        drop(engine);
        engine = Engine::open(&system).unwrap();
        assert!(!engine.is_read_only());
        require_released_adapter(&engine);
        assert_eq!(engine.state().operations, edited_receipts);
        assert_eq!(engine.file_branch_head(&selected.branch_id).unwrap(), file_head);
        assert_eq!(engine.state().versions, original_versions);
        assert_eq!(inventory(&disks[0].join("common-data")), cold_edited_material);
        let metadata = fs::metadata(&edited_path).unwrap();
        assert_eq!((metadata.dev(), metadata.ino()), edited_inode);
        read_edit(&engine);
    }
    // The same explicit request now publishes exactly one FILE successor and
    // one directory node, retaining the original source and worktree identity.
    #[cfg(feature = "fault-injection")]
    {
        engine = published_commit_tail::interrupt_and_resume(
            engine, &system, &disks, &directory_commit, &principal(),
        );
    }
    let updated = commit_worktree(&mut engine, &directory_commit, &principal()).unwrap();
    assert_eq!(updated.repository_id, handle.repository_id);
    assert_eq!(updated.branch_id, handle.branch_id);
    assert_ne!(updated.node_id, handle.node_id);
    assert_eq!(engine.state().versions.len(), original_versions.len() + 1);
    assert_eq!(engine.file_branch_head(&selected.branch_id).unwrap(), edited_version.to_string());
    assert_eq!(engine.state().versions[&edited_version.to_string()].source_version_ids,
        vec![entry.version_id.clone()]);
    assert_eq!(engine.state().versions[&entry.version_id], original_versions[&entry.version_id]);
    assert!(!edited_path.exists());
    let read_committed = |engine: &Engine| {
        use std::os::unix::fs::MetadataExt;
        let index = engine.snapshot_receipts().unwrap();
        let original = index.reopen_node(engine, handle).unwrap();
        let successor = index.reopen_node(engine, updated).unwrap();
        let session = index.worktree(worktree).unwrap().reopen(&original).unwrap();
        assert_eq!(session.instance(), observed.0);
        assert_eq!(session.worktree().source(), observed.1.source());
        assert_eq!(session.worktree().revision(), revision);
        let view = session.read(ReadMaterial::Published(&successor)).unwrap();
        let mut read = view.open_file(&LookupPath::parse("original.txt").unwrap()).unwrap();
        let metadata = read.metadata().unwrap();
        assert_eq!((metadata.dev(), metadata.ino()), edited_inode);
        let mut bytes = Vec::new();
        read.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, edited_body);
        let source = successor.manifest().entries()[0].content;
        assert!(matches!(source, SnapshotEntryContent::File {
            material, source: SnapshotFileSource::Container { branch, node },
        } if material.version_id == edited_version && material.file_id.to_string() == entry.file_id
            && branch.to_string() == selected.branch_id
            && node == storage_contract::FileNodeId::Version(edited_version)));
        let mut bytes = Vec::new();
        original.open_payload(entry.version_id.parse().unwrap()).unwrap()
            .read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, BODY);
        for (version, expected) in [
            (entry.version_id.clone(), BODY), (edited_version.to_string(), edited_body.as_slice()),
        ] {
            let mut opened = engine.open_content(&version).unwrap();
            let mut bytes = Vec::new();
            opened.file.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, expected);
        }
    };
    read_committed(&engine);
    let committed_material = inventory(&disks[0].join("common-data"));
    let committed_receipts = engine.state().operations.clone();
    drop(engine);
    engine = Engine::open(&system).unwrap();
    let cold_committed_material = inventory(&disks[0].join("common-data"));
    assert_cold_material(&committed_material, &cold_committed_material, &database, &file_scope,
        engine.state().sequence);
    read_committed(&engine);
    let before = Arc::clone(&engine.state);
    assert_eq!(commit_worktree(&mut engine, &directory_commit, &principal()).unwrap(), updated);
    assert!(Arc::ptr_eq(&before, &engine.state));
    assert_eq!(engine.state().operations, committed_receipts);
    assert_eq!(inventory(&disks[0].join("common-data")), cold_committed_material);
    assert!(!edited_path.exists());

    // The same actual container now gets a shared manual body. A lease
    // workspace is not created or retained for any of these draft writes.
    {
        use storage_contract::file_draft::{
            CommitMode, DraftCommand, DraftLocation, FileWorkingState,
        };
        use std::io::{Seek, SeekFrom, Write};
        fn input(bytes: &[u8]) -> crate::ReadFile {
            let mut file = tempfile::tempfile().unwrap();
            file.write_all(bytes).unwrap();
            file.seek(SeekFrom::Start(0)).unwrap();
            file.into()
        }
        fn read_draft(engine: &Engine, target: &DraftLocation) -> Vec<u8> {
            let mut bytes = Vec::new();
            engine.open_file_draft(target).unwrap().read_to_end(&mut bytes).unwrap();
            bytes
        }
        let target = DraftLocation {
            repository: first.repository_id.parse().unwrap(),
            path: PortablePath::parse("original.txt").unwrap(),
            entry: entry.entry_id.parse().unwrap(), file: entry.file_id.parse().unwrap(),
        };
        let versions_before = engine.state().versions.clone();
        let branch_before = engine.state().branches[&selected.branch_id].clone();
        let workspace_count = engine.workspaces.len();
        let mode = DraftCommand::SetMode {
            target: target.clone(),
            expected: engine.file_working_state(&target).unwrap().revision(),
            mode: CommitMode::Manual,
        };
        let mode_operation = OperationId::generate().unwrap();
        let mode_receipt = engine.change_file_working(&mode, &principal(), mode_operation).unwrap();
        assert_eq!(engine.state().branches[&selected.branch_id], branch_before);
        let base = engine.file_draft_base(&target).unwrap();
        let draft_bytes = b"shared manual body, not an immutable history node";
        let write = DraftCommand::Write {
            target: target.clone(),
            expected: engine.file_working_state(&target).unwrap().revision(),
            base, content_hash: ContentHash::from_digest(blake3::hash(draft_bytes)),
            logical_size: draft_bytes.len() as u64,
        };
        let write_operation = OperationId::generate().unwrap();
        #[cfg(feature = "fault-injection")]
        let orphan = {
            let drafts = disks[0].join("common-data/original.txt/drafts");
            assert!(!drafts.exists(), "metadata-only mode does not create draft storage");
            let before = Arc::clone(&engine.state);
            engine.inject_fault_once("after_payloads");
            let error = engine.write_file_draft(
                &write, &principal(), write_operation, input(draft_bytes),
            ).unwrap_err();
            assert_eq!(error.code(), storage_contract::ErrorCode::InjectedFailure);
            assert!(error.to_string().contains("after_payloads"));
            assert!(Arc::ptr_eq(&before, &engine.state));
            assert!(!engine.state().operations.contains_key(&write_operation));
            assert_eq!(engine.state().branches[&selected.branch_id], branch_before);
            let added = fs::read_dir(&drafts).unwrap().map(|item| item.unwrap().file_name())
                .collect::<Vec<_>>();
            assert_eq!(added.len(), 1);
            let orphan = drafts.join(&added[0]);
            assert_eq!(fs::read(&orphan).unwrap(), draft_bytes);
            orphan
        };
        // Same command/operation after a pre-PREPARE failure creates a fresh
        // payload; the earlier exclusive object is retained, never adopted.
        let written = engine.write_file_draft(
            &write, &principal(), write_operation, input(draft_bytes),
        ).unwrap();
        #[cfg(feature = "fault-injection")]
        {
            let current = engine.state().file_containers[&target.file].working.draft()
                .unwrap().1.payload_id();
            assert_ne!(orphan.file_name().unwrap().to_str().unwrap(), current.to_string());
            assert_eq!(fs::read(&orphan).unwrap(), draft_bytes);
        }
        let dirty = engine.file_working_state(&target).unwrap();
        assert!(matches!(dirty,
            FileWorkingState::ManualDirty { base: actual, .. } if actual == base));
        assert_eq!(engine.workspaces.len(), workspace_count);
        assert_eq!(engine.state().branches[&selected.branch_id], branch_before);
        assert_eq!(engine.state().versions, versions_before);
        assert_eq!(read_draft(&engine, &target), draft_bytes);
        {
            // This source retained the actual published inode. Equal bytes at
            // the same name cannot replace that live observation. Cold draft
            // identity remains payload ID/hash/size, not a persistent inode.
            let source = engine.planning_context().draft_payload_source(target.file).unwrap();
            let payload = dirty.draft().unwrap().1.payload_id();
            let path = disks[0].join("common-data/original.txt/drafts").join(payload.to_string());
            let parked = temporary.path().join("held-draft-parked");
            fs::rename(&path, &parked).unwrap();
            fs::write(&path, draft_bytes).unwrap();
            let error = source.open().err().unwrap();
            assert!(matches!(error,
                crate::Error::FreshLayout(crate::engine::FreshLayoutError::DraftChanged)));
            drop(source);
            fs::remove_file(&path).unwrap();
            fs::rename(&parked, &path).unwrap();
            assert_eq!(read_draft(&engine, &target), draft_bytes);
        }
        read_committed(&engine);
        let dirty_media = inventory(&disks[0].join("common-data"));
        let before = Arc::clone(&engine.state);
        let wrong_mode = DraftCommand::SetMode {
            target: target.clone(), expected: dirty.revision(), mode: CommitMode::Automatic,
        };
        assert!(matches!(engine.change_file_working(
            &wrong_mode, &principal(), OperationId::generate().unwrap(),
        ), Err(crate::Error::FileDraft(crate::DraftError::Dirty))));
        assert!(Arc::ptr_eq(&before, &engine.state));
        assert_eq!(inventory(&disks[0].join("common-data")), dirty_media);
        drop(engine);
        engine = Engine::open(&system).unwrap();
        let cold_dirty_media = inventory(&disks[0].join("common-data"));
        assert_cold_material(&dirty_media, &cold_dirty_media, &database, &file_scope,
            engine.state().sequence);
        assert!(!engine.is_read_only());
        assert_eq!(engine.file_working_state(&target).unwrap(), dirty);
        assert_eq!(read_draft(&engine, &target), draft_bytes);
        assert_eq!(engine.state().branches[&selected.branch_id], branch_before);
        assert_eq!(engine.state().versions, versions_before);
        assert_eq!(engine.operation(mode_operation).unwrap(), mode_receipt);
        let before = Arc::clone(&engine.state);
        // The complete receipt returns before looking at these unrelated bytes.
        assert_eq!(engine.write_file_draft(
            &write, &principal(), write_operation, input(b"must not be consumed"),
        ).unwrap(), written);
        assert!(Arc::ptr_eq(&before, &engine.state));
        assert_eq!(inventory(&disks[0].join("common-data")), cold_dirty_media);
        let commit = DraftCommand::Commit {
            target: target.clone(), expected: dirty.revision(), base,
            message: "explicit shared container draft commit".into(),
        };
        let commit_operation = OperationId::generate().unwrap();
        let committed = engine.change_file_working(
            &commit, &principal(), commit_operation,
        ).unwrap();
        let version = engine.file_branch_head(&selected.branch_id).unwrap();
        assert_ne!(version, edited_version.to_string());
        assert_eq!(engine.state().versions.len(), versions_before.len() + 1);
        assert_eq!(engine.state().versions[&version].source_version_ids,
            vec![edited_version.to_string()]);
        assert_eq!(engine.file_node_parents(&entry.file_id, &version).unwrap(),
            vec![base.node_id().to_string()]);
        assert!(matches!(engine.file_working_state(&target).unwrap(),
            FileWorkingState::ManualClean { revision }
                if revision == dirty.revision().next().unwrap()));
        let mut bytes = Vec::new();
        engine.open_content(&version).unwrap().file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, draft_bytes);
        read_committed(&engine);
        let material = inventory(&disks[0].join("common-data"));
        drop(engine);
        engine = Engine::open(&system).unwrap();
        let cold_manual_material = inventory(&disks[0].join("common-data"));
        assert_cold_material(&material, &cold_manual_material, &database, &file_scope,
            engine.state().sequence);
        assert!(!engine.is_read_only());
        let before = Arc::clone(&engine.state);
        assert_eq!(engine.change_file_working(&commit, &principal(), commit_operation).unwrap(),
            committed);
        assert!(Arc::ptr_eq(&before, &engine.state));
        assert_eq!(engine.file_branch_head(&selected.branch_id).unwrap(), version);
        assert_eq!(inventory(&disks[0].join("common-data")), cold_manual_material);
        let mut bytes = Vec::new();
        engine.open_content(&version).unwrap().file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, draft_bytes);

        #[cfg(feature = "fault-injection")]
        {
            let interrupted_body = b"accepted immutable draft that must survive cold recovery";
            let interrupted = DraftCommand::Write {
                target: target.clone(),
                expected: engine.file_working_state(&target).unwrap().revision(),
                base: engine.file_draft_base(&target).unwrap(),
                content_hash: ContentHash::from_digest(blake3::hash(interrupted_body)),
                logical_size: interrupted_body.len() as u64,
            };
            let operation = OperationId::generate().unwrap();
            let drafts = disks[0].join("common-data/original.txt/drafts");
            let old = fs::read_dir(&drafts).unwrap().map(|item| item.unwrap().file_name())
                .collect::<std::collections::BTreeSet<_>>();
            let before = Arc::clone(&engine.state);
            engine.inject_fault_once("after_accept_1");
            let error = engine.write_file_draft(
                &interrupted, &principal(), operation, input(interrupted_body),
            ).unwrap_err();
            assert_eq!(error.code(), storage_contract::ErrorCode::InDoubt);
            let mut cause: &dyn std::error::Error = &error;
            let mut actual_cut = false;
            while let Some(source) = cause.source() {
                actual_cut |= source.to_string().contains("after_accept_1");
                cause = source;
            }
            assert!(actual_cut, "the actual Accept cut must have been reached");
            assert!(Arc::ptr_eq(&before, &engine.state));
            let added = fs::read_dir(&drafts).unwrap().map(|item| item.unwrap().file_name())
                .filter(|name| !old.contains(name)).collect::<Vec<_>>();
            assert_eq!(added.len(), 1);
            added[0].to_str().unwrap().parse::<
                storage_contract::file_draft::DraftPayloadId,
            >().unwrap();
            let payload = drafts.join(&added[0]);
            assert_eq!(fs::read(&payload).unwrap(), interrupted_body);
            drop(engine);
            let parked = temporary.path().join("accepted-draft-parked");
            fs::rename(&payload, &parked).unwrap();
            let missing_system = inventory(&system);
            let missing_disks = disks.iter().map(|disk| inventory(disk)).collect::<Vec<_>>();
            assert!(Engine::open(&system).is_err(), "missing decided draft must not be published");
            assert_eq!(inventory(&system), missing_system);
            assert_eq!(disks.iter().map(|disk| inventory(disk)).collect::<Vec<_>>(), missing_disks);
            fs::rename(&parked, &payload).unwrap();
            fs::write(&payload, b"corrupted accepted draft").unwrap();
            let corrupt_system = inventory(&system);
            let corrupt_disks = disks.iter().map(|disk| inventory(disk)).collect::<Vec<_>>();
            assert!(Engine::open(&system).is_err(), "bad decided bytes must not be published");
            assert_eq!(inventory(&system), corrupt_system);
            assert_eq!(disks.iter().map(|disk| inventory(disk)).collect::<Vec<_>>(), corrupt_disks);
            fs::write(&payload, interrupted_body).unwrap();
            engine = Engine::open(&system).unwrap();
            assert!(!engine.is_read_only());
            assert_eq!(engine.operation(operation).unwrap().commit_state, CommitState::Published);
            let before = Arc::clone(&engine.state);
            engine.write_file_draft(
                &interrupted, &principal(), operation, input(b"not read on retry"),
            ).unwrap();
            assert!(Arc::ptr_eq(&before, &engine.state));
            assert_eq!(read_draft(&engine, &target), interrupted_body);
            assert_eq!(engine.file_branch_head(&selected.branch_id).unwrap(), version);
            let discard = DraftCommand::Discard {
                target: target.clone(),
                expected: engine.file_working_state(&target).unwrap().revision(),
            };
            engine.change_file_working(&discard, &principal(), OperationId::generate().unwrap())
                .unwrap();
            assert_eq!(fs::read(&payload).unwrap(), interrupted_body);
        }
    }

    // Cut after real reserve and complete physical node preparation, before the
    // metadata completion. This uses the real producer, not forged receipt rows.
    let pending = original_request(&engine, &second);
    let host = prepare_namespace_host(
        &mut engine, &pending.source, pending.host.root_id,
        pending.host.expected_root_generation, pending.operation_id,
    ).unwrap();
    let allocated = NodeAllocation {
        repository: SnapshotRepositoryId::generate().unwrap(),
        branch: SnapshotBranchId::generate().unwrap(),
        node: SnapshotNodeId::generate().unwrap(),
        completion: OperationId::generate().unwrap(),
    };
    let (reserved, manifest) = publication::reserve(
        &mut engine, &host, &pending, allocated, &principal(),
    ).unwrap();
    let measured = crate::engine::scope_closure::measure_initial_directory_reservation(&engine, allocated.repository).unwrap();
    assert_eq!(measured.0, pending.operation_id);
    assert!(crate::engine::scope_closure::measure_initial_directory_reservation(
        &engine, SnapshotRepositoryId::generate().unwrap()).is_err());
    let prepared = host.prepare_node(&mut engine, reserved, manifest).unwrap();
    // A real staged node now exists under this host. Its bytes cannot
    // silently promote or change the original unbound reservation root.
    assert_eq!(crate::engine::scope_closure::measure_initial_directory_reservation(
        &engine, allocated.repository).unwrap(), measured);
    prepared.require_engine(&engine).unwrap();
    let pending_receipt = engine.operation(pending.operation_id).unwrap();
    let pending_material = inventory(&disks[1].join("common-data"));
    assert!(!pending_material.is_empty());
    drop(prepared);
    drop(engine);

    engine = Engine::open(&system).unwrap();
    assert_eq!(crate::engine::scope_closure::measure_initial_directory_reservation(
        &engine, allocated.repository).unwrap(), measured);
    let before = Arc::clone(&engine.state);
    let error = initialize_root(&mut engine, &pending, &principal()).unwrap_err();
    assert!(matches!(error, RegistrationError::Pending(operation)
        if operation == pending.operation_id));
    assert!(Arc::ptr_eq(&before, &engine.state));
    assert_eq!(engine.operation(pending.operation_id).unwrap(), pending_receipt);
    assert!(!engine.state().operations.contains_key(&allocated.completion));
    assert_eq!(inventory(&disks[1].join("common-data")), pending_material);
}

// These controls use ordinary SYSTEM publications, not constructed receipt
// bytes as a source. Candidate mutations below are never installed or written.
struct LiveFixture {
    _temp: tempfile::TempDir,
    system: std::path::PathBuf,
    engine: crate::Engine,
    request: InitializeSnapshot,
}

fn principal() -> Principal {
    "admin".parse().unwrap()
}

impl LiveFixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let system = temporary.path().join("system");
        let mut engine = crate::Engine::initialize(&system, crate::Config::development(
            crate::test_disks([temporary.path().join("primary")]),
        ).unwrap()).unwrap();
        let repository = engine.create_disk_root(
            "P01", "admin", OperationId::generate().unwrap(),
        ).unwrap();
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "original.txt", BODY).unwrap();
        engine.stage_bytes(&workspace.workspace_id, "second.txt", b"independent root file")
            .unwrap();
        engine.commit_workspace(crate::CommitRequest {
            workspace_id: workspace.workspace_id, operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(), expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token, message: "real receipt source".into(),
            request_digest: None,
        }).unwrap();
        let root = engine.snapshot_root_context(repository.repository_id.parse().unwrap()).unwrap();
        let request = InitializeSnapshot {
            host: root.host, source: root.source, operation_id: OperationId::generate().unwrap(),
        };
        Self { _temp: temporary, system, engine, request }
    }

    fn reserve(&mut self) -> (
        crate::container_storage::namespace_host::PreparedNamespaceNode, NodeReservation,
    ) {
        let host = prepare_namespace_host(
            &mut self.engine, &self.request.source, self.request.host.root_id,
            self.request.host.expected_root_generation, self.request.operation_id,
        ).unwrap();
        let allocated = NodeAllocation {
            repository: SnapshotRepositoryId::generate().unwrap(),
            branch: SnapshotBranchId::generate().unwrap(),
            node: SnapshotNodeId::generate().unwrap(),
            completion: OperationId::generate().unwrap(),
        };
        let (reserved, manifest) = publication::reserve(
            &mut self.engine, &host, &self.request, allocated, &principal(),
        ).unwrap();
        let result = self.engine.operation(self.request.operation_id).unwrap();
        let Record::ReserveNode(plan) = codec::decode(&result).unwrap().unwrap().record else {
            panic!("the actual producer published the node reservation");
        };
        let node = host.prepare_node(&mut self.engine, reserved, manifest).unwrap();
        (node, plan)
    }

    fn publish_node(&mut self) -> SnapshotHandle {
        initialize_root(&mut self.engine, &self.request, &principal()).unwrap()
    }

    fn create(&mut self, node: SnapshotHandle) -> (CreateWorktree, WorktreeHandle) {
        let request = CreateWorktree {
            source: node, parent_directory_id: self.request.host.directory_id,
            name: Name::parse("worktree-main-001").unwrap(),
            expected_root_generation: self.request.host.expected_root_generation,
            operation_id: OperationId::generate().unwrap(),
        };
        let tree = create_worktree(&mut self.engine, &request, &principal()).unwrap();
        (request, tree)
    }

    fn cold(self) -> Self {
        let Self { _temp, system, engine, request } = self;
        drop(engine);
        let engine = crate::Engine::open(&system).unwrap();
        Self { _temp, system, engine, request }
    }
}

fn planned_record(
    engine: &crate::Engine, operation: OperationId, actor: &Principal, record: &Record,
) -> (State, OperationResult) {
    let encoded = codec::PublicationEncoding::new(record).unwrap();
    let plan = engine.plan_mutation_digest(
        record.kind(), actor.as_str(), operation, encoded.request(), None,
        |_, _, _| Ok(encoded.result()),
    ).unwrap();
    let crate::engine::MutationPlan::Prepared { next, prepared, result } = plan else {
        panic!("the actual fresh planner produced one prospective record");
    };
    assert!(prepared.is_empty());
    (*next, result)
}

#[test]
fn exact_receipt_chain_reopens_actual_empty_instance_and_original_material() {
    let mut fixture = LiveFixture::new();
    let node = fixture.publish_node();
    let (request, handle) = fixture.create(node);
    let receipts = fixture.engine.state().operations.clone();
    let mut fixture = fixture.cold();
    let index = fixture.engine.snapshot_receipts().unwrap();
    assert!(std::ptr::eq(index.source(), fixture.engine.state()));
    let registered = index.worktree(handle).unwrap();
    assert_eq!(registered.handle(), handle);
    assert_eq!(registered.source().handle(), node);
    let reopened = index.reopen_node(&fixture.engine, node).unwrap();
    let session = registered.reopen(&reopened).unwrap();
    let instance = reopened.worktrees_directory().path().join(
        crate::persist::layout::worktree::instance(handle.worktree_id).as_path(),
    );
    assert_eq!(fs::read_dir(instance).unwrap().count(), 0);
    let view = session.read(ReadMaterial::Original).unwrap();
    let page = view.directory(None, None, NonZeroU16::new(2).unwrap()).unwrap();
    assert_eq!(page.entries.len(), 2);
    assert!(page.next.is_none());
    let mut actual = Vec::new();
    view.open_file(&LookupPath::parse("original.txt").unwrap()).unwrap()
        .read_to_end(&mut actual).unwrap();
    assert_eq!(actual, BODY);
    drop(view);
    drop(session);
    drop(registered);
    drop(index);
    assert_eq!(create_worktree(&mut fixture.engine, &request, &principal()).unwrap(), handle);
    assert_eq!(fixture.engine.state().operations, receipts);
}

#[test]
fn existing_physical_material_does_not_complete_a_reservation_or_rebind_its_ledger() {
    let mut pending = LiveFixture::new();
    let (physical, plan) = pending.reserve();
    let handle = SnapshotHandle {
        publication: plan.allocated.completion, repository_id: plan.allocated.repository,
        branch_id: plan.allocated.branch, node_id: plan.allocated.node,
    };
    physical.require_engine(&pending.engine).unwrap();
    assert!(matches!(pending.engine.snapshot_receipts().unwrap().node(handle),
        Err(RegistrationError::Pending(operation)) if operation == pending.request.operation_id));
    let receipts = pending.engine.state().operations.clone();
    drop(physical);
    let mut pending = pending.cold();
    assert!(matches!(initialize_root(&mut pending.engine, &pending.request, &principal()),
        Err(RegistrationError::Pending(operation)) if operation == pending.request.operation_id));
    assert_eq!(pending.engine.state().operations, receipts);
    assert!(!pending.engine.state().operations.contains_key(&handle.publication));

    let mut published = LiveFixture::new();
    let original = published.publish_node();
    let (_, tree) = published.create(original);
    let published = published.cold();
    let mut foreign = LiveFixture::new();
    let foreign_handle = foreign.publish_node();
    let foreign_index = foreign.engine.snapshot_receipts().unwrap();
    let foreign_node = foreign_index.reopen_node(&foreign.engine, foreign_handle).unwrap();
    let index = published.engine.snapshot_receipts().unwrap();
    let registered = index.worktree(tree).unwrap();
    // Both nodes hold the same complete BODY bytes, but their actual creation,
    // source binding and ledger ownership are independent permanent decisions.
    assert!(matches!(registered.reopen(&foreign_node), Err(RegistrationError::Source)));
    let actual = index.reopen_node(&published.engine, original).unwrap();
    registered.reopen(&actual).unwrap();
}

#[test]
fn completion_owner_digest_events_and_allocated_identity_are_not_replaceable() {
    #[cfg(feature = "fault-injection")]
    {
        // This reserve and EXCL node are actual publications/material. Only
        // its metadata completion is cut at ACCEPT; no local success is used
        // to manufacture a missing original or installed receipt source.
        let mut pending = LiveFixture::new();
        let (physical, plan) = pending.reserve();
        let before = Arc::clone(&pending.engine.state);
        pending.engine.inject_fault_once("after_accept_1");
        let error = publication::complete(&mut pending.engine, &physical).unwrap_err();
        assert!(matches!(error, RegistrationError::Publication(crate::Error::Context {
            code: storage_contract::ErrorCode::InDoubt, source, ..
        }) if source.code() == storage_contract::ErrorCode::InjectedFailure));
        assert!(Arc::ptr_eq(&before, &pending.engine.state));
        assert!(!pending.engine.state().operations.contains_key(&plan.allocated.completion));
        drop(physical);
        let mut pending = pending.cold();
        let node = pending.publish_node();
        assert_eq!(node.publication, plan.allocated.completion);
        let index = pending.engine.snapshot_receipts().unwrap();
        index.reopen_node(&pending.engine, node).unwrap();
        assert_eq!(pending.engine.state().versions, before.versions);
        assert_eq!(pending.engine.state().operations.len(), before.operations.len() + 1);
        let published = Arc::clone(&pending.engine.state);
        assert_eq!(pending.publish_node(), node);
        assert!(Arc::ptr_eq(&published, &pending.engine.state));
    }
    let mut fixture = LiveFixture::new();
    let (physical, plan) = fixture.reserve();
    let completion = NodeCompletion {
        reserve: plan.request.operation_id,
        reservation: Record::ReserveNode(plan.clone()).commitment().unwrap(),
        binding: physical.binding(),
    };
    let record = Record::CompleteNode(completion);
    let receipts = fixture.engine.snapshot_receipt_source().unwrap();
    let index = receipts.index().unwrap();
    let before = fixture.engine.state().clone();
    let (next, result) = planned_record(
        &fixture.engine, plan.allocated.completion, &principal(), &record,
    );
    index.require_planned(&next, &result, &principal(), &record).unwrap();

    let mut wrong_completion = completion;
    wrong_completion.reserve = OperationId::generate().unwrap();
    let wrong_record = Record::CompleteNode(wrong_completion);
    let (wrong, result) = planned_record(
        &fixture.engine, plan.allocated.completion, &principal(), &wrong_record,
    );
    assert!(matches!(index.require_planned(&wrong, &result, &principal(), &wrong_record),
        Err(RegistrationError::Completion(_))));
    let other: Principal = "another author".parse().unwrap();
    let (wrong, result) = planned_record(
        &fixture.engine, plan.allocated.completion, &other, &record,
    );
    assert!(matches!(index.require_planned(&wrong, &result, &other, &record),
        Err(RegistrationError::Completion(_))));

    let (mut wrong, mut result) = planned_record(
        &fixture.engine, plan.allocated.completion, &principal(), &record,
    );
    result.request_digest = before.operations[&plan.request.operation_id].request_digest.clone();
    wrong.operations.insert(result.operation_id, result.clone());
    assert!(matches!(index.require_planned(&wrong, &result, &principal(), &record),
        Err(RegistrationError::PublicationChanged)));
    let (mut wrong, result) = planned_record(
        &fixture.engine, plan.allocated.completion, &principal(), &record,
    );
    wrong.events.retain(|event| event.operation_id != plan.request.operation_id);
    assert!(matches!(index.require_planned(&wrong, &result, &principal(), &record),
        Err(RegistrationError::PublicationChanged)));

    let mut duplicate = plan.clone();
    duplicate.request.operation_id = OperationId::generate().unwrap();
    duplicate.allocated.completion = OperationId::generate().unwrap();
    let duplicate = Record::ReserveNode(duplicate);
    let Record::ReserveNode(ref duplicate_plan) = duplicate else { panic!("node plan"); };
    let (wrong, result) = planned_record(
        &fixture.engine, duplicate_plan.request.operation_id, &principal(), &duplicate,
    );
    assert!(matches!(index.require_planned(&wrong, &result, &principal(), &duplicate),
        Err(RegistrationError::Identity)));
    assert_eq!(fixture.engine.state(), &before);
    physical.require_engine(&fixture.engine).unwrap();
    #[cfg(not(feature = "fault-injection"))]
    let node = publication::complete(&mut fixture.engine, &physical).unwrap();
    #[cfg(feature = "fault-injection")]
    let node = {
        let state = std::sync::Arc::clone(&fixture.engine.state);
        fixture.engine.inject_fault_once("after_local_publications");
        let error = publication::complete(&mut fixture.engine, &physical).unwrap_err();
        assert!(matches!(error, RegistrationError::Publication(crate::Error::Context {
            code: storage_contract::ErrorCode::InDoubt, source, ..
        }) if source.code() == storage_contract::ErrorCode::InjectedFailure));
        assert!(fixture.engine.blocked);
        assert!(std::sync::Arc::ptr_eq(&state, &fixture.engine.state));
        assert!(!fixture.engine.state().operations.contains_key(&plan.allocated.completion));
        // It is already durably published, not a no-write failure. A fresh
        // journal/barrier traversal supplies the original and visible result.
        drop(physical);
        fixture = fixture.cold();
        let handle = initialize_root(&mut fixture.engine, &fixture.request, &principal()).unwrap();
        assert_eq!(handle.publication, plan.allocated.completion);
        assert_eq!(fixture.engine.state().versions, before.versions);
        assert_eq!(fixture.engine.state().operations.len(), before.operations.len() + 1);
        handle
    };
    let (request, _) = fixture.create(node);
    let published = fixture.engine.operation(request.operation_id).unwrap();
    let Record::ReserveWorktree(mut duplicate) = codec::decode(&published).unwrap().unwrap().record
    else { panic!("actual worktree reservation"); };
    duplicate.request.operation_id = OperationId::generate().unwrap();
    duplicate.allocated.completion = OperationId::generate().unwrap();
    duplicate.allocated.worktree = WorktreeId::generate().unwrap();
    duplicate.request.name = Name::parse("WORKTREE-MAIN-001").unwrap();
    let operation = duplicate.request.operation_id;
    let record = Record::ReserveWorktree(duplicate);
    let (wrong, result) = planned_record(&fixture.engine, operation, &principal(), &record);
    assert!(matches!(fixture.engine.snapshot_receipts().unwrap()
        .require_planned(&wrong, &result, &principal(), &record), Err(RegistrationError::Mount)));
}

#[test]
fn typed_receipt_rejects_retag_unknown_fields_oversize_and_unverified_recovery_overlay() {
    let mut fixture = LiveFixture::new();
    let node = fixture.publish_node();
    fixture.engine.snapshot_receipts().unwrap().reopen_node(&fixture.engine, node).unwrap();
    let receipt = fixture.engine.operation(fixture.request.operation_id).unwrap();
    let mut wrong = receipt.clone();
    wrong.kind = "file.note".into();
    assert!(matches!(codec::decode(&wrong), Err(codec::CodecError::Kind)));
    let mut wrong = receipt.clone();
    wrong.result["snapshot_worktree"]["record"]["unrecognized"] = json_codec::json!(true);
    assert!(matches!(codec::decode(&wrong), Err(codec::CodecError::Decoding(_))));
    let mut wrong = receipt.clone();
    wrong.result["snapshot_worktree"]["record"]["entries"] = json_codec::Value::Array(
        vec![json_codec::Value::Null; crate::working_tree::snapshot::MAX_ENTRIES + 1],
    );
    assert!(matches!(codec::decode(&wrong), Err(codec::CodecError::Budget)));
    let mut wrong = receipt.clone();
    wrong.commit_state = CommitState::PublishedDegraded;
    assert!(matches!(codec::decode(&wrong), Err(codec::CodecError::Publication)));
    let decoded = codec::decode(&receipt).unwrap().unwrap();
    let Record::ReserveNode(plan) = decoded.record else { panic!("actual node reservation"); };
    assert_eq!(plan.request, fixture.request);
    assert_eq!(decoded.timestamp.to_string(), receipt.result["timestamp"].as_str().unwrap());
    assert_eq!(decoded.membership_epoch, fixture.engine.config.membership_epoch());
}

fn held_edit_body(bytes: &[u8]) -> (tempfile::TempDir, crate::ReadFile) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("edit-source");
    fs::write(&path, bytes).unwrap();
    let file = fs::File::open(path).unwrap();
    (directory, file.into())
}
