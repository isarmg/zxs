//! Current request and permanent receipt bytes. Publishing stays in business.
use crate::business::file_metadata::{Plan, Request, invalid};
use crate::{Engine, Error, FileMetadataError, OperationResult, Result};
use data_serialization::Deserialize;
use json_codec::{Value, json};
use storage_contract::{BranchId, BranchName, FileId, Generation, HistoryTreeId, Note, VersionId};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct NoteEnvelope {
    file_id: FileId,
    version_id: Option<VersionId>,
    note: Note,
    #[serialization(with = "storage_contract::u64_string")]
    timestamp: u64,
    #[serialization(with = "storage_contract::u64_string")]
    membership_epoch: u64,
    version_protection: Vec<()>,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct RenameEnvelope {
    file_id: FileId,
    branch_id: BranchId,
    history_tree_id: HistoryTreeId,
    previous_name: String,
    name: BranchName,
    no_change: bool,
    branch_generation: Generation,
    #[serialization(with = "storage_contract::u64_string")]
    timestamp: u64,
    #[serialization(with = "storage_contract::u64_string")]
    membership_epoch: u64,
    version_protection: Vec<()>,
}

pub(crate) enum Decoded {
    Note(NoteEnvelope),
    Rename(RenameEnvelope),
}

impl Decoded {
    pub(crate) fn request(&self) -> Request<'_> {
        match self {
            Self::Note(value) => Request::Note {
                file: value.file_id, version: value.version_id, note: &value.note,
            },
            Self::Rename(value) => Request::Rename {
                file: value.file_id, branch: value.branch_id, name: &value.name,
            },
        }
    }

    pub(crate) fn encoded(&self, plan: &Plan<'_>) -> Result<Value> {
        let (timestamp, epoch) = match (self, plan) {
            (Self::Note(value), Plan::Note { .. }) => {
                if !value.version_protection.is_empty() { return Err(invalid()); }
                (value.timestamp, value.membership_epoch)
            }
            (Self::Rename(value), Plan::Rename {
                source, repository, name, ..
            }) => {
                if !value.version_protection.is_empty()
                    || value.history_tree_id.to_string() != repository.history_tree_id
                    || value.branch_generation.get() != source.generation
                    || value.previous_name != source.name
                    || value.no_change != (source.name == name.as_str())
                {
                    return Err(invalid());
                }
                (value.timestamp, value.membership_epoch)
            }
            (Self::Note(_), Plan::Rename { .. }) | (Self::Rename(_), Plan::Note { .. }) => {
                return Err(invalid());
            }
        };
        let mut result = outcome(plan);
        result["timestamp"] = json!(timestamp.to_string());
        result["membership_epoch"] = json!(epoch.to_string());
        result["version_protection"] = json!([]);
        if let Plan::Rename { source, .. } = plan {
            result["branch_generation"] = json!(source.generation.to_string());
        }
        Ok(result)
    }
}

pub(crate) fn is_kind(kind: &str) -> bool {
    matches!(kind, "file.metadata" | "version.metadata" | "file.branch.rename")
}

pub(crate) fn recognizes(receipt: &OperationResult) -> bool {
    is_kind(&receipt.kind)
        || receipt.result.get("file_id").is_some()
            && (receipt.result.get("version_id").is_some()
                && receipt.result.get("note").is_some()
                || receipt.result.get("branch_id").is_some()
                    && receipt.result.get("previous_name").is_some())
}

pub(crate) fn decode(receipt: &OperationResult) -> Result<Decoded> {
    let codec = |source| Error::FileMetadata(FileMetadataError::Encoding(source));
    let decoded = match receipt.kind.as_str() {
        "file.metadata" | "version.metadata" => Decoded::Note(
            json_codec::from_value(receipt.result.clone()).map_err(codec)?,
        ),
        "file.branch.rename" => Decoded::Rename(
            json_codec::from_value(receipt.result.clone()).map_err(codec)?,
        ),
        _ => return Err(invalid()),
    };
    if decoded.request().kind() != receipt.kind { return Err(invalid()); }
    Ok(decoded)
}

// Raw ingress text is encoded before identifier parsing, preserving the existing
// actor/idempotency/write/clock precedence and the original request digest.
pub(crate) fn note_input(file: &str, version: Option<&str>, note: &Note) -> Value {
    json!({"file_id":file,"version_id":version,"note":note.as_str()})
}

pub(crate) fn rename_input(file: &str, branch: &str, name: &BranchName) -> Value {
    json!({"file_id":file,"branch_id":branch,"name":name.as_str()})
}

pub(crate) fn digest(request: Request<'_>, principal: &str) -> Result<String> {
    let body = match request {
        Request::Note { file, version, note } => {
            json!({"file_id":file,"version_id":version,"note":note.as_str()})
        }
        Request::Rename { file, branch, name } => {
            json!({"file_id":file,"branch_id":branch,"name":name.as_str()})
        }
    };
    Engine::native_request_digest(request.kind(), principal, &body)
}

pub(crate) fn outcome(plan: &Plan<'_>) -> Value {
    match plan {
        Plan::Note { file, version, note } => {
            json!({"file_id":file,"version_id":version,"note":note.as_str()})
        }
        Plan::Rename { file, branch, source, repository, name } => json!({
            "file_id":file,
            "branch_id":branch,
            "history_tree_id":repository.history_tree_id,
            "previous_name":source.name,
            "name":name.as_str(),
            "no_change":source.name == name.as_str(),
        }),
    }
}
