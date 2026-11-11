//! Typed retained commands derive both the HTTP request and server digest.
//! No retry reads the current listing or mints another operation identity.
use crate::api;
use crate::files::mkdir::failure;
use crate::files::mkdir::storage::{Format, Namespace, Record, Stored, decode_error, required_path};
use crate::ui::State;
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use name_policy::PortablePath;
use storage_contract::directory_git::{DirectoryBranchCreateSource, HistoricalDirectoryBranchSource};
use storage_contract::{
    BranchName, DirectoryBranchId, DirectoryTreeId, OperationId, Principal, RepositoryId,
};

#[derive(Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum DirectoryRequest {
    Rename {
        repository_id: RepositoryId,
        directory_branch_id: DirectoryBranchId,
        name: BranchName,
    },
    Create {
        source: DirectoryBranchCreateSource,
        repository_path: String,
        name: String,
    },
    CreateHistorical {
        source: HistoricalDirectoryBranchSource,
        #[serialization(deserialize_with = "required_path")]
        repository_path: Option<PortablePath>,
        name: BranchName,
    },
    Checkout {
        repository_id: String,
        directory_branch_id: String,
        expected_tree_id: String,
        expected_head: String,
    },
}
impl DirectoryRequest {
    pub(super) fn is_checkout(&self) -> bool {
        matches!(self, Self::Checkout { .. })
    }
    pub(super) fn checkout_applies(&self, state: &State) -> bool {
        match self {
            Self::Checkout {
                repository_id,
                directory_branch_id,
                expected_tree_id,
                ..
            } => {
                state.files.repository_id == *repository_id
                    && state
                        .files
                        .directory_preview
                        .as_ref()
                        .and_then(|preview| preview.branch())
                        .is_some_and(|branch| {
                            branch.id() == directory_branch_id
                                && branch.head_tree_id() == expected_tree_id
                        })
            }
            Self::Create { .. } | Self::CreateHistorical { .. } | Self::Rename { .. } => false,
        }
    }
    pub(super) fn label(&self, path: &str) -> String {
        match self {
            Self::Rename {
                directory_branch_id,
                name,
                ..
            } => format!("{path} · rename {directory_branch_id} → {}", name.as_str()),
            Self::Create { name, .. } => format!("{path} · create branch {name}"),
            Self::CreateHistorical { name, source, .. } => format!(
                "{path} · create branch {} from {}",
                name.as_str(),
                source.tree_id(),
            ),
            Self::Checkout {
                directory_branch_id,
                ..
            } => {
                format!("{path} · set default {directory_branch_id}")
            }
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Rename { .. } => "directory.branch.rename",
            Self::Create { .. } => "directory.git.branch-create",
            Self::CreateHistorical { .. } => "directory.git.branch-create-history",
            Self::Checkout { .. } => "directory.git.checkout",
        }
    }
    fn input(&self) -> Value {
        match self {
            Self::Rename {
                repository_id,
                directory_branch_id,
                name,
            } => json!({
                "repository_id":repository_id, "directory_branch_id":directory_branch_id,
                "name":name.as_str(),
            }),
            Self::Create {
                source,
                repository_path,
                name,
            } => json!({
                "repository_id":source.repository_id,
                "path":repository_path, "name":name, "source":source,
            }),
            Self::CreateHistorical {
                source,
                repository_path,
                name,
            } => json!({
                "repository_id":source.current().repository_id,
                "path":repository_path.as_ref().map_or("", PortablePath::as_str),
                "name":name.as_str(), "source":source,
            }),
            Self::Checkout {
                repository_id,
                directory_branch_id,
                expected_tree_id,
                expected_head,
            } => json!({
                "repository_id":repository_id, "directory_branch_id":directory_branch_id,
                "expected_tree_id":expected_tree_id, "expected_head":expected_head,
            }),
        }
    }
    pub(super) fn validate(&self, display: &str) -> Result<(), api::Error> {
        match self {
            // Typed fields already admit the original label request. Display
            // paths identify the editor location, not a new server-side CAS.
            Self::Rename { .. } => {}
            Self::Create {
                source,
                repository_path,
                name,
            } => {
                source.validate().map_err(decode_error)?;
                storage_contract::BranchName::parse(name).map_err(decode_error)?;
                if !repository_path.is_empty() {
                    name_policy::PortablePath::parse(repository_path)
                        .map_err(|source| decode_error(source.into()))?;
                }
                if (repository_path.is_empty()) != (source.directory_id == source.repository_id)
                    || (repository_path != display
                        && display.split_once('/').map(|(_, tail)| tail)
                            != Some(repository_path.as_str())
                        && !(repository_path.is_empty()
                            && !display.is_empty()
                            && !display.contains('/')))
                {
                    return Err(failure(
                        "The retained branch source and folder paths disagree.",
                    ));
                }
            }
            Self::CreateHistorical {
                source,
                repository_path,
                ..
            } => {
                let current = source.current();
                let path = repository_path.as_ref().map_or("", PortablePath::as_str);
                if repository_path.is_none() != (current.directory_id == current.repository_id)
                    || (path != display
                        && display.split_once('/').map(|(_, tail)| tail) != Some(path)
                        && !(repository_path.is_none()
                            && !display.is_empty()
                            && !display.contains('/')))
                {
                    return Err(failure(
                        "The retained historical branch source and folder paths disagree.",
                    ));
                }
            }
            Self::Checkout {
                repository_id,
                directory_branch_id,
                expected_tree_id,
                expected_head,
            } => {
                storage_contract::validate_id(repository_id, "repo").map_err(decode_error)?;
                storage_contract::validate_id(directory_branch_id, "dbr").map_err(decode_error)?;
                storage_contract::validate_id(expected_tree_id, "dtree").map_err(decode_error)?;
                storage_contract::validate_id(expected_head, "commit").map_err(decode_error)?;
            }
        }
        Ok(())
    }
    pub(super) fn digest(&self, principal: &Principal) -> Result<String, api::Error> {
        storage_contract::object_hash(
            "request",
            &json!({"kind":self.kind(),"principal":principal,"body":self.input()}),
        )
        .map_err(decode_error)
    }
    pub(super) fn request(
        &self,
        slot: &str,
        display: &str,
        operation: OperationId,
    ) -> (&'static str, String, Value) {
        match self {
            Self::Rename {
                repository_id,
                directory_branch_id,
                name,
            } => (
                "PATCH",
                format!(
                    "/api/repositories/{repository_id}/directory-branches/{directory_branch_id}"
                ),
                json!({"name":name.as_str(),"operation_id":operation}),
            ),
            Self::Create { source, name, .. } => (
                "POST",
                format!(
                    "/api/file-manager/disks/{}/directory-branches",
                    api::segment(slot)
                ),
                json!({"path":display,"name":name,"operation_id":operation,"source":source}),
            ),
            Self::CreateHistorical { source, name, .. } => (
                "POST",
                format!(
                    "/api/file-manager/disks/{}/directory-branches/from-history",
                    api::segment(slot)
                ),
                json!({
                    "path":display,"name":name.as_str(),
                    "operation_id":operation,"source":source,
                }),
            ),
            Self::Checkout {
                repository_id,
                directory_branch_id,
                expected_tree_id,
                expected_head,
            } => (
                "PUT",
                format!(
                    "/api/repositories/{}/default-branch",
                    api::segment(repository_id)
                ),
                json!({
                    "operation_id":operation,"directory_branch_id":directory_branch_id,
                    "expected_tree_id":expected_tree_id,"expected_head":expected_head,
                }),
            ),
        }
    }
    pub(super) fn matches_receipt(&self, receipt: &Value) -> bool {
        if receipt["operation_kind"] != self.kind() {
            return false;
        }
        if !matches!(self, Self::Rename { .. }) && receipt["result"]["input"] != self.input() {
            return false;
        }
        let result = &receipt["result"];
        match self {
            Self::Rename {
                directory_branch_id,
                name,
                ..
            } => {
                // This existing endpoint has no result.input or CAS fields.
                // Record binds repository, branch and name through the digest.
                let Ok(result) = json_codec::from_value::<RenameReceipt>(result.clone()) else {
                    return false;
                };
                result.directory_branch_id == *directory_branch_id
                    && result.name.as_str() == name.as_str()
                    && (result.path.is_empty() || PortablePath::parse(&result.path).is_ok())
            }
            Self::Create {
                source,
                repository_path,
                name,
            } => {
                result["directory_branch_id"].as_str().is_some_and(|id| {
                    storage_contract::validate_id(id, "dbr").is_ok()
                        && id != source.directory_branch_id
                }) && result["head_tree_id"] == source.working_tree_id
                    && result["path"] == *repository_path
                    && result["name"] == *name
                    && result["selected"] == false
                    && result["materialized"] == false
            }
            Self::CreateHistorical {
                source,
                repository_path,
                name,
            } => {
                result["directory_branch_id"].as_str().is_some_and(|id| {
                    id.parse::<DirectoryBranchId>().is_ok()
                        && id != source.current().directory_branch_id
                }) && result["head_tree_id"] == source.tree_id().to_string()
                    && result["path"] == repository_path.as_ref().map_or("", PortablePath::as_str)
                    && result["name"] == name.as_str()
                    && result["selected"] == false
                    && result["materialized"] == false
                    && result["copied_files"] == "0"
                    && result["placeholder_files"] == "0"
            }
            Self::Checkout {
                directory_branch_id,
                ..
            } => {
                result["directory_branch_id"] == *directory_branch_id
                    && result["commit_id"]
                        .as_str()
                        .is_some_and(|id| storage_contract::validate_id(id, "commit").is_ok())
                    && (result["no_change"] == true
                        || (result["no_change"] == false && result["selected"] == true))
            }
        }
    }
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct RenameReceipt {
    directory_branch_id: DirectoryBranchId,
    name: BranchName,
    path: String,
    #[serialization(rename = "head_tree_id")]
    _head_tree_id: DirectoryTreeId,
}

impl Record {
    pub(in crate::files::mkdir) fn capture_directory_rename(
        state: &State,
        principal: &Principal,
        operation: OperationId,
        repository: &str,
        branch: &str,
        name: &str,
    ) -> Result<Self, api::Error> {
        // The editor carries the admitted row, which need not be the viewed
        // branch or occur in the separate working-branch listing. Its live
        // history/location gate is checked by the editor before submission.
        if state.files.repository_id != repository {
            return Err(failure(
                "The edited directory repository is no longer displayed.",
            ));
        }
        let repository_id = repository.parse().map_err(decode_error)?;
        let directory_branch_id = branch.parse().map_err(decode_error)?;
        let name = BranchName::parse(name).map_err(decode_error)?;
        Self::capture_directory(
            state,
            principal,
            operation,
            DirectoryRequest::Rename {
                repository_id,
                directory_branch_id,
                name,
            },
        )
    }

