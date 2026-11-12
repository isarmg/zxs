//! Actual projection/SQLite codec over public Engine state. Installation is
//! separately exercised after the shared held-resource installer is integrated.
use crate as engine_api;
#[path = "../../support/container_tags.rs"]
mod container_tags_fixture;
use super::*;
use crate::*;
use platform_io::sqlite::block_on_sqlite_connection;
use sqlite_access::sqlite::{SqliteConnectOptions, SqliteOwnedBuf};
use sqlite_access::{Connection, Executor};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::PermissionsExt,
};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    repo: Repository,
    file: Entry,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut engine = Engine::initialize(
            &temp.path().join("system"),
            Config::development([temp.path().join("disk")]).unwrap(),
        )
        .unwrap();
        let repo = engine.create_disk_root("P01", "admin", op()).unwrap();
        for bytes in [b"old content".as_slice(), b"new content".as_slice()] {
            let workspace = engine
                .create_workspace(&repo.main_branch_id, "admin")
                .unwrap();
            engine
                .stage_bytes(&workspace.workspace_id, "outside/path/source.txt", bytes)
                .unwrap();
            engine
                .commit_workspace(CommitRequest {
                    workspace_id: workspace.workspace_id,
                    operation_id: op(),
                    principal: "admin".into(),
                    expected_head: workspace.base_head,
                    fencing_token: workspace.fencing_token,
                    message: "file database projection".into(),
                    request_digest: None,
                })
                .unwrap();
        }
        let file = engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("outside/path/source.txt")
            .unwrap()
            .clone();
        Self {
            temp,
            engine,
            repo,
            file,
        }
    }
    fn bytes(&self) -> Vec<u8> {
        let empty = State::default();
        let projection =
            crate::state_projection::ProjectionTransition::new(&empty, self.engine.state(), None)
                .unwrap();
        let prepared = prepare(
            projection.next(),
            &self.file.file_id,
            &self.repo.history_tree_id,
        )
        .unwrap();
        verify_scope(
            &prepared.bytes,
            &self.file.file_id,
            &self.repo.history_tree_id,
        )
        .unwrap();
        prepared.bytes
    }
    fn delete(&mut self) -> FileDeletionNode {
        let repository = HistoryRepositoryId::parse(&self.repo.repository_id).unwrap();
        let file = HistoryFileId::parse(&self.file.file_id).unwrap();
        let path = name_policy::LookupPath::parse(self.file.path()).unwrap();
        self.engine
            .soft_delete_file(DeleteFileRequest {
                target: FileDeletionTarget {
                    repository_id: &repository,
                    file_id: &file,
                    path: &path,
                },
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        self.engine
            .file_deletion(&self.file.file_id)
            .unwrap()
            .clone()
    }
}
fn decoded(bytes: &[u8]) -> BTreeMap<(String, String), json_codec::Value> {
    block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().in_memory(true))
                .await
                .unwrap();
        connection
            .deserialize(None, SqliteOwnedBuf::try_from(bytes).unwrap(), true)
            .await
            .unwrap();
        let values = sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
        let rows = values
            .into_iter()
            .map(|row| {
                let body: String = row.try_get("body").unwrap();
                (
                    (row.try_get("kind").unwrap(), row.try_get("id").unwrap()),
                    json_codec::from_str(&body).unwrap(),
                )
            })
            .collect();
        connection.close().await.unwrap();
        rows
    })
}
#[test]
fn file_database_has_retained_nodes_branches_tags_and_no_exterior_path_records() {
    let mut fixture = Fixture::new();
    let branch = fixture
        .engine
        .create_file_branch(
            &fixture.file.file_id,
            &fixture.file.version_id,
            &BranchName::parse("review").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let path_receipt = fixture
        .engine
        .file_namespace(
            &fixture.repo.repository_id,
            NamespaceOperation::parse(
                NamespaceOperationKind::Rename,
                fixture.file.path(),
                Some("outside/path/renamed.txt"),
            )
            .unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    fixture.file = fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap()
        .get("outside/path/renamed.txt")
        .unwrap()
        .clone();
    let head = fixture.engine.file_branch_head(&branch).unwrap();
    container_tags_fixture::edit(&mut fixture.engine, &fixture.file.file_id, &["retained-label".into()], &[], "admin", op())
        .unwrap();
    fixture
        .engine
        .set_file_note(
            &fixture.file.file_id,
            None,
            &Note::parse("file note".into()).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let data = decoded(&fixture.bytes());
    assert_eq!(data.keys().filter(|(kind, _)| kind == "version").count(), 2);
    assert_eq!(data.keys().filter(|(kind, _)| kind == "branch").count(), 2);
    let facts = crate::persist::path_changes::read(&path_receipt).unwrap();
    assert_eq!(facts.len(), 1);
    assert!(!data.keys().any(|(kind, _)| kind == "anchor"));
    let path = &data[&("path_change".into(), facts[0].event_id.to_string())];
    assert_eq!(path["source_file_id"], fixture.file.file_id);
    assert_eq!(path["source_history_tree_id"], fixture.repo.history_tree_id);
    assert_eq!(path["file_id"], fixture.file.file_id);
    assert_eq!(path["operation_id"], path_receipt.operation_id.to_string());
    assert_eq!(path["sequence"], path_receipt.sequence.to_string());
    assert_eq!(
        path["occurred_at"],
        json_codec::to_value(facts[0].occurred_at).unwrap(),
    );
    assert_eq!(path["before_version_id"], fixture.file.version_id);
    assert_eq!(path["after_version_id"], fixture.file.version_id);
    assert_eq!(
        data[&("branch".into(), branch.clone())]["head_node_id"],
        fixture.engine.file_branch_head(&branch).unwrap()
    );
    assert_eq!(
        data[&("file".into(), fixture.file.file_id.clone())]["note"],
        "file note"
    );
    assert!(
        data.values()
            .any(|row| row["container_tags"]["tags"] == json_codec::json!(["retained-label"]))
    );
    for row in data.values() {
        let text = json_codec::to_string(row).unwrap();
        for forbidden in [
            "outside/path/",
            "elsewhere/",
            "\"path\"",
            "\"previous_path\"",
            "\"location\"",
            "\"route_slot_ids\"",
        ] {
            assert!(
                !text.contains(forbidden),
                "exterior material {forbidden} in {text}"
            );
        }
    }
}

#[test]
fn file_database_rejects_old_path_nodes_without_rewriting_state_or_archive_material() {
    use crate::container_storage::object_database::{
        ObjectDatabaseMaterial, ObjectDatabaseRecord, ObjectDatabaseScope,
    };

    let fixture = Fixture::new();
    let original_database = fixture.bytes();
    let mut state = fixture.engine.state().clone();
    let branch = state
        .file_contexts
        .values()
        .find(|context| {
            state.branches[&context.branch_id].file_id.as_deref()
                == Some(fixture.file.file_id.as_str())
        })
        .unwrap()
        .branch_id
        .clone();
    let node = FilePathNode {
        node_id: new_id("fnode").unwrap(),
        history_tree_id: fixture.repo.history_tree_id.clone(),
        file_id: fixture.file.file_id.clone(),
        version_id: fixture.file.version_id.clone(),
        parent_node_ids: vec![fixture.file.version_id.clone()],
        branch_id: branch,
        path: fixture.file.path().into(),
        previous_path: "old/location.txt".into(),
        operation_id: op(),
        author: "admin".into(),
        sequence: state.sequence,
        created_at: 1,
    };
    state.file_path_nodes.insert(node.node_id.clone(), node.clone());
    let old_bytes = canonical_json(&state).unwrap();
    assert_eq!(
        crate::history_package::validate_prepared_state(&state)
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid,
    );
    assert_eq!(
        crate::history_package::HistoryManifest::from_state(&state, &fixture.repo.history_tree_id)
            .err()
            .unwrap()
            .code(),
        ErrorCode::PublicationInvalid,
    );
    let projection = crate::state_projection::ProjectionSnapshot::new(&state);
    assert_eq!(
        projection::records(projection.view(), &fixture.file.file_id)
            .err()
            .unwrap()
            .code(),
        ErrorCode::PublicationInvalid,
    );
    assert_eq!(canonical_json(&state).unwrap(), old_bytes);
    let mut anchor = json_codec::to_value(&node).unwrap();
    let anchor_fields = anchor.as_object_mut().unwrap();
    anchor_fields.remove("path");
    anchor_fields.remove("previous_path");
    anchor_fields.insert("note".into(), json_codec::Value::Null);
    let material = ObjectDatabaseMaterial::new(
        ObjectDatabaseScope::File {
            file_id: fixture.file.file_id.clone(),
            history_tree_id: fixture.repo.history_tree_id.clone(),
        },
        state.sequence,
        "0".repeat(64),
        vec![ObjectDatabaseRecord::new(
            "anchor",
            node.node_id.clone(),
            json_codec::to_string(&anchor).unwrap(),
        )],
    );
    let material_bytes = canonical_json(&material).unwrap();
    assert_eq!(
        material::reconstruct_material(
            &material,
            &fixture.file.file_id,
            &fixture.repo.history_tree_id,
        )
        .unwrap_err()
        .code(),
        ErrorCode::ContainerLayoutInvalid,
    );
    assert_eq!(canonical_json(&material).unwrap(), material_bytes);
    assert_eq!(fixture.bytes(), original_database);
}
#[test]
fn file_database_retains_undone_deletions_and_exact_frontier_through_cold_open() {
    let mut fixture = Fixture::new();
    fixture
        .engine
        .create_file_branch(
            &fixture.file.file_id,
            &fixture.file.version_id,
            &BranchName::parse("review").unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let first = fixture.delete();
    let file = HistoryFileId::parse(&fixture.file.file_id).unwrap();
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file,
            deletion_node_id: &first.node_id,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let after_undo = decoded(&fixture.bytes());
    assert!(!after_undo.keys().any(|(kind, _)| kind == "deletion"));
    assert_eq!(
        after_undo
            .keys()
            .filter(|(kind, _)| kind == "lifecycle")
            .count(),
        2
    );
    let second = fixture.delete();
    let data = decoded(&fixture.bytes());
    let events = data
        .iter()
        .filter(|((kind, _), _)| kind == "lifecycle")
        .map(|(_, row)| row)
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .map(|row| row["node"]["node_id"].as_str().unwrap())
            .collect::<BTreeSet<_>>(),
        [&*first.node_id, &*second.node_id].into()
    );
    assert_eq!(
        data[&("deletion".into(), second.node_id.clone())]["branch_heads"],
        json_codec::to_value(&second.branch_heads).unwrap()
    );
    let Fixture {
        temp,
        engine,
        repo,
        file,
    } = fixture;
    drop(engine);
    let fixture = Fixture {
        engine: Engine::open(&temp.path().join("system")).unwrap(),
        temp,
        repo,
        file,
    };
    assert_eq!(decoded(&fixture.bytes()), data);
}
fn rewrite(bytes: &[u8], edit: impl FnOnce(&mut Vec<Record>)) -> Vec<u8> {
    block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().in_memory(true))
                .await
                .unwrap();
        connection
            .deserialize(None, SqliteOwnedBuf::try_from(bytes).unwrap(), false)
            .await
            .unwrap();
        let values = sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
        let mut records = values
            .into_iter()
            .map(|row| {
                let kind: String = row.try_get("kind").unwrap();
                let kind = match kind.as_str() {
                    "file" => "file",
                    "version" => "version",
                    "branch" => "branch",
                    "path_change" => "path_change",
                    "metadata" => "metadata",
                    "deletion" => "deletion",
                    "lifecycle" => "lifecycle",
                    "merge" => "merge",
                    "merge_link" => "merge_link",
                    _ => panic!("unexpected kind"),
                };
                Record {
                    kind,
                    id: row.try_get("id").unwrap(),
                    body: row.try_get("body").unwrap(),
                }
            })
            .collect::<Vec<_>>();
        edit(&mut records);
        connection.execute("DELETE FROM records").await.unwrap();
        for record in &records {
            sqlite_access::query("INSERT INTO records VALUES(?,?,?)")
                .bind(record.kind)
                .bind(&record.id)
                .bind(&record.body)
                .execute(&mut connection)
                .await
                .unwrap();
        }
        let hash = storage_contract::object_hash("file-local-records", &records).unwrap();
        sqlite_access::query("UPDATE identity SET records_hash=?")
            .bind(hash)
            .execute(&mut connection)
            .await
            .unwrap();
        let bytes = connection.serialize(None).await.unwrap().to_vec();
        connection.close().await.unwrap();
        bytes
    })
}
#[test]
fn file_database_rejects_self_hashed_extra_fields_foreign_rows_and_missing_old_deletion() {
    let mut fixture = Fixture::new();
    let first = fixture.delete();
    let file = HistoryFileId::parse(&fixture.file.file_id).unwrap();
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file,
            deletion_node_id: &first.node_id,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    fixture.delete();
    let good = fixture.bytes();
    let extra = rewrite(&good, |records| {
        let row = records.iter_mut().find(|row| row.kind == "branch").unwrap();
        let mut body: json_codec::Value = json_codec::from_str(&row.body).unwrap();
        body["exterior_path"] = json_codec::json!("must be preserved as unknown");
        row.body = json_codec::to_string(&body).unwrap();
    });
    let missing = rewrite(&good, |records| {
        records.retain(|row| row.id != first.operation_id.to_string())
    });
    let foreign = rewrite(&good, |records| {
        let row = records.iter_mut().find(|row| row.kind == "file").unwrap();
        let mut body: json_codec::Value = json_codec::from_str(&row.body).unwrap();
        body["file_id"] = json_codec::json!(format!("file_{}", "f".repeat(32)));
        row.body = json_codec::to_string(&body).unwrap();
    });
    for bad in [&extra, &missing, &foreign] {
        assert!(verify_scope(bad, &fixture.file.file_id, &fixture.repo.history_tree_id).is_err());
    }
    verify_scope(&good, &fixture.file.file_id, &fixture.repo.history_tree_id).unwrap();
}
#[test]
fn file_database_bounds_real_record_encoding_before_sqlite_allocation() {
    let fixture = Fixture::new();
    let mut state = fixture.engine.state().clone();
    state.file_notes.insert(
        fixture.file.file_id.clone(),
        "x".repeat(projection::MAX_RECORD_BYTES + 1),
    );
    let empty = State::default();
    let transition =
        crate::state_projection::ProjectionTransition::new(&empty, &state, None).unwrap();
    let error = match prepare(
        transition.next(),
        &fixture.file.file_id,
        &fixture.repo.history_tree_id,
    ) {
        Ok(_) => panic!("oversized record was prepared"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
}

fn rewrite_history(bytes: &[u8], history: &str) -> Vec<u8> {
    block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().in_memory(true))
                .await
                .unwrap();
        connection
            .deserialize(None, SqliteOwnedBuf::try_from(bytes).unwrap(), false)
            .await
            .unwrap();
        sqlite_access::query("UPDATE identity SET history_tree_id=?")
            .bind(history)
            .execute(&mut connection)
            .await
            .unwrap();
        let bytes = connection.serialize(None).await.unwrap().to_vec();
        connection.close().await.unwrap();
        bytes
    })
}
#[test]
fn file_database_binds_its_header_and_source_to_the_real_single_history() {
    let fixture = Fixture::new();
    let other = format!("history_{}", "f".repeat(32));
    let good = fixture.bytes();
    let mislabeled = rewrite_history(&good, &other);
    assert!(verify_scope(&mislabeled, &fixture.file.file_id, &other).is_err());
    let empty = State::default();
    let transition =
        crate::state_projection::ProjectionTransition::new(&empty, fixture.engine.state(), None)
            .unwrap();
    assert!(prepare(transition.next(), &fixture.file.file_id, &other).is_err());
    let mut mixed = fixture.engine.state().clone();
    mixed
        .versions
        .get_mut(&fixture.file.version_id)
        .unwrap()
        .history_tree_id = other;
    let transition =
        crate::state_projection::ProjectionTransition::new(&empty, &mixed, None).unwrap();
    assert!(
        prepare(
            transition.next(),
            &fixture.file.file_id,
            &fixture.repo.history_tree_id
        )
        .is_err()
    );
}
#[test]
fn file_database_checks_current_complete_frontier_and_retained_endpoint_times() {
    let mut fixture = Fixture::new();
    fixture
        .engine
        .create_file_branch(
            &fixture.file.file_id,
            &fixture.file.version_id,
            &BranchName::parse("review").unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let deletion = fixture.delete();
    let good = fixture.bytes();
    let removed = deletion.branch_heads.keys().next().unwrap();
    let reduced = rewrite(&good, |records| {
        for row in records
            .iter_mut()
            .filter(|row| matches!(row.kind, "deletion" | "lifecycle"))
        {
            let mut value: json_codec::Value = json_codec::from_str(&row.body).unwrap();
            let body = if row.kind == "deletion" {
                &mut value
            } else {
                &mut value["node"]
            };
            let heads = body["branch_heads"].as_object_mut().unwrap();
            heads.remove(removed);
            let remaining = heads
                .values()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect::<BTreeSet<_>>();
            body["parent_node_ids"] = json_codec::to_value(remaining).unwrap();
            row.body = json_codec::to_string(&value).unwrap();
        }
    });
    let missing_head = rewrite(&good, |records| {
        records.retain(|row| row.id != deletion.parent_node_ids[0])
    });
    let future = rewrite(&good, |records| {
        let row = records
            .iter_mut()
            .find(|row| row.kind == "lifecycle")
            .unwrap();
        let mut value: json_codec::Value = json_codec::from_str(&row.body).unwrap();
        value["sequence"] = json_codec::json!((fixture.engine.state().sequence + 1).to_string());
        row.body = json_codec::to_string(&value).unwrap();
    });
    for bad in [&reduced, &missing_head, &future] {
        assert!(verify_scope(bad, &fixture.file.file_id, &fixture.repo.history_tree_id).is_err());
    }
}
#[test]
fn file_database_hot_lifecycle_memberships_equal_cold_projection_and_self_hash_is_not_authority() {
    use crate::state_projection::{DerivedOwners, ProjectionTransition};
    fn compare(fixture: &Fixture, before: &State, owners: &DerivedOwners) -> DerivedOwners {
        let transition =
            ProjectionTransition::new(before, fixture.engine.state(), Some(owners)).unwrap();
        assert_eq!(
            transition.next().owners(),
            &DerivedOwners::from_state(fixture.engine.state())
        );
        assert_eq!(
            prepare(
                transition.next(),
                &fixture.file.file_id,
                &fixture.repo.history_tree_id
            )
            .unwrap()
            .bytes,
            fixture.bytes()
        );
        transition.into_owners()
    }
    let mut fixture = Fixture::new();
    fixture
        .engine
        .create_file_branch(
            &fixture.file.file_id,
            &fixture.file.version_id,
            &BranchName::parse("review").unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let mut previous = fixture.engine.state().clone();
    let mut owners = DerivedOwners::from_state(&previous);
    let first = fixture.delete();
    owners = compare(&fixture, &previous, &owners);
    previous = fixture.engine.state().clone();
    let undo = op();
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&fixture.file.file_id).unwrap(),
            deletion_node_id: &first.node_id,
            principal: "admin",
            operation_id: undo,
        })
        .unwrap();
    owners = compare(&fixture, &previous, &owners);
    previous = fixture.engine.state().clone();
    fixture.delete();
    compare(&fixture, &previous, &owners);
    let expected = fixture.bytes();
    let incomplete = rewrite(&expected, |records| {
        records
            .retain(|row| row.id != first.operation_id.to_string() && row.id != undo.to_string());
    });
    // A balanced omitted pair is structurally self-consistent. A self hash may
    // not certify completeness. The installer must bind exact expected bytes
    // rebuilt from the admitted State, not just accept this format verifier.
    verify_scope(
        &incomplete,
        &fixture.file.file_id,
        &fixture.repo.history_tree_id,
    )
    .unwrap();
    assert_ne!(incomplete, expected);
}

