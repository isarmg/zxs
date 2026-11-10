//! Immutable browser hints. A local record never proves server publication.
use super::*;
use data_serialization::{Deserialize, Serialize};
use storage_contract::history_window::HistoryRepositoryId;

mod directory;
use directory::DirectoryRequest;

const PREFIX: &str = "zxs:folder-create:";
pub(super) const MAX_RECORDS: usize = 32;
const MAX_RECORD_BYTES: usize = 32 * 1024;
const MAX_STORAGE_KEYS: u32 = 4096;

#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "folder-create-original-request")]
    OriginalRequest,
    #[serialization(rename = "ordinary-file-relocation-original-request")]
    FileRelocation,
    #[serialization(rename = "directory-branch-original-request")]
    DirectoryBranch,
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Namespace {
    DirectoryBranch { request: DirectoryRequest },
    UninitializedRoot,
    FileRelocation {
        relocation: storage_contract::file_relocation::FileRelocation,
    },
    Repository {
        repository: HistoryRepositoryId,
        path: name_policy::PortablePath,
    },
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Stored {
    format: Format,
    principal: String,
    operation: OperationId,
    slot: String,
    #[serialization(deserialize_with = "required_path")]
    path: Option<name_policy::PortablePath>,
    namespace: Namespace,
}

pub(super) struct Record {
    stored: Stored,
    pub(super) bytes: Arc<str>,
    digest: Option<String>,
    relocation_digests: std::collections::BTreeMap<&'static str, String>,
}
impl Record {
    pub(super) fn operation(&self) -> OperationId {
        self.stored.operation
    }
    pub(super) fn principal(&self) -> &str {
        &self.stored.principal
    }
    fn path(&self) -> &str {
        self.stored.path.as_ref().map_or("", name_policy::PortablePath::as_str)
    }
    pub(super) fn is_directory_checkout(&self) -> bool {
        matches!(&self.stored.namespace, Namespace::DirectoryBranch { request } if request.is_checkout())
    }
    pub(super) fn checkout_applies(&self, state: &State) -> bool {
        match &self.stored.namespace {
            Namespace::DirectoryBranch { request } => {
                state.files.slot == self.stored.slot
                    && state.files.directory == self.path()
                    && request.checkout_applies(state)
            }
            _ => true,
        }
    }
    pub(super) fn label(&self) -> String {
        let path = match &self.stored.namespace {
            Namespace::DirectoryBranch { request } => request.label(self.path()),
            Namespace::FileRelocation { relocation } => format!(
                "{} → {} (repository {})",
                self.path(),
                relocation.destination.as_str(),
                relocation.repository_id.as_str(),
            ),
            Namespace::UninitializedRoot | Namespace::Repository { .. } => {
                self.path().to_owned()
            }
        };
        format!("{}/{} · {}", self.stored.slot, path, self.operation())
    }
    pub(super) fn can_lookup(&self) -> bool {
        self.digest.is_some() || matches!(&self.stored.namespace, Namespace::FileRelocation { .. })
    }
    fn key(&self) -> String {
        format!("{}{}", prefix(self.principal()), self.operation())
    }
    pub(super) fn request(&self) -> (&'static str, String, Value) {
        if let Namespace::DirectoryBranch { request } = &self.stored.namespace {
            return request.request(&self.stored.slot, self.path(), self.operation());
        }
        if let Namespace::FileRelocation { relocation } = &self.stored.namespace {
            return (
                "POST",
                format!(
                    "/api/file-manager/disks/{}/file-relocation",
                    api::segment(&self.stored.slot)
                ),
                json!({"operation_id":self.operation(),"relocation":relocation}),
            );
        }
        (
            "POST",
            format!(
                "/api/file-manager/disks/{}/namespace",
                api::segment(&self.stored.slot)
            ),
            json!({
                "kind":"mkdir", "path":self.path(),
                "operation_id":self.operation(),
            }),
        )
    }
    pub(super) fn matches_receipt(&self, receipt: &Value, origin: ReceiptOrigin) -> bool {
        if let Namespace::DirectoryBranch { request } = &self.stored.namespace {
            return self.digest.as_deref() == receipt["request_digest"].as_str()
                && request.matches_receipt(receipt);
        }
        if let Namespace::FileRelocation { relocation } = &self.stored.namespace {
            let Some(kind) = receipt["operation_kind"].as_str() else {
                return false;
            };
            if !matches!(
                kind,
                "file.namespace"
                    | "file.namespace.recursive"
                    | "file.namespace.recursive-directories"
                    | "file.namespace.recursive-routes"
            ) {
                return false;
            }
            return self.relocation_digests.get(kind).map(String::as_str)
                == receipt["request_digest"].as_str()
                && receipt["result"]["original_file"] == json!(relocation)
                && receipt["result"]["action"] == "rename"
                && receipt["result"]["path"].as_str() == Some(relocation.source.as_str());
        }
        let path = match &self.stored.namespace {
            Namespace::DirectoryBranch { .. } | Namespace::FileRelocation { .. } => unreachable!("invariant: relocation handled above"),
            Namespace::UninitializedRoot => self.path(),
            Namespace::Repository { path, .. } => path.as_str(),
        };
        receipt["operation_kind"] == "file.namespace"
            && receipt["result"]["action"] == "mkdir"
            && receipt["result"]["path"].as_str() == Some(path)
            && match &self.digest {
                Some(digest) => receipt["request_digest"].as_str() == Some(digest.as_str()),
                // The first empty disk has no repository ID until the server
                // creates its root. Only the exact original POST can bind its
                // receipt; a generic ID lookup cannot prove the missing input.
                None => origin == ReceiptOrigin::OriginalRequest,
            }
    }
    pub(super) fn capture(
        state: &State,
        operation: OperationId,
        name: &str,
    ) -> Result<Self, api::Error> {
        let path = name_policy::PortablePath::parse(&super::super::model::join_path(
            &state.files.directory,
            name,
        ))
        .map_err(|source| decode_error(source.into()))?;
        let namespace = if state.files.repository_id.is_empty() {
            if !state.files.directory.is_empty() || !state.files.repository_path.is_empty() {
                return Err(failure(
                    "The folder listing has no admitted repository identity.",
                ));
            }
            Namespace::UninitializedRoot
        } else {
            Namespace::Repository {
                repository: HistoryRepositoryId::parse(&state.files.repository_id)
                    .map_err(decode_error)?,
                path: name_policy::PortablePath::parse(&super::super::model::join_path(
                    &state.files.repository_path,
                    name,
                ))
                .map_err(|source| decode_error(source.into()))?,
            }
        };
        let stored = Stored {
            format: Format::OriginalRequest,
            principal: state.principal.clone(),
            operation,
            slot: state.files.slot.clone(),
            path: Some(path),
            namespace,
        };
        let bytes = storage_contract::canonical_json(&stored).map_err(decode_error)?;
        let bytes = String::from_utf8(bytes).expect("invariant: canonical JSON is UTF-8");
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(limit(
                "The original folder request exceeds 32 KiB; no request was sent.",
            ));
        }
        Self::bind(stored, Arc::from(bytes))
    }
    pub(super) fn capture_relocation(
        state: &State,
        operation: OperationId,
        relocation: storage_contract::file_relocation::FileRelocation,
    ) -> Result<Self, api::Error> {
        let entry = state
            .files
            .entries
            .iter()
            .find(|entry| {
                entry.entry_id() == relocation.entry_id.as_str()
                    && entry.file_id() == relocation.file_id.as_str()
                    && entry.repository_path() == Some(relocation.source.as_str())
                    && entry.repository_id() == Some(relocation.repository_id.as_str())
                    && !entry.read_only()
                    && !entry.backup()
                    && !entry.retained_history()
            })
            .ok_or_else(|| failure("The captured file is no longer in the current listing."))?;
        if state.files.repository_id != relocation.repository_id.as_str()
            || state.files.namespace_head != relocation.expected_head.as_str()
            || state.files.directory_branches_supported
        {
            return Err(failure(
                "The original ordinary namespace changed; review it again.",
            ));
        }
        let stored = Stored {
            format: Format::FileRelocation,
            principal: state.principal.clone(),
            operation,
            slot: state.files.slot.clone(),
            path: Some(name_policy::PortablePath::parse(entry.path())
                .map_err(|source| decode_error(source.into()))?),
            namespace: Namespace::FileRelocation { relocation },
        };
        let bytes = storage_contract::canonical_json(&stored).map_err(decode_error)?;
        let bytes = String::from_utf8(bytes).expect("invariant: canonical JSON is UTF-8");
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(limit(
                "The original file move exceeds 32 KiB; no request was sent.",
            ));
        }
        Self::bind(stored, Arc::from(bytes))
    }
    fn decode(key: &str, principal: &str, bytes: &str) -> Result<Self, api::Error> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(limit(
                "A folder recovery record exceeds 32 KiB; it was preserved.",
            ));
        }
        if !bytes.trim_start().starts_with('{') {
            return Err(failure("Folder recovery must be a named request object."));
        }
        let stored: Stored = storage_contract::StrictJson::parse(bytes.as_bytes())
            .and_then(|json| json.decode::<Stored>())
            .map_err(decode_error)?;
        if stored.principal != principal {
            return Err(failure("The folder recovery account does not match."));
        }
        let record = Self::bind(stored, Arc::from(bytes))?;
        if record.key() != key {
            return Err(failure(
                "The folder operation differs from its immutable storage key.",
            ));
        }
        Ok(record)
    }
    fn bind(stored: Stored, bytes: Arc<str>) -> Result<Self, api::Error> {
        if stored.principal.is_empty() || stored.principal.len() > 1024 {
            return Err(failure("The folder recovery account is invalid."));
        }
        name_policy::validate_slot(&stored.slot).map_err(|source| decode_error(source.into()))?;
        if !matches!(
            (&stored.format, &stored.namespace),
            (Format::OriginalRequest, Namespace::UninitializedRoot | Namespace::Repository { .. })
                | (Format::FileRelocation, Namespace::FileRelocation { .. })
                | (Format::DirectoryBranch, Namespace::DirectoryBranch { .. })
        ) {
            return Err(failure("The retained request format and action disagree."));
        }
        if stored.path.is_none() && !matches!(&stored.namespace, Namespace::DirectoryBranch { .. }) {
            return Err(failure("The retained namespace request must have a path."));
        }
        let display_path = stored.path.as_ref().map_or("", name_policy::PortablePath::as_str);
        let digest = match &stored.namespace {
            Namespace::DirectoryBranch { request } => {
                request.validate(display_path)?;
                Some(request.digest(&stored.principal)?)
            }
            Namespace::FileRelocation { relocation } => {
                let source = relocation.source.as_str();
                if source != display_path
                    && display_path.split_once('/').map(|(_, tail)| tail) != Some(source)
                {
                    return Err(failure(
                        "The original file paths do not share one namespace.",
                    ));
                }
                None
            }
            Namespace::UninitializedRoot => {
                name_policy::validate_name(display_path)
                    .map_err(|source| decode_error(source.into()))?;
                None
            }
            Namespace::Repository { repository, path } => {
                // A named repository removes exactly one displayed component;
                // a disk-root repository keeps the complete displayed path.
                if path.as_str() != display_path
                    && display_path.split_once('/').map(|(_, tail)| tail)
                        != Some(path.as_str())
                {
                    return Err(failure(
                        "The original folder paths do not share one namespace.",
                    ));
                }
                let change = storage_contract::NamespaceOperation::Mkdir { path: path.clone() };
                Some(
                    storage_contract::object_hash(
                        "request",
                        &json!({
                            "kind":"file.namespace", "principal":stored.principal,
                            "body":{"repository_id":repository.as_str(),"change":change},
                        }),
                    )
                    .map_err(decode_error)?,
                )
            }
        };
        let mut relocation_digests = std::collections::BTreeMap::new();
        if let Namespace::FileRelocation { relocation } = &stored.namespace {
            for kind in [
                "file.namespace",
                "file.namespace.recursive",
                "file.namespace.recursive-directories",
                "file.namespace.recursive-routes",
            ] {
                let digest = storage_contract::object_hash(
                    "request",
                    &json!({
                        "kind":kind, "principal":stored.principal, "body":relocation.request(),
                    }),
                )
                .map_err(decode_error)?;
                relocation_digests.insert(kind, digest);
            }
        }
        Ok(Self {
            stored,
            bytes,
            digest,
            relocation_digests,
        })
    }
}
fn decode_error(source: storage_contract::Error) -> api::Error {
    let mut error = failure("The original folder request is invalid; it was preserved.");
    error.source = Some(Arc::new(api::ErrorSource::FolderCreationRecovery(source)));
    error
}
fn prefix(principal: &str) -> String {
    format!("{PREFIX}{}:", api::segment(principal))
}
fn storage() -> Result<web_sys::Storage, api::Error> {
    web_sys::window()
        .ok_or_else(|| failure("Browser storage is unavailable; no folder request was sent."))?
        .local_storage()
        .map_err(api::js_error)?
        .ok_or_else(|| failure("Browser storage is unavailable; no folder request was sent."))
}
pub(super) fn load(principal: &str) -> Result<Vec<Arc<Record>>, api::Error> {
    let storage = storage()?;
    let count = storage.length().map_err(api::js_error)?;
    if count > MAX_STORAGE_KEYS {
        return Err(limit(
            "Folder recovery exceeds the local storage scan budget.",
        ));
    }
    let prefix = prefix(principal);
    let mut records = Vec::new();
    for index in 0..count {
        let Some(key) = storage.key(index).map_err(api::js_error)? else {
            continue;
        };
        if !key.starts_with(&prefix) {
            continue;
        }
        let Some(bytes) = storage.get_item(&key).map_err(api::js_error)? else {
            continue;
        };
        if records.len() == MAX_RECORDS {
            return Err(limit(
                "Folder recovery exceeds 32 operations; nothing was removed.",
            ));
        }
        records.push(Arc::new(Record::decode(&key, principal, &bytes)?));
    }
    records.sort_by_key(|record| record.operation());
    Ok(records)
}
pub(super) fn save(record: &Record) -> Result<(), api::Error> {
    if !load(record.principal())?.is_empty() {
        return Err(failure(
            "Resolve the original folder creation before starting another.",
        ));
    }
    let storage = storage()?;
    let key = record.key();
    if storage.get_item(&key).map_err(api::js_error)?.is_some() {
        return Err(failure(
            "The folder operation key is occupied; it was not overwritten.",
        ));
    }
    storage
        .set_item(&key, &record.bytes)
        .map_err(api::js_error)?;
    verify_with(&storage, record)
}
fn verify_with(storage: &web_sys::Storage, record: &Record) -> Result<(), api::Error> {
    match storage.get_item(&record.key()).map_err(api::js_error)? {
        Some(bytes) if bytes == record.bytes.as_ref() => Ok(()),
        _ => Err(failure(
            "The original folder record is missing or changed; no request was sent.",
        )),
    }
}
pub(super) fn verify(record: &Record) -> Result<(), api::Error> {
    verify_with(&storage()?, record)
}
pub(super) fn remove(record: &Record) -> Result<(), api::Error> {
    let storage = storage()?;
    let key = record.key();
    if let Some(bytes) = storage.get_item(&key).map_err(api::js_error)? {
        if bytes != record.bytes.as_ref() {
            return Err(failure(
                "The folder recovery record changed; it was not removed.",
            ));
        }
        storage.remove_item(&key).map_err(api::js_error)?;
    }
    if storage.get_item(&key).map_err(api::js_error)?.is_some() {
        return Err(failure("Local folder recovery cleanup was not confirmed."));
    }
    Ok(())
}

// Unlike an optional field, the root path must be explicitly present as null.
fn required_path<'de, D>(deserializer: D) -> Result<Option<name_policy::PortablePath>, D::Error>
where
    D: data_serialization::Deserializer<'de>,
{
    Option::<name_policy::PortablePath>::deserialize(deserializer)
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/directory_branch_recovery_tests.rs"]
mod tests;
