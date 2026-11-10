use crate::api;
use data_serialization::{
    Deserialize, Deserializer, Serialize,
    de::{MapAccess, Visitor, value::MapAccessDeserializer},
};
use json_codec::{Value, json};
use std::sync::Arc;
use storage_contract::history_window::HistoryRepositoryId;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::files::deletion) struct Request {
    slot: String,
    repository: HistoryRepositoryId,
    owner: String,
    branch: String,
    tree: String,
    directory: String,
    path: name_policy::PortablePath,
}

fn identity_error(source: storage_contract::Error) -> api::Error {
    let mut error = api::error(
        storage_contract::ErrorCode::InvalidRequest,
        "The directory deletion identity is invalid; no request was sent.",
    );
    error.source = Some(Arc::new(api::ErrorSource::FileDeletionRecovery(source)));
    error
}

impl Request {
    pub(in crate::files::deletion) fn new(
        slot: String,
        repository: HistoryRepositoryId,
        owner: String,
        branch: String,
        tree: String,
        directory: String,
        path: name_policy::PortablePath,
    ) -> Result<Self, api::Error> {
        name_policy::validate_slot(&slot).map_err(|source| identity_error(source.into()))?;
        if owner != repository.as_str() {
            storage_contract::validate_id(&owner, "entry").map_err(identity_error)?;
        }
        storage_contract::validate_id(&branch, "dbr").map_err(identity_error)?;
        storage_contract::validate_id(&tree, "dtree").map_err(identity_error)?;
        storage_contract::validate_id(&directory, "entry").map_err(identity_error)?;
        if owner == directory {
            return Err(api::error(
                storage_contract::ErrorCode::InvalidRequest,
                "An initialized owner cannot be deleted as an ordinary directory.",
            ));
        }
        Ok(Self {
            slot,
            repository,
            owner,
            branch,
            tree,
            directory,
            path,
        })
    }

    pub(in crate::files::deletion) fn slot(&self) -> &str {
        &self.slot
    }
    pub(in crate::files::deletion) fn repository(&self) -> &HistoryRepositoryId {
        &self.repository
    }
    pub(in crate::files::deletion) fn owner(&self) -> &str {
        &self.owner
    }
    pub(in crate::files::deletion) fn branch(&self) -> &str {
        &self.branch
    }
    pub(in crate::files::deletion) fn tree(&self) -> &str {
        &self.tree
    }
    pub(in crate::files::deletion) fn directory(&self) -> &str {
        &self.directory
    }
    pub(in crate::files::deletion) fn path(&self) -> &name_policy::PortablePath {
        &self.path
    }

    fn namespace_request(&self) -> storage_contract::DirectoryNamespaceBatch {
        storage_contract::DirectoryNamespaceBatch {
            kind: storage_contract::DirectoryBatchKind::Delete,
            items: vec![storage_contract::DirectoryNamespaceItem {
                source: storage_contract::DirectoryNamespaceSource {
                    directory_branch_id: self.branch.clone(),
                    expected_tree_id: self.tree.clone(),
                    entry_id: self.directory.clone(),
                    path: self.path.as_str().into(),
                },
                target: None,
            }],
        }
    }

    pub(in crate::files::deletion) fn request(&self, operation: &str) -> (String, Value) {
        let request = self.namespace_request();
        (
            format!(
                "/api/file-manager/disks/{}/namespace-batch",
                api::segment(&self.slot)
            ),
            json!({"kind":request.kind,"items":request.items,"operation_id":operation}),
        )
    }

    pub(in crate::files::deletion) fn input(&self) -> Value {
        json!({"repository_id":self.repository.as_str(),"request":self.namespace_request()})
    }

    pub(in crate::files::deletion) fn label(&self) -> String {
        format!(
            "{}/{} · {} · {}",
            self.slot,
            self.path.as_str(),
            self.directory,
            self.owner
        )
    }

    pub(in crate::files::deletion) fn receipt_matches(
        &self,
        id: &str,
        principal: &str,
        receipt: &Value,
    ) -> bool {
        if receipt["operation_kind"] != "directory.delete" {
            return false;
        }
        let Some(sequence) = super::super::wire::publication_sequence(
            receipt,
            &["input", "directory_deletion", "deletion_nodes"],
        ) else {
            return false;
        };
        let result = &receipt["result"];
        if result["input"] != self.input() {
            return false;
        }
        let node = match super::wire::parse_node(result["directory_deletion"].clone()) {
            Ok(node) => node,
            Err(_) => return false,
        };
        node.repository_id == self.repository.as_str()
            && node.owner_directory_id == self.owner
            && node.directory_id == self.directory
            && node.directory_branch_id == self.branch
            && node.path == self.path
            && node.operation_id.to_string() == id
            && node.author == principal
            && node.sequence == sequence
            && super::wire::files_match(&node, &result["deletion_nodes"])
    }

    pub(in crate::files::deletion) fn stored(&self) -> Stored {
        Stored {
            slot: self.slot.clone(),
            repository_id: self.repository.clone(),
            owner_directory_id: self.owner.clone(),
            directory_branch_id: self.branch.clone(),
            expected_tree_id: self.tree.clone(),
            directory_id: self.directory.clone(),
            path: self.path.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::files::deletion) struct Stored {
    slot: String,
    repository_id: HistoryRepositoryId,
    owner_directory_id: String,
    directory_branch_id: String,
    expected_tree_id: String,
    directory_id: String,
    path: name_policy::PortablePath,
}
impl Stored {
    pub(in crate::files::deletion) fn admit(self) -> Result<Request, api::Error> {
        let Self {
            slot,
            repository_id,
            owner_directory_id,
            directory_branch_id,
            expected_tree_id,
            directory_id,
            path,
        } = self;
        Request::new(
            slot,
            repository_id,
            owner_directory_id,
            directory_branch_id,
            expected_tree_id,
            directory_id,
            path,
        )
    }
}

pub(in crate::files::deletion) fn stored<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Stored, D::Error> {
    struct Named;
    impl<'de> Visitor<'de> for Named {
        type Value = Stored;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a named directory-deletion request")
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            Stored::deserialize(MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_map(Named)
}