#[test]
fn isolated_file_material_rebuilds_sqlite_after_original_system_and_database_are_gone() {
    let fixture = Fixture::new();
    let scope = ObjectDatabaseScope::File {
        file_id: fixture.file.file_id.clone(),
        history_tree_id: fixture.repo.history_tree_id.clone(),
    };
    let bytes = fixture.bytes();
    let survivor = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    std::fs::write(survivor.path().join(NAME), bytes).unwrap();
    drop(fixture);
    let material = inspect_object_database_material(
        std::fs::File::open(survivor.path()).unwrap(),
        survivor.path().into(),
        &scope,
    )
    .unwrap();
    let archive = material.archive_bytes().unwrap();
    std::fs::remove_file(survivor.path().join(NAME)).unwrap();
    let rebuilt = reconstruct_object_database_material(&archive, &scope).unwrap();
    assert_eq!(rebuilt.material(), &material);
    assert_eq!(read_material(rebuilt.database_bytes()).unwrap(), material);
    assert!(!survivor.path().join(NAME).exists());
    assert!(
        material
            .records()
            .iter()
            .all(|record| !record.body_json().contains("outside/path"))
    );
}

#[test]
fn file_material_refuses_wrong_scope_and_retains_unknown_sidecar() {
    let fixture = Fixture::new();
    let survivor = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let bytes = fixture.bytes();
    std::fs::write(survivor.path().join(NAME), &bytes).unwrap();
    let scope = ObjectDatabaseScope::File {
        file_id: fixture.file.file_id.clone(),
        history_tree_id: fixture.repo.history_tree_id.clone(),
    };
    inspect_object_database_material(
        std::fs::File::open(survivor.path()).unwrap(),
        survivor.path().into(),
        &scope,
    )
    .unwrap();
    let wrong = ObjectDatabaseScope::File {
        file_id: format!("file_{}", "a".repeat(32)),
        history_tree_id: fixture.repo.history_tree_id.clone(),
    };
    assert!(
        inspect_object_database_material(
            std::fs::File::open(survivor.path()).unwrap(),
            survivor.path().into(),
            &wrong,
        )
        .is_err()
    );
    let sidecar = survivor.path().join(format!("{NAME}-wal"));
    std::fs::write(&sidecar, b"unknown retained WAL").unwrap();
    assert!(
        inspect_object_database_material(
            std::fs::File::open(survivor.path()).unwrap(),
            survivor.path().into(),
            &scope,
        )
        .is_err()
    );
    assert_eq!(std::fs::read(sidecar).unwrap(), b"unknown retained WAL");
    assert_eq!(std::fs::read(survivor.path().join(NAME)).unwrap(), bytes);
}

