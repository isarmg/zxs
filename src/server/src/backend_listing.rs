//! A parsed directory listing and its owned, page-local observation.
use crate::{ApiError, ContentDispositionSnapshot, DirectoryBranchChoice};
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 500;
pub(crate) const MAX_CURSOR_BYTES: usize = 32_768;
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct SortKey {
    pub(crate) file: bool,
    pub(crate) folded: String,
    pub(crate) normalized: String,
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) identity: String,
}
impl SortKey {
    #[cfg(feature = "core")]
    pub(crate) fn new(directory: bool, name: &str, path: &str, identity: &str) -> Self {
        Self {
            file: !directory,
            folded: name.to_lowercase(),
            normalized: name_policy::lookup_key(name),
            name: name.into(),
            path: path.into(),
            identity: identity.into(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ListingCursor {
    pub(crate) snapshot: String,
    pub(crate) request: String,
    pub(crate) backward: bool,
    pub(crate) key: SortKey,
}

/// Static query constraints only; cursors never confer admission or snapshot validity.
pub struct ReadListing {
    pub(crate) slot_id: String,
    pub(crate) path: Option<name_policy::LookupPath>,
    pub(crate) limit: usize,
    pub(crate) query: String,
    pub(crate) cursor: Option<ListingCursor>,
    pub(crate) selected: Option<(String, String)>,
}
impl ReadListing {
    pub fn parse(slot_id: String, mut values: BTreeMap<String, String>) -> Result<Self, ApiError> {
        if slot_id.is_empty() {
            return Err(ApiError::invalid("missing string field slot_id"));
        }
        let path = values
            .remove("path")
            .filter(|path| !path.is_empty())
            .map(|path| name_policy::LookupPath::parse(&path).map_err(ApiError::invalid_source))
            .transpose()?;
        if let Some(key) = values.keys().find(|key| {
            ![
                "path",
                "query",
                "limit",
                "cursor",
                "selected_path",
                "selected_entry_id",
            ]
            .contains(&key.as_str())
        }) {
            return Err(ApiError::invalid(format!("unknown field {key}")));
        }
        let limit = values
            .remove("limit")
            .map(|value| value.parse::<usize>().map_err(ApiError::invalid_source))
            .transpose()?
            .unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(ApiError::invalid("limit must be between 1 and 500"));
        }
        let query = values.remove("query").unwrap_or_default();
        if query.len() > 1024 {
            return Err(ApiError::invalid("query exceeds 1024 UTF-8 bytes"));
        }
        let selected = match (
            values.remove("selected_path"),
            values.remove("selected_entry_id"),
        ) {
            (None, None) => None,
            (Some(selected_path), Some(id)) if !id.is_empty() && id.len() <= 256 => {
                name_policy::validate_lookup_name(
                    selected_path
                        .rsplit('/')
                        .next()
                        .expect("invariant: split has at least one element"),
                )
                .map_err(ApiError::invalid_source)?;
                let parent = selected_path
                    .rsplit_once('/')
                    .map(|(parent, _)| parent)
                    .unwrap_or("");
                if name_policy::path_key(parent)
                    != path.as_ref().map(|path| path.key()).unwrap_or("")
                {
                    return Err(ApiError::invalid(
                        "selected entry must be a direct child of the requested directory",
                    ));
                }
                Some((name_policy::path_key(&selected_path), id))
            }
            (Some(_), Some(_)) | (Some(_), None) | (None, Some(_)) => {
                return Err(ApiError::invalid(
                    "selected_path and selected_entry_id must be supplied together",
                ));
            }
        };
        let cursor = values
            .remove("cursor")
            .map(|value| decode(&value))
            .transpose()?;
        Ok(Self {
            slot_id,
            path,
            limit,
            query: query.to_lowercase(),
            cursor,
            selected,
        })
    }
    pub fn limit(&self) -> usize {
        self.limit
    }
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn cursor(&self) -> Option<&ListingCursor> {
        self.cursor.as_ref()
    }
    pub fn selected(&self) -> Option<(&str, &str)> {
        self.selected
            .as_ref()
            .map(|(path, id)| (path.as_str(), id.as_str()))
    }
    pub fn slot_id(&self) -> &str {
        &self.slot_id
    }
    pub fn path(&self) -> &str {
        self.path.as_ref().map(|path| path.as_str()).unwrap_or("")
    }
}
fn decode(value: &str) -> Result<ListingCursor, ApiError> {
    if value.is_empty() || value.len() > MAX_CURSOR_BYTES || !value.len().is_multiple_of(2) {
        return Err(ApiError::invalid("invalid listing cursor length"));
    }
    let bytes = value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                _ => Err(ApiError::invalid("invalid listing cursor encoding")),
            };
            Ok((digit(pair[0])? << 4) | digit(pair[1])?)
        })
        .collect::<Result<Vec<u8>, ApiError>>()?;
    json_codec::from_slice(&bytes).map_err(ApiError::invalid_source)
}