    pub(in crate::files::mkdir) fn capture_historical_directory_branch(
        state: &State,
        principal: &Principal,
        operation: OperationId,
        source: HistoricalDirectoryBranchSource,
        name: String,
    ) -> Result<Self, api::Error> {
        if state.files.repository_id != source.current().repository_id
            || state.files.namespace_head != source.current().namespace_head
        {
            return Err(failure(
                "The reviewed historical branch source no longer matches this directory.",
            ));
        }
        let repository_path = if state.files.repository_path.is_empty() {
            None
        } else {
            Some(
                PortablePath::parse(&state.files.repository_path)
                    .map_err(|source| decode_error(source.into()))?,
            )
        };
        Self::capture_directory(
            state,
            principal,
            operation,
            DirectoryRequest::CreateHistorical {
                source,
                repository_path,
                name: BranchName::parse(&name).map_err(decode_error)?,
            },
        )
    }

    pub(in crate::files::mkdir) fn capture_directory_branch(
        state: &State,
        principal: &Principal,
        operation: OperationId,
        source: DirectoryBranchCreateSource,
        name: String,
    ) -> Result<Self, api::Error> {
        Self::capture_directory(
            state,
            principal,
            operation,
            DirectoryRequest::Create {
                source,
                repository_path: state.files.repository_path.clone(),
                name,
            },
        )
    }
    pub(in crate::files::mkdir) fn capture_directory_checkout(
        state: &State,
        principal: &Principal,
        operation: OperationId,
    ) -> Result<Self, api::Error> {
        let branch = state
            .files
            .directory_preview
            .as_ref()
            .filter(|preview| preview.at_directory(&state.files.directory))
            .and_then(|preview| preview.branch())
            .ok_or_else(|| failure("The reviewed directory branch is no longer displayed."))?;
        Self::capture_directory(
            state,
            principal,
            operation,
            DirectoryRequest::Checkout {
                repository_id: state.files.repository_id.clone(),
                directory_branch_id: branch.id().to_owned(),
                expected_tree_id: branch.head_tree_id().to_owned(),
                expected_head: state.files.namespace_head.clone(),
            },
        )
    }
    fn capture_directory(
        state: &State,
        principal: &Principal,
        operation: OperationId,
        request: DirectoryRequest,
    ) -> Result<Self, api::Error> {
        let path = if state.files.directory.is_empty() {
            None
        } else {
            Some(
                name_policy::PortablePath::parse(&state.files.directory)
                    .map_err(|source| decode_error(source.into()))?,
            )
        };
        let stored = Stored {
            format: Format::DirectoryBranch,
            principal: principal.clone(),
            operation,
            slot: state.files.slot.clone(),
            path,
            namespace: Namespace::DirectoryBranch { request },
        };
        Self::encode(
            stored,
            "The original directory request exceeds 32 KiB; no request was sent.",
        )
    }
}
