//! Exact automatic backfill request and receipt bytes. No physical authority.
use crate::protection::InitialBackupConfiguration;
use crate::{Config, DiskConfig, Engine, Error, FileVersion, OperationResult, Placement, Result};
use crate::AutomaticBackupError;
use crate::protection::invalid_automatic_backup as invalid;
use data_serialization::{Deserialize, Serialize};
use name_policy::{Name, PortablePath};
use std::collections::BTreeMap;
use storage_contract::{
    CommitState, CurrentHealth, DiskId, EntryId, FileId, Generation, HistoryTreeId, Id, IdKind,
    OperationId, ProtectionAtPublish, RepositoryId,
};
use storage_contract::working_tree::{DiskRegistrationIdentity, RootBinding};

pub(crate) const KIND: &str = "backup.configure-automatic";
pub(crate) const DIRECTORY_KIND: &str = "backup.configure-automatic-directory";
pub(crate) const FILE_KIND: &str = "backup.configure-automatic-file";

#[derive(Clone, Copy)]
pub(crate) struct FileSelection {
    pub(crate) entry_id: EntryId,
    pub(crate) file_id: FileId,
}

#[derive(Clone, Copy)]
pub(crate) enum Selection {
    RetainedFiles,
    DirectoryTree(EntryId),
    File(FileSelection),
}
impl Selection {
    pub(crate) fn directory(self) -> Option<EntryId> {
        match self {
            Self::RetainedFiles | Self::File(_) => None,
            Self::DirectoryTree(entry) => Some(entry),
        }
    }