#[test]
fn recomputed_file_archive_digest_cannot_add_an_external_path_record() {
    let fixture = Fixture::new();
    let material = read_material(&fixture.bytes()).unwrap();
    let mut archive: json_codec::Value =
        json_codec::from_slice(&material.archive_bytes().unwrap()).unwrap();
    let record = archive["material"]["records"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|record| record["kind"] == "file")
        .unwrap();
    let mut body: json_codec::Value =
        json_codec::from_str(record["body"].as_str().unwrap()).unwrap();
    body["external_path"] = json_codec::json!("other/disk/private");
    record["body"] = json_codec::json!(
        String::from_utf8(storage_contract::canonical_json(&body).unwrap()).unwrap()
    );
    archive["material"]["records_hash"] = json_codec::json!(
        storage_contract::object_hash("file-local-records", &archive["material"]["records"])
            .unwrap()
    );
    assert!(
        reconstruct_object_database_material(
            &storage_contract::canonical_json(&archive).unwrap(),
            material.scope(),
        )
        .is_err()
    );
}

#[test]
fn file_rescue_readback_requires_all_records_from_the_exact_selected_capture() {
    let mut fixture = Fixture::new();
    let original = fixture.bytes();
    let material = read_material(&original).unwrap();
    let rebuilt =
        reconstruct_object_database_material(&material.archive_bytes().unwrap(), material.scope())
            .unwrap();
    rebuilt.verify_database_bytes(&original).unwrap();
    rebuilt
        .verify_database_bytes(rebuilt.database_bytes())
        .unwrap();
    fixture.delete();
    let later = fixture.bytes();
    assert!(read_material(&later).is_ok());
    assert!(rebuilt.verify_database_bytes(&later).is_err());
    assert!(
        rebuilt
            .verify_database_bytes(&original[..original.len() / 2])
            .is_err()
    );
}

