//! Bounded, immutable namespace objects referenced by commits: per-directory
//! pages sorted by component lookup key, with an ordered index when one
//! directory exceeds a page. Entries carry component names; absolute paths
//! are reconstructed by traversal and must satisfy the existing path budget.
use crate::{Error, ErrorCode, Result};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};

/// Format constants of the paged namespace layout; they describe the layout,
/// never an admission budget or release version.
pub const NAMESPACE_PAGE_KIND: &str = "namespace-page";
pub const NAMESPACE_INDEX_KIND: &str = "namespace-index";
pub const NAMESPACE_PAGE_ENTRIES: usize = 128;
pub const NAMESPACE_PAGE_TARGET_BYTES: usize = 64 * 1024;
pub const NAMESPACE_INDEX_FANOUT: usize = 128;
pub const NAMESPACE_INDEX_DEPTH: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NamespaceRefKind {
    Page,
    Index,
}
impl NamespaceRefKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Page => NAMESPACE_PAGE_KIND,
            Self::Index => NAMESPACE_INDEX_KIND,
        }
    }
}
impl Serialize for NamespaceRefKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for NamespaceRefKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct KindVisitor;
        impl data_serialization::de::Visitor<'_> for KindVisitor {
            type Value = NamespaceRefKind;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("namespace-page or namespace-index")
            }
            fn visit_str<E: data_serialization::de::Error>(
                self,
                token: &str,
            ) -> std::result::Result<Self::Value, E> {
                match token {
                    NAMESPACE_PAGE_KIND => Ok(NamespaceRefKind::Page),
                    NAMESPACE_INDEX_KIND => Ok(NamespaceRefKind::Index),
                    _ => Err(E::unknown_variant(
                        token,
                        &[NAMESPACE_PAGE_KIND, NAMESPACE_INDEX_KIND],
                    )),
                }
            }
        }
        deserializer.deserialize_str(KindVisitor)
    }
}

/// A strong reference to one immutable namespace object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct NamespaceReference {
    pub kind: NamespaceRefKind,
    pub hash: String,
}
impl NamespaceReference {
    pub fn validate(&self) -> Result<()> {
        if self.hash.len() == 64
            && self
                .hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "invalid namespace object reference",
            ))
        }
    }
}

/// A commit's bounded namespace root plus its recorded total entry count.
/// The count is a placement fact, re-verified by full admission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CommitNamespace {
    pub root: NamespaceReference,
    #[serialization(with = "crate::u64_string")]
    pub entries: u64,
}