    pub(crate) fn kind(self) -> &'static str {
        match self {
            Self::RetainedFiles => KIND,
            Self::DirectoryTree(_) => DIRECTORY_KIND,
            Self::File(_) => FILE_KIND,
        }
    }
}
pub(crate) enum Policy {}
impl IdKind for Policy { const PREFIX: &'static str = "policy"; }
pub(crate) type PolicyId = Id<Policy>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    pub(crate) repository_id: RepositoryId,
    #[serialization(deserialize_with = "required_path")]
    pub(crate) path: Option<PortablePath>,
    pub(crate) target_copies: u32,
    pub(crate) expected_generation: Generation,
}
fn required_path<'de, D: data_serialization::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<PortablePath>, D::Error> {
    Option::<PortablePath>::deserialize(deserializer)
}
impl Input {
    pub(crate) fn scoped<'a>(
        &'a self,
        repository: &'a str,
        prefixes: &'a BTreeMap<String, String>,
        principal: &'a str,
        operation_id: OperationId,
    ) -> crate::ScopedBackupConfigurationRequest<'a> {
        crate::ScopedBackupConfigurationRequest {
            repository_id: repository,
            path: self.path.as_ref().map_or("", PortablePath::as_str),
            expected_scope_id: None,
            target_copies: self.target_copies,
            backup_prefixes: prefixes,
            expected_generation: self.expected_generation.get(),
            principal, operation_id,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct DirectoryInput {
    repository_id: RepositoryId,
    path: PortablePath,
    directory_id: EntryId,
    target_copies: u32,
    expected_generation: Generation,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct FileInput {
    repository_id: RepositoryId,
    path: PortablePath,
    entry_id: EntryId,
    file_id: FileId,
    target_copies: u32,
    expected_generation: Generation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct NamedDisk {
    registration: DiskRegistrationIdentity,
    name: Name,
    level: u32,
    #[serialization(deserialize_with = "required_upstream")]
    upstream: Option<DiskId>,
}
fn required_upstream<'de, D: data_serialization::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<DiskId>, D::Error> {
    Option::<DiskId>::deserialize(deserializer)
}
impl NamedDisk {
    pub(crate) fn disk_id(&self) -> Result<DiskId> {
        Ok(self.registration.disk_id.parse()?)
    }
    pub(crate) fn from_config(config: &Config, disk: &DiskConfig) -> Result<Self> {
        let root = config.storage_roots.get(&disk.disk_id.parse::<DiskId>()?)
            .ok_or_else(invalid)?;
        let upstream = disk.upstream_slot_id.as_ref().map(|slot| {
            config.disks.iter().find(|source| source.slot_id.eq_ignore_ascii_case(slot))
                .ok_or_else(invalid)?.disk_id.parse::<DiskId>().map_err(Error::from)
        }).transpose()?;
        Ok(Self {
            registration: root.registration.clone(), name: disk.name.clone(), level: disk.level,
            upstream,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Source {
    pub(crate) root: RootBinding,
    pub(crate) disks: Vec<NamedDisk>,
}

fn encode_error(source: json_codec::Error) -> Error {
    Error::AutomaticBackup(AutomaticBackupError::Encoding(source))
}
pub(crate) fn request(input: &Input) -> Result<json_codec::Value> {
    json_codec::to_value(input).map_err(encode_error)
}

pub(crate) fn selected_request(input: &Input, selection: Selection) -> Result<json_codec::Value> {
    match selection {
        Selection::RetainedFiles => request(input),
        Selection::DirectoryTree(directory_id) => {
            let path = input.path.clone().ok_or_else(invalid)?;
            json_codec::to_value(DirectoryInput {
                repository_id: input.repository_id, path, directory_id,
                target_copies: input.target_copies,
                expected_generation: input.expected_generation,
            }).map_err(encode_error)
        }
        Selection::File(file) => {
            let path = input.path.clone().ok_or_else(invalid)?;
            json_codec::to_value(FileInput {
                repository_id: input.repository_id,
                path,
                entry_id: file.entry_id,
                file_id: file.file_id,
                target_copies: input.target_copies,
                expected_generation: input.expected_generation,
            }).map_err(encode_error)
        }
    }
}

// This is also the original manual Initial result encoding. Moving it here
// does not reinterpret its prefix strings or change its receipt bytes.
pub(crate) fn configuration(
    history: &str,
    configured: &InitialBackupConfiguration,
    configured_at: u64,
) -> json_codec::Value {
    json_codec::json!({
        "history_tree_id":history, "policy":configured.policy,
        "scope":{"id":configured.scope.id,"kind":configured.scope.kind,
            "path":configured.scope.path}, "file_count":configured.file_count.to_string(),
        "version_count":configured.version_count.to_string(),
        "target_copies":configured.policy.target_copies,
        "backup_prefixes":configured.policy.backup_prefixes,
        "configured_at":configured_at.to_string()
    })
}
pub(crate) fn selected_outcome(
    input: &Input, selection: Selection, source: &Source, generated: &[PolicyId],
    history: &str, configured: &InitialBackupConfiguration, configured_at: u64,
) -> Result<json_codec::Value> {
    let mut value = configuration(history, configured, configured_at);
    value["automatic_request"] = selected_request(input, selection)?;
    value["automatic_source"] = json_codec::to_value(source).map_err(encode_error)?;
    value["generated_policy_ids"] = json_codec::to_value(generated).map_err(encode_error)?;
    Ok(value)
}

pub(crate) struct Decoded {
    pub(crate) input: Input,
    pub(crate) selection: Selection,
    pub(crate) source: Source,
    pub(crate) generated: Vec<PolicyId>,
    configured_at: u64,
    timestamp: u64,
    epoch: u64,
}
pub(crate) fn recognizes(result: &OperationResult) -> Result<bool> {
    let encoded = ["automatic_request", "automatic_source", "generated_policy_ids"]
        .iter().any(|key| result.result.get(*key).is_some());
    if result.kind == KIND || result.kind == DIRECTORY_KIND || result.kind == FILE_KIND {
        Ok(true)
    } else if encoded {
        Err(invalid())
    } else {
        Ok(false)
    }
}
pub(crate) fn decode(result: &OperationResult) -> Result<Decoded> {
    if result.kind != KIND && result.kind != DIRECTORY_KIND && result.kind != FILE_KIND {
        return Err(invalid());
    }
    let object = result.result.as_object().ok_or_else(invalid)?;
    let required = ["history_tree_id", "policy", "scope", "file_count", "version_count",
        "target_copies", "backup_prefixes", "configured_at", "automatic_request",
        "automatic_source", "generated_policy_ids", "timestamp", "membership_epoch",
        "version_protection"];
    if object.len() != required.len() || required.iter().any(|key| !object.contains_key(*key)) {
        return Err(invalid());
    }
    let ids = object["generated_policy_ids"].as_array().ok_or_else(invalid)?;
    // The shared transformation creates at most one policy and one retained policy.
    if ids.len() > 2 { return Err(invalid()); }
    let decimal = |key| -> Result<u64> {
        Ok(storage_contract::parse_u64_decimal(
            object[key].as_str().ok_or_else(invalid)?,
        )?)
    };
    let (input, selection) = if result.kind == DIRECTORY_KIND {
        let input: DirectoryInput = json_codec::from_value(object["automatic_request"].clone())
            .map_err(encode_error)?;
        (Input {
            repository_id: input.repository_id, path: Some(input.path),
            target_copies: input.target_copies, expected_generation: input.expected_generation,
        }, Selection::DirectoryTree(input.directory_id))
    } else if result.kind == FILE_KIND {
        let input: FileInput = json_codec::from_value(object["automatic_request"].clone())
            .map_err(encode_error)?;
        (
            Input {
                repository_id: input.repository_id,
                path: Some(input.path),
                target_copies: input.target_copies,
                expected_generation: input.expected_generation,
            },
            Selection::File(FileSelection {
                entry_id: input.entry_id,
                file_id: input.file_id,
            }),
        )
    } else {
        (json_codec::from_value(object["automatic_request"].clone()).map_err(encode_error)?,
            Selection::RetainedFiles)
    };
    Ok(Decoded {
        input, selection,
        source: json_codec::from_value(object["automatic_source"].clone())
            .map_err(encode_error)?,
        generated: json_codec::from_value(object["generated_policy_ids"].clone())
            .map_err(encode_error)?,
        configured_at: decimal("configured_at")?, timestamp: decimal("timestamp")?,
        epoch: decimal("membership_epoch")?,
    })
}

pub(crate) fn receipt(
    config: &crate::ValidatedConfig,
    decoded: &Decoded,
    history: &str,
    configured: &InitialBackupConfiguration,
    facts: &[(FileVersion, Placement)],
    operation_id: OperationId,
    sequence: u64,
    principal: &str,
) -> Result<OperationResult> {
    if decoded.epoch != config.membership_epoch() { return Err(invalid()); }
    let mut result = selected_outcome(&decoded.input, decoded.selection,
        &decoded.source, &decoded.generated,
        history, configured, decoded.configured_at)?;
    result["timestamp"] = json_codec::json!(decoded.timestamp.to_string());
    result["membership_epoch"] = json_codec::json!(decoded.epoch.to_string());
    let mut protection = Vec::new();
    for (version, placement) in facts {
        let mut slots = Vec::new();
        for slot in &placement.route_slot_ids {
            let disk = config.disks.iter().find(|disk| disk.slot_id == *slot)
                .ok_or_else(invalid)?;
            slots.push(json_codec::json!({
                "slot_id":slot, "copy_level":disk.level, "disk_id":disk.disk_id,
                "binding_generation":disk.binding_generation.to_string(),
                "state":"COMPLETE_VERIFIED"
            }));
        }
        protection.push(json_codec::json!({
            "version_id":version.version_id, "primary_slot_id":placement.primary_slot_id,
            "target_copies":placement.target_copies, "required_slots":placement.route_slot_ids,
            "topology_generation":placement.topology_generation.to_string(), "slots":slots
        }));
    }
    result["version_protection"] = json_codec::json!(protection);
    Ok(OperationResult {
        operation_id, kind: decoded.selection.kind().into(), commit_id: None, branch_id: None,
        history_tree_id: Some(history.parse::<HistoryTreeId>()?.to_string()),
        commit_state: CommitState::Published,
        protection_at_publish: if facts.is_empty() { ProtectionAtPublish::NotApplicable }
            else { ProtectionAtPublish::TargetMet },
        current_health: CurrentHealth::Healthy,
        request_digest: Engine::native_request_digest(decoded.selection.kind(), principal,
            &selected_request(&decoded.input, decoded.selection)?)?,
        sequence, result,
    })
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/persist/automatic_backup.rs"]
mod tests;