pub struct ListingSnapshot {
    pub slot_id: String,
    pub path: String,
    pub backup: bool,
    pub entries: Vec<ListingEntry>,
    pub selected_entry: Option<ListingEntry>,
    pub context: Option<ListingContext>,
    pub directory_file_count: usize,
    pub directory_branches: Vec<DirectoryBranchChoice>,
    pub directory_branches_supported: bool,
    pub default_branch_scope: bool,
    pub total_entries: usize,
    pub snapshot: String,
    pub previous_cursor: Option<String>,
    pub next_cursor: Option<String>,
}
#[derive(Serialize)]
pub struct ListingContext {
    pub repository_id: String,
    pub repository_path: String,
    pub branch_id: String,
    pub base_head: String,
}
pub struct ListingEntry {
    pub entry_id: String,
    pub name: String,
    pub path: String,
    pub kind: storage_contract::NamespaceEntryKind,
    pub size: u64,
    pub file_id: String,
    pub version: ListingVersion,
    pub location: Option<ListingLocation>,
    pub note: Option<String>,
    pub node_id: Option<String>,
    pub read_only: Option<bool>,
    pub backup: bool,
    pub historical_only: Option<bool>,
}
pub enum ListingVersion {
    Directory(String),
    File {
        content: ContentDispositionSnapshot,
        backup_version: Option<String>,
    },
}
#[derive(Serialize)]
pub struct ListingLocation {
    pub repository_id: String,
    pub repository_path: Option<String>,
    pub container: bool,
    pub selected_branch_id: Option<String>,
}
#[derive(Serialize)]
pub(crate) struct ListingResponse {
    slot_id: String,
    path: String,
    #[serialization(skip_serializing_if = "is_false")]
    backup: bool,
    writable: bool,
    entries: Vec<ListingEntryResponse>,
    selected_entry: Option<ListingEntryResponse>,
    context: Option<ListingContext>,
    directory_file_count: String,
    directory_branches: Vec<crate::backend_directory_scopes::DirectoryChoiceRecord>,
    directory_branches_supported: bool,
    default_branch_scope: bool,
    total_entries: String,
    snapshot: String,
    previous_cursor: Option<String>,
    next_cursor: Option<String>,
}
fn is_false(value: &bool) -> bool {
    !*value
}
#[derive(Serialize)]
pub(crate) struct ListingEntryResponse {
    entry_id: String,
    name: String,
    path: String,
    kind: storage_contract::NamespaceEntryKind,
    #[serialization(with = "storage_contract::u64_string")]
    size: u64,
    file_id: String,
    #[serialization(flatten)]
    version: ListingVersionResponse,
    #[serialization(flatten)]
    location: Option<ListingLocation>,
    #[serialization(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serialization(skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
    #[serialization(skip_serializing_if = "Option::is_none")]
    read_only: Option<bool>,
    #[serialization(skip_serializing_if = "is_false")]
    backup: bool,
    #[serialization(skip_serializing_if = "Option::is_none")]
    historical_only: Option<bool>,
}
#[derive(Serialize)]
#[serialization(untagged)]
enum ListingVersionResponse {
    Directory { version_id: String },
    File(crate::backend_branch::ContentDispositionResponse),
}
impl From<ListingEntry> for ListingEntryResponse {
    fn from(entry: ListingEntry) -> Self {
        let ListingEntry {
            entry_id,
            name,
            path,
            kind,
            size,
            file_id,
            version,
            location,
            note,
            node_id,
            read_only,
            backup,
            historical_only,
        } = entry;
        let version = match version {
            ListingVersion::Directory(version_id) => {
                ListingVersionResponse::Directory { version_id }
            }
            ListingVersion::File {
                content,
                backup_version,
            } => {
                let mut response = crate::backend_branch::ContentDispositionResponse::from(content);
                if let Some(version) = backup_version {
                    response.version_id = Some(version);
                }
                ListingVersionResponse::File(response)
            }
        };
        Self {
            entry_id,
            name,
            path,
            kind,
            size,
            file_id,
            version,
            location,
            note,
            node_id,
            read_only,
            backup,
            historical_only,
        }
    }
}
impl From<ListingSnapshot> for ListingResponse {
    fn from(snapshot: ListingSnapshot) -> Self {
        let ListingSnapshot {
            slot_id,
            path,
            backup,
            entries,
            selected_entry,
            context,
            directory_file_count,
            directory_branches,
            directory_branches_supported,
            default_branch_scope,
            total_entries,
            snapshot,
            previous_cursor,
            next_cursor,
        } = snapshot;
        Self {
            slot_id,
            path,
            backup,
            writable: !backup,
            entries: entries.into_iter().map(Into::into).collect(),
            selected_entry: selected_entry.map(Into::into),
            context,
            directory_file_count: directory_file_count.to_string(),
            directory_branches: directory_branches.into_iter().map(Into::into).collect(),
            directory_branches_supported,
            default_branch_scope,
            total_entries: total_entries.to_string(),
            snapshot,
            previous_cursor,
            next_cursor,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_listing.rs"]
mod tests;