#[test]
fn typed_copy_capture_preserves_record_hash_and_exact_lifecycle_without_json_roundtrip() {
    fn check(fixture: &Fixture, count: usize) {
        let projection = crate::state_projection::ProjectionSnapshot::new(fixture.engine.state());
        let file = &fixture.file.file_id;
        let history = &fixture.repo.history_tree_id;
        let projected = validated_records(projection.view(), file, history).unwrap();
        // The former reader is an independent test oracle, not production work.
        let mut decoded = projected
            .records
            .iter()
            .filter(|record| record.kind == "lifecycle")
            .map(|record| {
                storage_contract::decode_strict::<crate::file_deletion::retained::LifecycleRow>(
                    record.body.as_bytes(),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        decoded.sort_by_key(|event| event.sequence);
        assert_eq!(decoded.len(), count);
        assert_eq!(projected.events, decoded);
        let expected_hash =
            storage_contract::object_hash("file-local-records", &projected.records).unwrap();
        let shared = capture_projected_copy(projection.view(), file, history).unwrap();
        let standalone = capture_copy(fixture.engine.state(), file, history).unwrap();
        assert_eq!(shared.events, decoded);
        assert_eq!(standalone.events, decoded);
        assert_eq!(shared.records_hash, expected_hash);
        assert_eq!(standalone.records_hash, expected_hash);
    }

    let mut fixture = Fixture::new();
    check(&fixture, 0);
    let deleted = fixture.delete();
    check(&fixture, 1);
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&fixture.file.file_id).unwrap(),
            deletion_node_id: &deleted.node_id,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    fixture.delete();
    check(&fixture, 3);
}

#[test]
fn file_database_storage_integer_admission_preserves_cause_order_and_bytes() {
    use std::error::Error as _;
    use storage_contract::encoding::canonical_json;

    let fixture = Fixture::new();
    let state_before = canonical_json(fixture.engine.state()).unwrap();
    let good = fixture.bytes();
    let good_before = good.clone();
    for corrupt_calendar in [false, true] {
        let bad = rewrite(&good, |records| {
            let record = records
                .iter_mut()
                .find(|row| row.kind == "version")
                .unwrap();
            let mut row: rows::VersionRow =
                storage_contract::decode_strict(record.body.as_bytes()).unwrap();
            let (_, suffix) = row.version.storage_name.split_once('_').unwrap();
            row.version.storage_name = format!("v18446744073709551616_{suffix}");
            if corrupt_calendar {
                row.version.created_at = "2023-02-29T00:00:00Z".into();
            }
            record.body = String::from_utf8(
                crate::disk::encode_metadata(&row, projection::MAX_RECORD_BYTES).unwrap(),
            )
            .unwrap();
        });
        let before = bad.clone();
        let error = verify_scope(&bad, &fixture.file.file_id, &fixture.repo.history_tree_id)
            .unwrap_err();
        let mut current = &error;
        for _ in 0..2 {
            assert_eq!(current.code(), ErrorCode::ContainerLayoutInvalid);
            assert!(!current.is_retryable());
            assert!(current.published_result().is_none());
            let Error::Context { source, .. } = current else {
                panic!("storage admission must retain both context layers: {current:?}");
            };
            current = source;
        }
        assert!(matches!(current, Error::Integer(_)));
        assert_eq!(current.code(), ErrorCode::ContainerLayoutInvalid);
        assert!(!current.is_retryable());
        assert!(current.published_result().is_none());
        let cause = current
            .source()
            .unwrap()
            .downcast_ref::<std::num::ParseIntError>()
            .unwrap();
        assert_eq!(cause.kind(), &std::num::IntErrorKind::PosOverflow);
        assert!(cause.source().is_none());
        assert_eq!(bad, before);
    }
    // Admission accepts the largest sequence; this is not a next-sequence request.
    let maximum = rewrite(&good, |records| {
        let record = records
            .iter_mut()
            .find(|row| row.kind == "version")
            .unwrap();
        let mut row: rows::VersionRow =
            storage_contract::decode_strict(record.body.as_bytes()).unwrap();
        let (_, suffix) = row.version.storage_name.split_once('_').unwrap();
        row.version.storage_name = format!("v{}_{suffix}", u64::MAX);
        record.body = String::from_utf8(
            crate::disk::encode_metadata(&row, projection::MAX_RECORD_BYTES).unwrap(),
        )
        .unwrap();
    });
    let maximum_before = maximum.clone();
    verify_scope(&maximum, &fixture.file.file_id, &fixture.repo.history_tree_id).unwrap();
    assert_eq!(maximum, maximum_before);
    assert_eq!(good, good_before);
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), state_before);
}

