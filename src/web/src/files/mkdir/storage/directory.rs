//! Typed retained commands derive both the HTTP request and server digest.
//! No retry reads the current listing or mints another operation identity.
use super::*;
use storage_contract::directory_git::DirectoryBranchCreateSource;

#[derive(Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum DirectoryRequest {
    Create {
        source: DirectoryBranchCreateSource,
        repository_path: String,
        name: String,
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
            Self::Create { .. } => false,
        }
    }
    pub(super) fn label(&self, path: &str) -> String {
        match self {
            Self::Create { name, .. } => format!("{path} · create branch {name}"),
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
            Self::Create { .. } => "directory.git.branch-create",
            Self::Checkout { .. } => "directory.git.checkout",
        }
    }
    fn input(&self) -> Value {
        match self {
            Self::Create {
                source,
                repository_path,
                name,
            } => json!({
                "repository_id":source.repository_id,
                "path":repository_path, "name":name, "source":source,
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
    pub(super) fn digest(&self, principal: &str) -> Result<String, api::Error> {
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
            Self::Create { source, name, .. } => (
                "POST",
                format!(
                    "/api/file-manager/disks/{}/directory-branches",
                    api::segment(slot)
                ),
                json!({"path":display,"name":name,"operation_id":operation,"source":source}),
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
        if receipt["operation_kind"] != self.kind() || receipt["result"]["input"] != self.input() {
            return false;
        }
        let result = &receipt["result"];
        match self {
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
impl Record {
    pub(in crate::files::mkdir) fn capture_directory_branch(
        state: &State,
        operation: OperationId,
        source: DirectoryBranchCreateSource,
        name: String,
    ) -> Result<Self, api::Error> {
        Self::capture_directory(
            state,
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
            principal: state.principal.clone(),
            operation,
            slot: state.files.slot.clone(),
            path,
            namespace: Namespace::DirectoryBranch { request },
        };
        let bytes = storage_contract::canonical_json(&stored).map_err(decode_error)?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(limit(
                "The original directory request exceeds 32 KiB; no request was sent.",
            ));
        }
        let bytes = String::from_utf8(bytes).expect("invariant: canonical JSON is UTF-8");
        Self::bind(stored, Arc::from(bytes))
    }
}
