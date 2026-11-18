//! A bounded directory response is admitted once, before it enters UI state.
use crate::api;
use json_codec::Value;
use storage_contract::NamespaceEntryKind;

mod entry;
mod ingress;
mod object;

pub(super) const PAGE_SIZE: usize = 200;

/// Request identity derived from admitted navigation or a parsed destination.
pub(super) struct DirectoryKey(String);
impl DirectoryKey {
    /// Navigation is root, an admitted entry path, or a breadcrumb prefix of it.
    pub(super) fn navigation(path: &str) -> Self {
        Self(name_policy::path_key(path))
    }

    pub(super) fn destination(path: Option<&name_policy::PortablePath>) -> Self {
        Self(path.map_or_else(String::new, |path| path.key().to_owned()))
    }
}

#[derive(Debug, error_definition::Error)]
pub enum DecodeError {
    #[error("missing directory response field {0}")]
    Missing(&'static str),
    #[error("directory response field {field} must be {expected}")]
    Field {
        field: &'static str,
        expected: &'static str,
    },
    #[error("unknown directory response field {0}")]
    Unknown(String),
    #[error("invalid directory response path {field}: {source}")]
    Path {
        field: &'static str,
        #[source]
        source: name_policy::Error,
    },
    #[error("invalid directory response integer {field}: {source}")]
    Integer {
        field: &'static str,
        #[source]
        source: std::num::ParseIntError,
    },
    #[error("invalid directory response identity {field}: {source}")]
    Identity {
        field: &'static str,
        #[source]
        source: storage_contract::Error,
    },
    #[error("inconsistent directory response: {0}")]
    Relation(&'static str),
}
impl From<DecodeError> for api::Error {
    fn from(source: DecodeError) -> Self {
        Self {
            status: 200,
            code: storage_contract::ErrorCode::InvalidDirectoryPage,
            message: "The directory page did not match the server contract.".into(),
            retryable: false,
            request_id: None,
            source: Some(std::sync::Arc::new(api::ErrorSource::DirectoryPage(source))),
        }
    }
}

pub(super) struct Page {
    parts: PageParts,
}
pub(crate) struct PageParts {
    pub(crate) entries: Vec<Entry>,
    pub(crate) selected: Option<Entry>,
    pub(crate) next: Option<String>,
    pub(crate) previous: Option<String>,
    pub(crate) total: u64,
    pub(crate) context: Option<Context>,
    pub(crate) writable: bool,
    pub(crate) directory_file_count: u64,
}
impl Page {
    pub(super) fn parse(
        value: Value,
        slot: &str,
        directory: &DirectoryKey,
        limit: usize,
        selected: Option<&Entry>,
    ) -> Result<Self, api::Error> {
        ingress::page(value, slot, directory, limit, selected).map_err(Into::into)
    }
    pub(super) fn into_parts(self) -> PageParts {
        self.parts
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Context {
    repository_id: String,
    repository_path: Option<name_policy::LookupPath>,
    base_head: String,
}
impl Context {
    pub(crate) fn base_head(&self) -> &str {
        &self.base_head
    }
    pub(crate) fn repository_id(&self) -> &str {
        &self.repository_id
    }
    pub(crate) fn repository_path(&self) -> &str {
        self.repository_path
            .as_ref()
            .map_or("", |path| path.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DisplayPath {
    text: String,
    key: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Location {
    repository_id: String,
    path: Option<name_policy::PortablePath>,
    selected_branch_id: Option<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ownership {
    Materialized,
    Inherited,
    Empty,
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum Body {
    Directory,
    Unborn {
        target: storage_contract::file_draft::DraftLocation,
        // Display-only bridge to existing row keys; request identity stays in target.
        file_id: String,
        size: u64,
    },
    File {
        file_id: String,
        size: u64,
        version_id: Option<String>,
        node_id: String,
        ownership: Ownership,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum EntryIdentity {
    Namespace(String),
    Retained(storage_contract::history_window::HistoryFileId),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Entry {
    entry_id: EntryIdentity,
    path: DisplayPath,
    body: Body,
    location: Option<Location>,
    note: Option<String>,
    backup: bool,
    read_only: bool,
}
impl Entry {
    pub(crate) fn unborn(&self) -> Option<&storage_contract::file_draft::DraftLocation> {
        match &self.body {
            Body::Unborn { target, .. } => Some(target),
            Body::Directory | Body::File { .. } => None,
        }
    }
    pub(crate) fn entry_id(&self) -> &str {
        match &self.entry_id {
            EntryIdentity::Namespace(entry) => entry,
            EntryIdentity::Retained(file) => file.as_str(),
        }
    }
    pub(crate) fn retained_history(&self) -> bool {
        matches!(self.entry_id, EntryIdentity::Retained(_))
    }
    pub(super) fn retained(file: &storage_contract::detached_history::File) -> Self {
        Self {
            entry_id: EntryIdentity::Retained(file.file_id.clone()),
            path: DisplayPath {
                text: file.recorded_name.clone(),
                key: name_policy::path_key(&file.recorded_name),
            },
            body: Body::File {
                file_id: file.file_id.to_string(),
                size: file.representative_size,
                version_id: Some(file.representative_version_id.to_string()),
                node_id: file.representative_version_id.to_string(),
                ownership: Ownership::Materialized,
            },
            location: None,
            note: None,
            backup: false,
            read_only: true,
        }
    }
    pub(crate) fn name(&self) -> &str {
        self.path
            .text
            .rsplit('/')
            .next()
            .expect("invariant: admitted paths have a name")
    }
    pub(crate) fn path(&self) -> &str {
        &self.path.text
    }
    pub(crate) fn file_id(&self) -> &str {
        match &self.body {
            Body::Directory => "",
            Body::File { file_id, .. } => file_id,
            Body::Unborn { file_id, .. } => file_id,
        }
    }
    pub(crate) fn kind(&self) -> NamespaceEntryKind {
        match self.body {
            Body::Directory => NamespaceEntryKind::Directory,
            Body::File { .. } | Body::Unborn { .. } => NamespaceEntryKind::File,
        }
    }
    pub(crate) fn size(&self) -> u64 {
        match self.body {
            Body::Directory => 0,
            Body::File { size, .. } | Body::Unborn { size, .. } => size,
        }
    }
    pub(crate) fn version_id(&self) -> Option<&str> {
        match &self.body {
            Body::Directory | Body::Unborn { .. } => None,
            Body::File { version_id, .. } => version_id.as_deref(),
        }
    }
    pub(crate) fn node_id(&self) -> Option<&str> {
        match &self.body {
            Body::Directory | Body::Unborn { .. } => None,
            Body::File { node_id, .. } => (!node_id.is_empty()).then_some(node_id.as_str()),
        }
    }
    pub(crate) fn selected_branch_id(&self) -> Option<&str> {
        self.location
            .as_ref()
            .and_then(|location| location.selected_branch_id.as_deref())
    }
    pub(crate) fn repository_id(&self) -> Option<&str> {
        self.location
            .as_ref()
            .map(|location| location.repository_id.as_str())
    }
    pub(crate) fn repository_path(&self) -> Option<&str> {
        self.location
            .as_ref()
            .and_then(|location| location.path.as_ref().map(|path| path.as_str()))
    }
    pub(crate) fn context(&self) -> Option<(&str, &str)> {
        self.repository_id().zip(self.repository_path())
    }
    pub(crate) fn backup(&self) -> bool {
        self.backup
    }
    pub(crate) fn read_only(&self) -> bool {
        self.read_only
    }
    pub(crate) fn inherited(&self) -> bool {
        matches!(
            self.body,
            Body::File {
                ownership: Ownership::Inherited,
                ..
            }
        )
    }
    pub(crate) fn visible_content(&self) -> bool {
        self.version_id().is_some()
    }
    pub(crate) fn inherited_content(&self) -> bool {
        self.visible_content() && self.inherited()
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/page/tests.rs"]
pub(crate) mod tests;
