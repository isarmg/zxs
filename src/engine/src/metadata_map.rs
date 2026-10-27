//! Canonical bounded metadata maps, shared independently of history projection.
//!
//! The current node domains and partition bounds are unchanged: state-page
//! leaves and history-index branches. Root schemas and semantic admission stay
//! with their consumers; this module owns deterministic partitioning and routing.
use crate::{
    Error, Result, canonical_json, canonical_json_value,
    disk::MAX_METADATA,
    journal::{AuthenticatedMetadataObject, PreparedMetadataObject},
};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const PAGE_ENTRIES: usize = 128;
const PAGE_TARGET_BYTES: usize = 64 * 1024;
const RADIX_DEPTH: usize = 64;

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
pub(crate) struct Reference {
    kind: ReferenceKind,
    hash: String,
}
impl Reference {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.hash.len() != RADIX_DEPTH || !hexadecimal(&self.hash) {
            return Err(invalid("history graph reference has an invalid digest"));
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct HistoryIndex {
    prefix: String,
    children: BTreeMap<String, Reference>,
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

struct Entry {
    key: String,
    value: Value,
    route: String,
    // Canonical key, colon, and value, without surrounding braces or commas.
    encoded_bytes: usize,
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
        let hash = object.hash.clone();
        if self.seen.insert((kind, hash.clone())) {
            self.objects.push(object);
        }
        Ok(hash)
    }

    pub(crate) fn map(&mut self, map: Map<String, Value>) -> Result<Reference> {
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