#[test]
fn container_tag_record_is_file_owned_and_rescue_preserves_its_cas_generation() {
    let fixture = Fixture::new();
    let mut state = fixture.engine.state().clone();
    let file = fixture.file.file_id.parse::<storage_contract::FileId>().unwrap();
    let tags = storage_contract::file_container_tags::TagSet::new([
        storage_contract::file_container_tags::Tag::parse("review".into()).unwrap(),
    ]).unwrap();
    let record = storage_contract::file_container_tags::ContainerTags::new(
        tags, storage_contract::Generation::new(7),
    );
    state.container_tags.insert(file, record.clone());
    let empty = State::default();
    let projection = crate::state_projection::ProjectionTransition::new(&empty, &state, None)
        .unwrap();
    let prepared = prepare(projection.next(), &fixture.file.file_id, &fixture.repo.history_tree_id)
        .unwrap();
    let records = decoded(&prepared.bytes);
    let row = &records[&("file".into(), fixture.file.file_id.clone())];
    assert_eq!(row["container_tags"], json_codec::to_value(&record).unwrap());
    for ((kind, _), row) in &records {
        if kind != "file" {
            assert!(row.get("tags").is_none());
        }
    }
    let material = read_material(&prepared.bytes).unwrap();
    let rebuilt = reconstruct_material(
        &material, &fixture.file.file_id, &fixture.repo.history_tree_id,
    ).unwrap();
    assert_eq!(read_material(&rebuilt).unwrap(), material);
    state.container_tags.remove(&file);
    let projection = crate::state_projection::ProjectionTransition::new(&empty, &state, None)
        .unwrap();
    assert!(prepare(projection.next(), &fixture.file.file_id, &fixture.repo.history_tree_id).is_err());
}

