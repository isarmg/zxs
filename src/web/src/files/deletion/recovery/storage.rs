//! Bounded immutable local recovery hints.
use super::*;
use crate::files::recovery_storage::Store;
use data_serialization::{
    Deserialize, Deserializer, Serialize,
    de::{MapAccess, Visitor, value::MapAccessDeserializer},
};
use storage_contract::Principal;
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

const PREFIX: &str = "zxs:file-deletion:";
pub(super) const MAX_RECORDS: usize = 32;
pub(super) const MAX_QUEUE_BYTES: usize = 4 * 1024 * 1024;
const MAX_RECORD_BYTES: usize =
    storage_contract::client_protocol::MAX_FILE_DELETION_RESPONSE_BYTES + 16 * 1024;

#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "file-deletion-original-request")]
    OriginalRequest,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Stored {
    format: Format,
    principal: Principal,
    operation: OperationId,
    request: StoredIntent,
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StoredIntent {
    Delete {
        file_id: HistoryFileId,
        repository_id: HistoryRepositoryId,
        path: name_policy::PortablePath,
    },
    DeleteBatch {
        #[serialization(deserialize_with = "batch::stored")]
        request: batch::Stored,
    },
    DeleteDirectory {
        #[serialization(deserialize_with = "directory::stored")]
        request: directory::Stored,
    },
    Undo {
        #[serialization(deserialize_with = "stored_node")]
        deletion_node: storage_contract::FileDeletionNode,
    },
    UndoSummary {
        #[serialization(deserialize_with = "stored_summary")]
        deletion_summary: storage_contract::file_deletion_review::Summary,
    },
    UndoBatch {
        #[serialization(deserialize_with = "stored_batch_preview")]
        preview: storage_contract::file_deletion_batch::FileDeletionBatchUndoPreview,
    },
    UndoDirectory {
        #[serialization(deserialize_with = "directory::wire::stored_node")]
        deletion_node: storage_contract::directory_git::DirectoryDeletionNode,
    },
}
fn stored_node<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<storage_contract::FileDeletionNode, D::Error> {
    struct NodeObject;
    impl<'de> Visitor<'de> for NodeObject {
        type Value = storage_contract::FileDeletionNode;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a named deletion-node object")
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            storage_contract::FileDeletionNode::deserialize(MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_map(NodeObject)
}
fn stored_summary<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<storage_contract::file_deletion_review::Summary, D::Error> {
    struct SummaryObject;
    impl<'de> Visitor<'de> for SummaryObject {
        type Value = storage_contract::file_deletion_review::Summary;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a named deletion-summary object")
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            Self::Value::deserialize(MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_map(SummaryObject)
}

fn stored_batch_preview<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<storage_contract::file_deletion_batch::FileDeletionBatchUndoPreview, D::Error> {
    #[derive(Deserialize)]
    struct Member(
        #[serialization(deserialize_with = "stored_node")] storage_contract::FileDeletionNode,
    );
    #[derive(Deserialize)]
    #[serialization(deny_unknown_fields)]
    struct Preview {
        deletion_operation_id: OperationId,
        deletion_nodes: Vec<Member>,
    }
    struct PreviewObject;
    impl<'de> Visitor<'de> for PreviewObject {
        type Value = storage_contract::file_deletion_batch::FileDeletionBatchUndoPreview;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a named original-batch preview object")
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            let preview = Preview::deserialize(MapAccessDeserializer::new(map))?;
            Ok(
                storage_contract::file_deletion_batch::FileDeletionBatchUndoPreview {
                    deletion_operation_id: preview.deletion_operation_id,
                    deletion_nodes: preview
                        .deletion_nodes
                        .into_iter()
                        .map(|member| member.0)
                        .collect(),
                },
            )
        }
    }
    deserializer.deserialize_map(PreviewObject)
}

fn decode_error(source: storage_contract::Error) -> api::Error {
    let mut error = failure("The local deletion recovery record is invalid; it was preserved.");
    error.source = Some(Arc::new(api::ErrorSource::FileDeletionRecovery(source)));
    error
}
fn prefix(principal: &Principal) -> String {
    format!("{PREFIX}{}:", api::segment(principal.as_str()))
}
impl Record {
    pub(super) fn key(&self) -> String {
        format!("{}{}", prefix(&self.principal), self.operation)
    }
    pub(super) fn decode(
        key: &str,
        principal: &Principal,
        bytes: &str,
    ) -> Result<Self, api::Error> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(limit(
                "The deletion recovery record exceeds its byte budget.",
            ));
        }
        if !bytes.trim_start().starts_with('{') {
            return Err(failure("Deletion recovery must be a named request object."));
        }
        let stored: Stored = storage_contract::StrictJson::parse(bytes.as_bytes())
            .and_then(|json| json.decode::<Stored>())
            .map_err(decode_error)?;
        if stored.principal.as_str().len() > 1024 || &stored.principal != principal {
            return Err(failure("The deletion recovery account does not match."));
        }
        let intent = match stored.request {
            StoredIntent::Delete {
                file_id,
                repository_id,
                path,
            } => request::Intent::Delete(request::Target {
                file: file_id,
                repository: repository_id,
                path,
            }),
            StoredIntent::DeleteBatch { request } => {
                request::Intent::DeleteBatch(Arc::new(request.admit()?))
            }
            StoredIntent::DeleteDirectory { request } => {
                request::Intent::DeleteDirectory(Arc::new(request.admit()?))
            }
            StoredIntent::Undo { deletion_node } => {
                wire::validate_node(&deletion_node).map_err(|mut error| {
                    error.status = 0;
                    error.code = ErrorCode::InvalidRequest;
                    error.message = "The locally retained deletion node is invalid; the original record is preserved.".into();
                    error
                })?;
                request::Intent::Undo(Arc::new(deletion_node))
            }
            StoredIntent::UndoSummary { deletion_summary } => {
                request::Intent::UndoSummary(Arc::new(deletion_summary))
            }
            StoredIntent::UndoBatch { preview } => {
                request::Intent::UndoBatch(Arc::new(batch_restore::Request::admit(
                    preview.deletion_operation_id,
                    preview.deletion_nodes,
                )?))
            }
            StoredIntent::UndoDirectory { deletion_node } => {
                request::Intent::UndoDirectory(Arc::new(deletion_node))
            }
        };
        let record = Self {
            request_digest: intent.digest(&stored.principal)?,
            principal: stored.principal,
            operation: stored.operation,
            intent,
            bytes: Arc::from(bytes),
        };
        if record.key() != key {
            return Err(failure(
                "The deletion recovery operation does not match its immutable key.",
            ));
        }
        Ok(record)
    }
    pub(super) fn from_attempt(attempt: &request::Attempt) -> Result<Self, api::Error> {
        let request = match &attempt.intent {
            request::Intent::Delete(target) => StoredIntent::Delete {
                file_id: target.file.clone(),
                repository_id: target.repository.clone(),
                path: target.path.clone(),
            },
            request::Intent::DeleteBatch(request) => StoredIntent::DeleteBatch {
                request: request.stored(),
            },
            request::Intent::DeleteDirectory(request) => StoredIntent::DeleteDirectory {
                request: request.stored(),
            },
            request::Intent::Undo(node) => StoredIntent::Undo {
                deletion_node: node.as_ref().clone(),
            },
            request::Intent::UndoSummary(summary) => StoredIntent::UndoSummary {
                deletion_summary: summary.as_ref().clone(),
            },
            request::Intent::UndoBatch(request) => StoredIntent::UndoBatch {
                preview: request.preview().clone(),
            },
            request::Intent::UndoDirectory(node) => StoredIntent::UndoDirectory {
                deletion_node: node.as_ref().clone(),
            },
        };
        let operation: OperationId = attempt
            .operation
            .parse()
            .expect("invariant: browser generated a canonical operation ID");
        let stored = Stored {
            format: Format::OriginalRequest,
            principal: attempt.principal.clone(),
            operation,
            request,
        };
        let bytes = storage_contract::canonical_json(&stored).map_err(decode_error)?;
        let bytes = String::from_utf8(bytes).expect("invariant: canonical JSON is UTF-8");
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(limit(
                "The deletion recovery record exceeds its byte budget; no request was sent.",
            ));
        }
        Ok(Self {
            principal: attempt.principal.clone(),
            operation,
            intent: attempt.intent.clone(),
            request_digest: attempt.request_digest.clone(),
            bytes: Arc::from(bytes),
        })
    }
}
pub(super) fn load(principal: &Principal) -> Result<Vec<Retained>, api::Error> {
    let mut records = Vec::new();
    let mut size = 0;
    Store::open()?.scan(&prefix(principal), |key, bytes| {
        // The platform returns one value at a time. Bound it before JSON parsing
        // and bound the entire queue before retaining another decoded frontier.
        if records.len() == MAX_RECORDS || bytes.len() > MAX_QUEUE_BYTES - size {
            return Err(limit(
                "Deletion recovery exceeds 32 records or 4 MiB; nothing was removed.",
            ));
        }
        size += bytes.len();
        let record = Record::decode(key, principal, bytes)?;
        records.push(Retained::unknown(record));
        Ok(())
    })?;
    Ok(records)
}
pub(super) fn save(attempt: &request::Attempt) -> Result<Record, api::Error> {
    if !load(&attempt.principal)?.is_empty() {
        return Err(failure(
            "Resolve the retained deletion or restoration operations before starting another.",
        ));
    }
    let record = Record::from_attempt(attempt)?;
    Store::open()?.save(&record.key(), &record.bytes, |bytes| {
        Ok(bytes == record.bytes.as_ref())
    })?;
    Ok(record)
}
pub(super) fn verify(record: &Record) -> Result<(), api::Error> {
    Store::open()?.verify(&record.key(), |bytes| Ok(bytes == record.bytes.as_ref()))
}
pub(super) fn remove(record: &Record) -> Result<(), api::Error> {
    Store::open()?.remove(&record.key(), |bytes| Ok(bytes == record.bytes.as_ref()))
}

#[cfg(test)]
#[path = "../../../../../../test/web/unit/files/deletion/batch_restore_storage_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../../test/web/unit/files/deletion/summary_storage_tests.rs"]
mod summary_tests;