/// One direct child of a directory: a file at a frozen logical version, or a
/// subdirectory referencing its own immutable root with recorded totals.
#[derive(Clone, Debug, PartialEq)]
pub enum NamespacePageEntry {
    File {
        entry_id: String,
        name: String,
        file_id: String,
        version_id: String,
        size: u64,
    },
    Directory {
        entry_id: String,
        name: String,
        entries: u64,
        files: u64,
        subtree: NamespaceReference,
    },
}
impl NamespacePageEntry {
    pub fn entry_id(&self) -> &str {
        match self {
            Self::File { entry_id, .. } | Self::Directory { entry_id, .. } => entry_id,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Self::File { name, .. } | Self::Directory { name, .. } => name,
        }
    }
    /// The component lookup key derived from the stored display name.
    pub fn lookup_key(&self) -> String {
        name_policy::lookup_key(self.name())
    }
    pub fn is_directory(&self) -> bool {
        matches!(self, Self::Directory { .. })
    }
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct WirePageEntry {
    entry_id: String,
    name: String,
    lookup_key: String,
    kind: crate::NamespaceEntryKind,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    file_id: Option<String>,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    version_id: Option<String>,
    #[serialization(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::optional_u64_string"
    )]
    size: Option<u64>,
    #[serialization(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::optional_u64_string"
    )]
    entries: Option<u64>,
    #[serialization(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::optional_u64_string"
    )]
    files: Option<u64>,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    subtree: Option<NamespaceReference>,
}
impl Serialize for NamespacePageEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let wire = match self {
            Self::File {
                entry_id,
                name,
                file_id,
                version_id,
                size,
            } => WirePageEntry {
                entry_id: entry_id.clone(),
                name: name.clone(),
                lookup_key: name_policy::lookup_key(name),
                kind: crate::NamespaceEntryKind::File,
                file_id: Some(file_id.clone()),
                version_id: Some(version_id.clone()),
                size: Some(*size),
                entries: None,
                files: None,
                subtree: None,
            },
            Self::Directory {
                entry_id,
                name,
                entries,
                files,
                subtree,
            } => WirePageEntry {
                entry_id: entry_id.clone(),
                name: name.clone(),
                lookup_key: name_policy::lookup_key(name),
                kind: crate::NamespaceEntryKind::Directory,
                file_id: None,
                version_id: None,
                size: None,
                entries: Some(*entries),
                files: Some(*files),
                subtree: Some(subtree.clone()),
            },
        };
        wire.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for NamespacePageEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        use data_serialization::de::Error as DeError;
        let wire = WirePageEntry::deserialize(deserializer)?;
        // The stored display name must be one valid path component whose
        // derived key equals the recorded lookup key.
        if wire.name.contains('/') {
            return Err(D::Error::custom("namespace entry name is not a component"));
        }
        let component = name_policy::PortablePath::parse(&wire.name).map_err(D::Error::custom)?;
        if component.file_name() != wire.name
            || name_policy::lookup_key(&wire.name) != wire.lookup_key
        {
            return Err(D::Error::custom(
                "entry lookup key differs from its display name",
            ));
        }
        match wire.kind {
            crate::NamespaceEntryKind::File => {
                match (
                    wire.file_id,
                    wire.version_id,
                    wire.size,
                    wire.entries,
                    wire.files,
                    wire.subtree,
                ) {
                    (Some(file_id), Some(version_id), Some(size), None, None, None)
                        if !file_id.is_empty() && !version_id.is_empty() =>
                    {
                        Ok(Self::File {
                            entry_id: wire.entry_id,
                            name: wire.name,
                            file_id,
                            version_id,
                            size,
                        })
                    }
                    _ => Err(D::Error::custom("file entry fields are inconsistent")),
                }
            }
            crate::NamespaceEntryKind::Directory => {
                match (
                    wire.file_id,
                    wire.version_id,
                    wire.size,
                    wire.entries,
                    wire.files,
                    wire.subtree,
                ) {
                    (None, None, None, Some(entries), Some(files), Some(subtree)) => {
                        subtree.validate().map_err(D::Error::custom)?;
                        Ok(Self::Directory {
                            entry_id: wire.entry_id,
                            name: wire.name,
                            entries,
                            files,
                            subtree,
                        })
                    }
                    _ => Err(D::Error::custom("directory entry fields are inconsistent")),
                }
            }
        }
    }
}

macro_rules! namespace_marker {
    ($name:ident, $wire:expr) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub struct $name;
        impl Serialize for $name {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str($wire)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                struct MarkerVisitor;
                impl data_serialization::de::Visitor<'_> for MarkerVisitor {
                    type Value = $name;
                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str(concat!("the fixed marker ", stringify!($name)))
                    }
                    fn visit_str<E: data_serialization::de::Error>(
                        self,
                        token: &str,
                    ) -> std::result::Result<Self::Value, E> {
                        if token == $wire {
                            Ok($name)
                        } else {
                            Err(E::unknown_variant(token, &[$wire]))
                        }
                    }
                }
                deserializer.deserialize_str(MarkerVisitor)
            }
        }
    };
}
namespace_marker!(NamespacePageFormat, NAMESPACE_PAGE_KIND);
namespace_marker!(NamespaceIndexFormat, NAMESPACE_INDEX_KIND);

/// One directory's direct children (or one slice of them), sorted by the
/// component lookup key with unique keys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct NamespacePage {
    pub format: NamespacePageFormat,
    pub entries: Vec<NamespacePageEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct NamespaceIndexChild {
    /// The first component lookup key under this child; strictly increasing
    /// across siblings, so lookups and listing cursors binary search here.
    pub first_key: String,
    #[serialization(with = "crate::u64_string")]
    pub entries: u64,
    #[serialization(with = "crate::u64_string")]
    pub files: u64,
    #[serialization(rename = "ref")]
    pub reference: NamespaceReference,
}

/// The ordered page index of one directory whose children exceed a page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct NamespaceIndex {
    pub format: NamespaceIndexFormat,
    pub children: Vec<NamespaceIndexChild>,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/namespace_pages.rs"]
mod tests;