#[test]
fn container_tags_snapshot_admission_requires_exact_file_coverage() {
    let fixture = Fixture::new();
    let file = fixture.file.file_id.parse::<storage_contract::FileId>().unwrap();
    let mut state = fixture.engine.state().clone();
    let tags = storage_contract::file_container_tags::ContainerTags::new(
        storage_contract::file_container_tags::TagSet::empty(),
        storage_contract::Generation::new(9),
    );
    state.container_tags.insert(file, tags.clone());
    let members = fixture.engine.members();
    let hash = crate::journal::store_snapshot(&members, &state).unwrap();
    let admitted = crate::publication_evidence::verified_snapshot(&members, &hash).unwrap();
    assert_eq!(admitted.state().container_tags.get(&file), Some(&tags));
    state.container_tags.remove(&file);
    let hash = crate::journal::store_snapshot(&members, &state).unwrap();
    assert!(crate::publication_evidence::verified_snapshot(&members, &hash).is_err());
    state.container_tags.insert(file, tags.clone());
    state.container_tags.insert(
        "file_ffffffffffffffffffffffffffffffff".parse().unwrap(), tags,
    );
    let hash = crate::journal::store_snapshot(&members, &state).unwrap();
    assert!(crate::publication_evidence::verified_snapshot(&members, &hash).is_err());
}
