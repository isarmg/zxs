//! Candidate-bound paged history roots. The partitioning constants belong
//! to this disk format, not to a caller's metadata admission budget.
use super::HistoryManifest;
use crate::{
    Error, HistoryTree, Repository, Result, canonical_json, canonical_json_value,
    disk::MAX_METADATA,
    journal::{AuthenticatedMetadataObject, PreparedMetadataObject},
};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const PAGE_ENTRIES: usize = 128;
const PAGE_TARGET_BYTES: usize = 64 * 1024;
const RADIX_DEPTH: usize = 64;

// This is a schema, not a list inferred from fields present in untrusted JSON.
// Even optional, omitted logical maps have explicit roots in the paged graph.
const MAP_FIELDS: &[&str] = &[
    "branches",
    "file_selections",
    "directory_branches",
    "directory_trees",
    "file_write_origins",
    "file_contexts",
    "file_path_nodes",
    "file_metadata_nodes",
    "file_branch_merges",
    "file_branch_aliases",
    "file_node_parents",
    "file_path_notes",
    "directory_tree_selections",
    "file_locations",
    "file_backup_paths",
    "file_backup_policies",
    "directory_selections",
    "directory_selection_generations",
    "file_selection_generations",
    "file_notes",
    "version_notes",
    "file_names",
    "commits",
    "versions",
    "placements",
    "merge_links",
    "boundary_marks",
    "content_states",
    "placement_rules",
    "protection_policies",
];

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum ReferenceKind {
    #[serialization(rename = "state-page")]
    Page,
    #[serialization(rename = "history-index")]
    Index,
}
impl ReferenceKind {
    fn domain(self) -> &'static str {
        match self {
            Self::Page => "state-page",
            Self::Index => "history-index",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct Reference {
    kind: ReferenceKind,
    hash: String,
}
impl Reference {
    fn validate(&self) -> Result<()> {
        if self.hash.len() != RADIX_DEPTH || !hexadecimal(&self.hash) {
            return Err(invalid("history graph reference has an invalid digest"));
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct PagedHistoryRoot {
    repository: Repository,
    history: HistoryTree,
    maps: BTreeMap<String, Reference>,
}

#[derive(Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct HistoryIndex {
    prefix: String,
    children: BTreeMap<String, Reference>,
}

pub(super) type GraphObject = AuthenticatedMetadataObject;
pub(super) type ResolvedGraph = (HistoryManifest, Vec<GraphObject>);

fn invalid(message: &str) -> Error {
    Error::new(storage_contract::ErrorCode::HistoryPackageInvalid, message)
}

fn hexadecimal(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn key_route(key: &str) -> String {
    blake3::hash(key.as_bytes()).to_hex().to_string()
}

struct Entry {
    key: String,
    value: Value,
    route: String,
    // Canonical key, colon, and value, without surrounding braces or commas.
    encoded_bytes: usize,
}

struct Encoder {
    maximum: usize,
    objects: Vec<PreparedMetadataObject>,
    seen: BTreeSet<(&'static str, String)>,
}
impl Encoder {
    fn object<T: Serialize>(&mut self, kind: &'static str, value: &T) -> Result<String> {
        let object = PreparedMetadataObject::new(kind, value, self.maximum)?;
        let hash = object.hash.clone();
        if self.seen.insert((kind, hash.clone())) {
            self.objects.push(object);
        }
        Ok(hash)
    }

    fn map(&mut self, map: Map<String, Value>) -> Result<Reference> {
        let entries = map
            .into_iter()
            .map(|(key, value)| {
                let encoded_bytes =
                    canonical_json(&key)?.len() + 1 + canonical_json_value(&value)?.len();
                // A single entity cannot be divided. Keep the existing reader
                // ceiling rather than turning the page target into a new
                // limit on Commit.entries and other existing entity payloads.
                if encoded_bytes > MAX_METADATA - 2 {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ResourceLimit,
                        "one history entity exceeds the metadata reader limit",
                    ));
                }
                Ok(Entry {
                    route: key_route(&key),
                    key,
                    value,
                    encoded_bytes,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.node(entries, String::new())
    }

    fn node(&mut self, entries: Vec<Entry>, prefix: String) -> Result<Reference> {
        let encoded_bytes = entries.iter().fold(
            2usize.saturating_add(entries.len().saturating_sub(1)),
            |size, entry| size.saturating_add(entry.encoded_bytes),
        );
        if entries.len() <= PAGE_ENTRIES
            && (encoded_bytes <= PAGE_TARGET_BYTES || entries.len() == 1)
        {
            let page = entries
                .into_iter()
                .map(|entry| (entry.key, entry.value))
                .collect::<Map<_, _>>();
            return Ok(Reference {
                kind: ReferenceKind::Page,
                hash: self.object("state-page", &page)?,
            });
        }
        let depth = prefix.len();
        if depth >= RADIX_DEPTH {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "history entity keys cannot be divided within the radix depth limit",
            ));
        }
        let mut buckets: [Vec<Entry>; 16] = std::array::from_fn(|_| Vec::new());
        for entry in entries {
            let digit = entry.route.as_bytes()[depth];
            let bucket = match digit {
                b'0'..=b'9' => digit - b'0',
                _ => digit - b'a' + 10,
            };
            buckets[usize::from(bucket)].push(entry);
        }
        let mut children = BTreeMap::new();
        for (bucket, entries) in buckets.into_iter().enumerate() {
            if !entries.is_empty() {
                let digit = format!("{bucket:x}");
                let child = self.node(entries, format!("{prefix}{digit}"))?;
                children.insert(digit, child);
            }
        }
        Ok(Reference {
            kind: ReferenceKind::Index,
            hash: self.object("history-index", &HistoryIndex { prefix, children })?,
        })
    }
}

/// Freeze the complete graph in dependency order. Admission limits may reject
/// a graph but never change its partitioning or its candidate-bound root hash.
pub(super) fn prepare(
    manifest: &HistoryManifest,
    maximum: usize,
) -> Result<(String, Vec<PreparedMetadataObject>)> {
    // Going through the logical serializer preserves all field-specific wire
    // rules, especially omitted optional maps and decimal-string u64 values.
    let Value::Object(mut logical) = json_codec::to_value(manifest)? else {
        return Err(invalid("history manifest is not an object"));
    };
    for field in ["repository", "history"] {
        logical.remove(field);
    }
    let mut encoder = Encoder {
        maximum: maximum.min(MAX_METADATA),
        objects: Vec::new(),
        seen: BTreeSet::new(),
    };
    let mut maps = BTreeMap::new();
    for &field in MAP_FIELDS {
        let value = logical
            .remove(field)
            .unwrap_or_else(|| Value::Object(Map::new()));
        let Value::Object(map) = value else {
            return Err(invalid("history manifest map is not an object"));
        };
        maps.insert(field.to_owned(), encoder.map(map)?);
    }
    if !logical.is_empty() {
        return Err(invalid(
            "history graph schema omits a logical manifest field",
        ));
    }
    let root = PagedHistoryRoot {
        repository: manifest.repository.clone(),
        history: manifest.history.clone(),
        maps,
    };
    let hash = encoder.object("history-manifest", &root)?;
    Ok((hash, encoder.objects))
}

struct Resolver<F> {
    load: F,
    objects: Vec<GraphObject>,
    seen: BTreeMap<(String, String), usize>,
}
impl<F: FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>> Resolver<F> {
    fn map(
        &mut self,
        reference: &Reference,
        prefix: &str,
        entries: &mut Map<String, Value>,
    ) -> Result<()> {
        // Validate before invoking a loader which may turn this address into a
        // filesystem path. The only possible domains come from ReferenceKind.
        reference.validate()?;
        if prefix.len() > RADIX_DEPTH || !hexadecimal(prefix) {
            return Err(invalid("invalid history graph routing prefix"));
        }
        let domain = reference.kind.domain();
        let address = (domain.to_owned(), reference.hash.clone());
        let loaded = match self.seen.get(&address) {
            Some(_) => None,
            None => Some((self.load)(domain, &reference.hash)?),
        };
        let object = match &loaded {
            Some(object) => object,
            None => &self.objects[self.seen[&address]],
        };
        let value = object.value();
        match reference.kind {
            ReferenceKind::Page => {
                let page = value
                    .as_object()
                    .ok_or_else(|| invalid("history graph leaf is not a map"))?;
                let bytes = object.bytes().len();
                if page.len() > PAGE_ENTRIES
                    || (bytes > PAGE_TARGET_BYTES && page.len() != 1)
                    || (page.is_empty() && !prefix.is_empty())
                {
                    return Err(invalid("history graph leaf exceeds its format bounds"));
                }
                for (key, value) in page {
                    if !key_route(key).starts_with(prefix) {
                        return Err(invalid("history entity is in the wrong radix branch"));
                    }
                    if entries.insert(key.clone(), value.clone()).is_some() {
                        return Err(invalid("duplicate history entity in graph"));
                    }
                }
            }
            ReferenceKind::Index => {
                let index: HistoryIndex = json_codec::from_value(value.clone())?;
                if index.prefix != prefix
                    || prefix.len() >= RADIX_DEPTH
                    || index.children.is_empty()
                    || index.children.len() > 16
                {
                    return Err(invalid("invalid history graph index"));
                }
                for (digit, child) in &index.children {
                    if digit.len() != 1 || !hexadecimal(digit) {
                        return Err(invalid("invalid history graph radix digit"));
                    }
                    child.validate()?;
                }
                for (digit, child) in index.children {
                    self.map(&child, &format!("{prefix}{digit}"), entries)?;
                }
            }
        }
        if let Some(object) = loaded {
            self.seen.insert(address, self.objects.len());
            self.objects.push(object);
        }
        Ok(())
    }
}

/// Sources authenticate each address once before the resolver checks graph shape
/// and route-dependent constraints. Returned objects retain their canonical bytes.
pub(super) fn resolve_with_objects(
    root: AuthenticatedMetadataObject,
    load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<ResolvedGraph> {
    let decoded: PagedHistoryRoot = json_codec::from_value(root.value().clone())?;
    if decoded.maps.len() != MAP_FIELDS.len()
        || MAP_FIELDS
            .iter()
            .any(|field| !decoded.maps.contains_key(*field))
    {
        return Err(invalid("history root has an invalid logical schema"));
    }
    for reference in decoded.maps.values() {
        reference.validate()?;
    }
    let mut logical = Map::new();
    logical.insert(
        "repository".to_owned(),
        json_codec::to_value(decoded.repository)?,
    );
    logical.insert("history".to_owned(), json_codec::to_value(decoded.history)?);
    let mut resolver = Resolver {
        load,
        objects: Vec::new(),
        seen: BTreeMap::new(),
    };
    for (field, reference) in decoded.maps {
        let mut entries = Map::new();
        resolver.map(&reference, "", &mut entries)?;
        logical.insert(field, Value::Object(entries));
    }
    let manifest = json_codec::from_value(Value::Object(logical))?;
    resolver.objects.push(root);
    Ok((manifest, resolver.objects))
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/history_package/graph/tests.rs"]
mod tests;
