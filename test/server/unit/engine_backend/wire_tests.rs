use super::*;
use crate::operation_wire::operation_wire;
use json_codec::json;
use storage_contract::{
    CommitState, OperationOutcome, ProtectionAtPublish,
    client_protocol::{OperationStatus, operation_status},
};

// Retain the old whole-tree projection only as a differential test oracle.
impl<'a> PendingSources<'a> {
    pub(in crate::engine_backend) fn pending_versions(
        &self,
    ) -> Result<std::collections::BTreeMap<String, String>, storage_engine::Error> {
        let historical = self
            .current
            .filter(|current| current.head_commit_id != self.commit.commit_id)
            .map(|_| {
                let mut contexts = std::collections::BTreeMap::new();
                for tree in self
                    .engine
                    .state()
                    .directory_trees
                    .values()
                    .filter(|tree| tree.commit_id == self.commit.commit_id)
                {
                    for (key, context) in &tree.file_contexts {
                        // BTreeMap iteration preserves the original first tree.
                        contexts.entry(key.as_str()).or_insert(context);
                    }
                }
                contexts
            });
        let Some(tree) = self.engine.commit_namespace(&self.commit.commit_id) else {
            return Ok(std::collections::BTreeMap::new());
        };
        let mut versions = std::collections::BTreeMap::new();
        tree.walk(|entry| {
            if entry.kind != storage_contract::NamespaceEntryKind::File {
                return Ok(true);
            }
            let context = self.main_repository.and_then(|repository| {
                self.engine
                    .file_context_for_current_entry(repository, entry)
            });
            let resolution = PendingResolution {
                state: self.engine.state(),
                commit: self.commit,
                current: self.current,
                main_repository: self.main_repository,
            };
            if let Some(source) = resolution.resolve(
                entry,
                entry,
                PendingContexts {
                    current: context.as_deref(),
                    historical: historical
                        .as_ref()
                        .and_then(|contexts| contexts.get(entry.lookup_key()).copied()),
                },
                || self.engine.pending_file_source(self.branch, &entry.file_id),
            ) {
                versions.insert(entry.file_id.clone(), source.to_owned());
            }
            Ok(true)
        })?;
        Ok(versions)
    }
}

fn domain(value: &Value) -> OperationOutcome {
    let content = value["operation_kind"] == "content_save";
    let result = &value["result"];
    OperationOutcome {
        operation_id: value["operation_id"].as_str().unwrap().parse().unwrap(),
        kind: if content {
            "workspace.commit"
        } else {
            value["operation_kind"].as_str().unwrap()
        }
        .into(),
        commit_id: result["commit_id"].as_str().map(str::to_owned),
        history_tree_id: None,
        branch_id: result["branch_id"].as_str().map(str::to_owned),
        commit_state: json_codec::from_value(value["commit_state"].clone()).unwrap(),
        protection_at_publish: json_codec::from_value(
            if content { result } else { value }["protection_at_publish"].clone(),
        )
        .unwrap(),
        request_digest: value["request_digest"].as_str().unwrap().into(),
        sequence: storage_contract::parse_u64_decimal(
            if content { result } else { value }["publication_sequence"]
                .as_str()
                .unwrap(),
        )
        .unwrap(),
        result: if content {
            json!({"branch_generation":result["branch_generation"],"membership_epoch":result["membership_epoch"],"version_protection":result["versions"]})
        } else {
            result.clone()
        },
    }
}

fn receipts() -> Vec<Value> {
    [
        include_str!("../../../web/unit/files/fixtures/namespace-batch-operation.json"),
        include_str!("../../../web/unit/files/fixtures/damaged-published-save-operation.json"),
        include_str!("../../../web/unit/files/fixtures/degraded-save-operation.json"),
    ]
    .into_iter()
    .map(|body| json_codec::from_str(body).unwrap())
    .collect()
}

#[test]
fn outcome_wire_matches_derived_fixture_fields_and_the_shared_parser() {
    for mut expected in receipts() {
        let actual = operation_wire(domain(&expected)).unwrap();
        // This comparison derives the outcome projection in memory; captured
        // fixture evidence is not rewritten to claim a new HTTP execution.
        expected.as_object_mut().unwrap().remove("current_health");
        assert_eq!(operation_status(&actual), operation_status(&expected));
        assert!(matches!(
            operation_status(&actual),
            Some(OperationStatus::Published | OperationStatus::Degraded)
        ));
        assert!(actual.get("current_health").is_none());
        assert_eq!(actual, expected);
    }
}

#[test]
fn unknown_and_aborted_projection_have_one_current_shape_per_operation_kind() {
    for expected in receipts().into_iter().take(2) {
        let content = expected["operation_kind"] == "content_save";
        for commit in [
            CommitState::InDoubt,
            CommitState::RecoveryRequired,
            CommitState::Aborted,
        ] {
            let mut operation = domain(&expected);
            operation.commit_state = commit;
            operation.protection_at_publish = ProtectionAtPublish::Unconfirmed;
            let actual = operation_wire(operation).unwrap();
            assert_eq!(
                operation_status(&actual),
                Some(if commit == CommitState::Aborted {
                    OperationStatus::Rejected
                } else {
                    OperationStatus::Unknown
                })
            );
            if content && commit == CommitState::Aborted {
                assert_eq!(actual["state"], "failed");
                assert!(actual["result"].is_null());
                let error = &actual["error"];
                assert_eq!(error["code"], "OPERATION_ABORTED");
                assert_eq!(error["retryable"], false);
                storage_contract::validate_id(error["request_id"].as_str().unwrap(), "req")
                    .unwrap();
                assert_eq!(actual["error"].as_object().unwrap().len(), 4);
                let mut old = actual.clone();
                old["state"] = "processing".into();
                old["error"] = Value::Null;
                assert_eq!(operation_status(&old), None);
            } else {
                assert_eq!(actual["state"], "in_doubt");
                assert!(actual["error"].is_null());
                assert_eq!(actual["result"].is_null(), content);
            }
        }
    }
}

#[test]
fn stored_version_content_state_preserves_sparse_available_and_explicit_dispositions() {
    use storage_contract::ContentState;
    assert_eq!(stored_version_content_state(None), ContentState::Available);
    for state in [
        ContentState::Available,
        ContentState::Purged,
        ContentState::Migrated,
    ] {
        assert_eq!(stored_version_content_state(Some(&state)), state);
    }
}
