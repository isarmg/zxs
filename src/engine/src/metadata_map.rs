//! Canonical bounded metadata maps, shared independently of history projection.
//!
//! The current graph uses state-page leaves and metadata-index branches.
//! Root schemas and semantic admission stay
//! with their consumers; this module owns deterministic partitioning and routing.
use crate::{
    Error, Result, canonical_json, canonical_json_value,
    disk::MAX_METADATA,
    journal::{AuthenticatedMetadataObject, PreparedMetadataObject},
};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Map, Value};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};
mod proof;
mod update;
pub(crate) use proof::{InclusionProof, lookup, read_proof};
pub(crate) use update::{HistoryBinding, MapRoot, MapUpdate, ProofMeasure};

/// Every current aggregate root carries this semantic discriminator.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub(crate) enum RootFormat {
    #[serialization(rename = "radix-map-roots")]
    RadixMapRoots,
}

const PAGE_ENTRIES: usize = 128;
const PAGE_TARGET_BYTES: usize = 64 * 1024;
const RADIX_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) enum ReferenceKind {
    #[serialization(rename = "state-page")]
    Page,
    #[serialization(rename = "metadata-index")]
    Index,
}
impl ReferenceKind {
    pub(crate) fn domain(self) -> &'static str {
        match self {
            Self::Page => "state-page",
            Self::Index => "metadata-index",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Reference {
    kind: ReferenceKind,
    hash: String,
}
impl Reference {
    pub(crate) fn kind(&self) -> ReferenceKind {
        self.kind
    }

    pub(crate) fn hash(&self) -> &str {
        &self.hash
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.hash.len() != RADIX_DEPTH || !hexadecimal(&self.hash) {
            return Err(invalid("metadata map reference has an invalid digest"));
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct MetadataIndex {
    pub(crate) prefix: String,
    pub(crate) children: BTreeMap<String, Reference>,
}

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

struct Entry<K = String> {
    key: K,
    text: Option<String>,
    value: Value,
    route: String,
    // Canonical key, colon, and value, without surrounding braces or commas.
    encoded_bytes: usize,
}

fn encode_entries<K>(
    entries: impl IntoIterator<Item = (K, Value)>,
    key_text: &impl Fn(&K) -> Cow<'_, str>,
) -> Result<Vec<Entry<K>>> {
    entries
        .into_iter()
        .map(|(key, value)| {
            let text = key_text(&key);
            let encoded_bytes =
                canonical_json(&text)?.len() + 1 + canonical_json_value(&value)?.len();
            if encoded_bytes > MAX_METADATA - 2 {
                return Err(Error::new(
                    storage_contract::ErrorCode::ResourceLimit,
                    "one metadata entity exceeds the metadata reader limit",
                ));
            }
            let route = key_route(&text);
            let text = match text {
                Cow::Borrowed(_) => None,
                Cow::Owned(text) => Some(text),
            };
            Ok(Entry {
                route,
                key,
                text,
                value,
                encoded_bytes,
            })
        })
        .collect()
}

fn leaf_fits(entries: usize, encoded_bytes: usize) -> bool {
    entries <= PAGE_ENTRIES && (encoded_bytes <= PAGE_TARGET_BYTES || entries == 1)
}

pub(crate) struct Encoder {
    maximum: usize,
    objects: Vec<PreparedMetadataObject>,
    seen: BTreeSet<(&'static str, String)>,
}
impl Encoder {
    pub(crate) fn new(maximum: usize) -> Self {
        Self {
            maximum: maximum.min(MAX_METADATA),
            objects: Vec::new(),
            seen: BTreeSet::new(),
        }
    }

    pub(crate) fn finish(self) -> Vec<PreparedMetadataObject> {
        self.objects
    }

    pub(crate) fn object<T: Serialize>(&mut self, kind: &'static str, value: &T) -> Result<String> {
        let object = PreparedMetadataObject::new(kind, value, self.maximum)?;
        Ok(self.prepared(object))
    }
    pub(crate) fn prepared(&mut self, object: PreparedMetadataObject) -> String {
        let hash = object.hash.clone();
        if self.seen.insert((object.kind(), hash.clone())) {
            self.objects.push(object);
        }
        hash
    }

    pub(crate) fn map(&mut self, map: Map<String, Value>) -> Result<Reference> {
        Ok(self.map_root(map, ProofMeasure::Plain)?.reference().clone())
    }

    pub(crate) fn map_root(
        &mut self,
        map: Map<String, Value>,
        measure: ProofMeasure,
    ) -> Result<MapRoot> {
        self.map_typed(map, measure, &|key: &String| Cow::Borrowed(key.as_str()))
    }

    pub(crate) fn map_typed<K: Ord + Clone>(
        &mut self,
        entries: impl IntoIterator<Item = (K, Value)>,
        measure: ProofMeasure,
        key_text: &impl Fn(&K) -> Cow<'_, str>,
    ) -> Result<MapRoot<K>> {
        self.node_root(
            encode_entries(entries, key_text)?,
            String::new(),
            measure,
            key_text,
        )
    }

    fn node_root<K: Ord + Clone>(
        &mut self,
        entries: Vec<Entry<K>>,
        prefix: String,
        measure: ProofMeasure,
        key_text: &impl Fn(&K) -> Cow<'_, str>,
    ) -> Result<MapRoot<K>> {
        let summary = update::Summary::from_entries(&entries)?;
        if leaf_fits(summary.entries, summary.page_bytes()) {
            let page = entries
                .iter()
                .map(|entry| {
                    let text = entry
                        .text
                        .as_deref()
                        .map(Cow::Borrowed)
                        .unwrap_or_else(|| key_text(&entry.key));
                    (text, &entry.value)
                })
                .collect::<BTreeMap<_, _>>();
            let tail = update::tombstone_tail(
                page.iter().map(|(key, value)| (key.as_ref(), *value)),
                summary,
                measure,
            )?;
            let hash = self.object("state-page", &page)?;
            drop(page);
            Ok(MapRoot::leaf(
                entries.into_iter().map(|entry| entry.key).collect(),
                hash,
                summary,
                tail,
            ))
        } else {
            let depth = prefix.len();
            if depth >= RADIX_DEPTH {
                return Err(Error::new(
                    storage_contract::ErrorCode::ResourceLimit,
                    "metadata entity keys cannot be divided within the radix depth limit",
                ));
            }
            let mut buckets: [Vec<Entry<K>>; 16] = std::array::from_fn(|_| Vec::new());
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
                    children.insert(
                        digit.clone(),
                        self.node_root(entries, format!("{prefix}{digit}"), measure, key_text)?,
                    );
                }
            }
            self.index_root(prefix, children)
        }
    }

    fn index_root<K: Ord + Clone>(
        &mut self,
        prefix: String,
        children: BTreeMap<String, MapRoot<K>>,
    ) -> Result<MapRoot<K>> {
        let index = MetadataIndex {
            prefix,
            children: children
                .iter()
                .map(|(digit, child)| (digit.clone(), child.reference().clone()))
                .collect(),
        };
        let object = PreparedMetadataObject::new("metadata-index", &index, self.maximum)?;
        let bytes = object.bytes().len();
        let hash = self.prepared(object);
        MapRoot::index(hash, bytes, children)
    }
}

pub(crate) struct Resolver<F> {
    load: F,
    objects: Vec<AuthenticatedMetadataObject>,
    seen: BTreeMap<(String, String), usize>,
}
impl<F: FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>> Resolver<F> {
    pub(crate) fn new(load: F) -> Self {
        Self {
            load,
            objects: Vec::new(),
            seen: BTreeMap::new(),
        }
    }

    pub(crate) fn map(&mut self, reference: &Reference) -> Result<Map<String, Value>> {
        reference.validate()?;
        let mut entries = Map::new();
        self.node(reference, "", &mut entries)?;
        Ok(entries)
    }

    pub(crate) fn finish(self) -> Vec<AuthenticatedMetadataObject> {
        self.objects
    }

    fn node(
        &mut self,
        reference: &Reference,
        prefix: &str,
        entries: &mut Map<String, Value>,
    ) -> Result<()> {
        // The root is admitted at map entry; decoded index edges already carry
        // checked addresses and digits. Prefixes are constructed from those edges.
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
                    .ok_or_else(|| invalid("metadata map leaf is not a map"))?;
                PageShape::read(object)?.validate_at(prefix)?;
                for (key, value) in page {
                    if entries.insert(key.clone(), value.clone()).is_some() {
                        return Err(invalid("duplicate metadata entity in graph"));
                    }
                }
            }
            ReferenceKind::Index => {
                let index = decode_index(object)?;
                if index.prefix != prefix {
                    return Err(invalid("invalid metadata map index"));
                }
                for (digit, child) in index.children {
                    self.node(&child, &format!("{prefix}{digit}"), entries)?;
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

/// Snapshot readers need logical entries, not a second retained copy of every
/// authenticated object's bytes/Value. Stream bounded leaves into their map.
pub(crate) fn read_map(
    reference: &Reference,
    load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<Map<String, Value>> {
    let mut entries = Map::new();
    visit_objects(reference, load, |object| {
        if object.kind() == "state-page" {
            for (key, value) in object
                .value()
                .as_object()
                .expect("invariant: visited leaf passed PageShape")
            {
                if entries.insert(key.clone(), value.clone()).is_some() {
                    return Err(invalid("duplicate metadata entity in graph"));
                }
            }
        }
        Ok(())
    })?;
    Ok(entries)
}

/// Streaming dependency-first traversal. Full recovery never substitutes a
/// re-encoded layout, and only one path of bounded objects is retained at once.
pub(crate) fn visit_objects(
    reference: &Reference,
    mut load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
    mut visit: impl FnMut(AuthenticatedMetadataObject) -> Result<()>,
) -> Result<()> {
    fn node(
        reference: &Reference,
        prefix: &str,
        load: &mut impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
        visit: &mut impl FnMut(AuthenticatedMetadataObject) -> Result<()>,
    ) -> Result<()> {
        let object = load(reference.kind.domain(), reference.hash())?;
        match reference.kind {
            ReferenceKind::Page => PageShape::read(&object)?.validate_at(prefix)?,
            ReferenceKind::Index => {
                let index = decode_index(&object)?;
                if index.prefix != prefix {
                    return Err(invalid("metadata map routing prefix differs"));
                }
                for (digit, child) in index.children {
                    node(&child, &format!("{prefix}{digit}"), load, visit)?;
                }
            }
        }
        visit(object)
    }
    reference.validate()?;
    node(reference, "", &mut load, &mut visit)
}

/// Parse one current index schema for both full resolution and closure walking.
pub(crate) fn decode_index(object: &AuthenticatedMetadataObject) -> Result<MetadataIndex> {
    let index: MetadataIndex = json_codec::from_value(object.value().clone())?;
    if index.prefix.len() >= RADIX_DEPTH
        || !hexadecimal(&index.prefix)
        || index.children.is_empty()
        || index.children.len() > 16
    {
        return Err(invalid("invalid metadata map index"));
    }
    for (digit, child) in &index.children {
        if digit.len() != 1 || !hexadecimal(digit) {
            return Err(invalid("invalid metadata map radix digit"));
        }
        child.validate()?;
    }
    Ok(index)
}

/// Bounded shape of a decoded map. A common key-route prefix proves every key
/// belongs to a requested radix edge; no logical values survive in this summary.
pub(crate) struct PageShape {
    entries: usize,
    encoded_bytes: usize,
    common_route: Option<String>,
}

impl PageShape {
    pub(crate) fn read(object: &AuthenticatedMetadataObject) -> Result<Self> {
        let page = object
            .value()
            .as_object()
            .ok_or_else(|| invalid("metadata page is not a map"))?;
        let mut common_route: Option<String> = None;
        for key in page.keys() {
            let route = key_route(key);
            match &mut common_route {
                Some(common) => {
                    let length = common
                        .bytes()
                        .zip(route.bytes())
                        .take_while(|(a, b)| a == b)
                        .count();
                    common.truncate(length);
                }
                None => common_route = Some(route),
            }
        }
        Ok(Self {
            entries: page.len(),
            encoded_bytes: object.bytes().len(),
            common_route,
        })
    }

    pub(crate) fn validate_at(&self, prefix: &str) -> Result<()> {
        if prefix.len() > RADIX_DEPTH || !hexadecimal(prefix) {
            return Err(invalid("invalid metadata map routing prefix"));
        }
        if self.entries > PAGE_ENTRIES
            || (self.encoded_bytes > PAGE_TARGET_BYTES && self.entries != 1)
            || (self.entries == 0 && !prefix.is_empty())
        {
            return Err(invalid("metadata map leaf exceeds its format bounds"));
        }
        if self
            .common_route
            .as_ref()
            .is_some_and(|route| !route.starts_with(prefix))
        {
            return Err(invalid("metadata entity is in the wrong radix branch"));
        }
        Ok(())
    }
}
