//! Path-copy descriptors retain bounded keys and exact aggregate sizes, never values.
use super::*;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(crate) enum ProofMeasure {
    Plain,
    ContentStates,
}

#[derive(Clone, Copy)]
pub(super) struct Summary {
    pub(super) entries: usize,
    entry_bytes: usize,
}
impl Summary {
    pub(super) fn page_bytes(self) -> usize {
        2 + self.entries.saturating_sub(1) + self.entry_bytes
    }
    pub(super) fn from_entries<K>(entries: &[Entry<K>]) -> Result<Self> {
        Ok(Self {
            entries: entries.len(),
            entry_bytes: checked_sum(entries.iter().map(|e| e.encoded_bytes))?,
        })
    }
    fn combine<'a>(summaries: impl Iterator<Item = &'a Self>) -> Result<Self> {
        let mut result = Self {
            entries: 0,
            entry_bytes: 0,
        };
        for summary in summaries {
            result.entries = add(result.entries, summary.entries)?;
            result.entry_bytes = add(result.entry_bytes, summary.entry_bytes)?;
        }
        Ok(result)
    }
}
fn add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "metadata map size overflow",
        )
    })
}
fn checked_sum(mut values: impl Iterator<Item = usize>) -> Result<usize> {
    values.try_fold(0, add)
}

pub(super) fn tombstone_tail<'a>(
    page: impl Iterator<Item = (&'a str, &'a Value)>,
    summary: Summary,
    measure: ProofMeasure,
) -> Result<Option<usize>> {
    if matches!(measure, ProofMeasure::Plain) {
        return Ok(None);
    }
    let mut maximum = None;
    for (key, value) in page {
        let status: storage_contract::ContentState = json_codec::from_value(value.clone())?;
        if matches!(
            status,
            storage_contract::ContentState::Purged | storage_contract::ContentState::Migrated
        ) {
            let tail = add(
                summary.page_bytes(),
                add(
                    canonical_json(key)?.len(),
                    canonical_json_value(value)?.len(),
                )?,
            )?;
            maximum = Some(maximum.map_or(tail, |previous: usize| previous.max(tail)));
        }
    }
    Ok(maximum)
}

/// Constructed only from this encoder or a fully authenticated exact graph.
/// Cloning a root is O(1); each update copies only changed radix paths.
#[derive(Clone)]
pub(crate) struct MapRoot<K = String>(Arc<Node<K>>);
#[cfg(test)]
impl<K: Ord + Clone> std::fmt::Debug for MapRoot<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reference().fmt(f)
    }
}
#[cfg(test)]
impl<K: Ord + Clone> PartialEq for MapRoot<K> {
    fn eq(&self, other: &Self) -> bool {
        self.reference() == other.reference()
            && self.maximum_object_bytes() == other.maximum_object_bytes()
    }
}
struct Node<K> {
    reference: Reference,
    summary: Summary,
    maximum: usize,
    bytes: usize,
    catalog_bound: bool,
    catalog: Option<BTreeMap<K, HistoryBinding>>,
    // Bytes inside the proof nodes array, including its internal commas.
    proof_bytes: usize,
    // Same path plus the actual eligible version ID and status JSON lengths.
    tombstone_tail: Option<usize>,
    children: BTreeMap<String, MapRoot<K>>,
    keys: Vec<K>,
}
#[derive(Clone)]
pub(crate) struct HistoryBinding {
    pub(crate) hash: String,
    pub(crate) maximum_object_bytes: usize,
    pub(crate) encoder: Option<Arc<crate::history_package::EncodedHistory>>,
}
impl<K: Ord + Clone> MapRoot<K> {
    pub(crate) fn visit(
        &self,
        visit: &mut impl FnMut(&Reference, usize) -> Result<()>,
    ) -> Result<()> {
        self.visit_nodes("", &mut |root, _| visit(root.reference(), root.0.maximum))
    }
    pub(crate) fn visit_nodes(
        &self,
        prefix: &str,
        visit: &mut impl FnMut(&Self, &str) -> Result<()>,
    ) -> Result<()> {
        for (digit, child) in &self.0.children {
            child.visit_nodes(&format!("{prefix}{digit}"), visit)?;
        }
        visit(self, prefix)
    }
    pub(crate) fn reference(&self) -> &Reference {
        &self.0.reference
    }
    pub(crate) fn maximum_object_bytes(&self) -> usize {
        self.0.maximum
    }
    pub(crate) fn maximum_inclusion_proof_bytes(&self) -> Option<usize> {
        (self.0.summary.entries != 0).then_some(self.0.proof_bytes + 2)
    }
    pub(crate) fn maximum_tombstone_tail(&self) -> Option<usize> {
        self.0.tombstone_tail
    }
    pub(super) fn leaf(
        keys: Vec<K>,
        hash: String,
        summary: Summary,
        tombstone_tail: Option<usize>,
    ) -> Self {
        let bytes = summary.page_bytes();
        Self(Arc::new(Node {
            reference: Reference {
                kind: ReferenceKind::Page,
                hash,
            },
            summary,
            maximum: bytes,
            bytes,
            catalog_bound: false,
            catalog: None,
            proof_bytes: bytes,
            tombstone_tail,
            children: BTreeMap::new(),
            keys,
        }))
    }
    pub(super) fn index(
        hash: String,
        bytes: usize,
        children: BTreeMap<String, Self>,
    ) -> Result<Self> {
        let summary = Summary::combine(children.values().map(|n| &n.0.summary))?;
        let maximum = children.values().fold(bytes, |m, n| m.max(n.0.maximum));
        let proof_bytes = add(
            bytes + 1,
            children
                .values()
                .map(|n| n.0.proof_bytes)
                .max()
                .expect("invariant: encoded indexes are nonempty"),
        )?;
        let tombstone_tail = children
            .values()
            .filter_map(|n| n.0.tombstone_tail)
            .max()
            .map(|tail| add(bytes + 1, tail))
            .transpose()?;
        let catalog_bound = children.values().all(|child| child.0.catalog_bound);
        Ok(Self(Arc::new(Node {
            reference: Reference {
                kind: ReferenceKind::Index,
                hash,
            },
            summary,
            maximum,
            bytes,
            catalog_bound,
            catalog: None,
            proof_bytes,
            tombstone_tail,
            children,
            keys: Vec::new(),
        })))
    }
    fn keys(&self) -> impl Iterator<Item = &K> {
        self.0
            .keys
            .iter()
            .chain(self.0.catalog.iter().flat_map(|entries| entries.keys()))
    }
    fn entries(
        &self,
        lookup: &mut impl FnMut(&K) -> Result<Option<Value>>,
        key_text: &impl Fn(&K) -> Cow<'_, str>,
    ) -> Result<Vec<Entry<K>>> {
        let mut map = BTreeMap::new();
        self.collect(&mut map, lookup)?;
        encode_entries(map, key_text)
    }
    fn collect(
        &self,
        map: &mut BTreeMap<K, Value>,
        lookup: &mut impl FnMut(&K) -> Result<Option<Value>>,
    ) -> Result<()> {
        for key in self.keys() {
            let value =
                lookup(key)?.expect("invariant: unchanged map keys remain in the new state");
            map.insert(key.clone(), value);
        }
        for child in self.0.children.values() {
            child.collect(map, lookup)?;
        }
        Ok(())
    }
}

impl MapRoot {
    pub(crate) fn reconstruct_catalog_object(
        &self,
        prefix: &str,
        maximum: usize,
    ) -> Result<PreparedMetadataObject> {
        assert!(
            self.0.catalog_bound,
            "invariant: catalog reconstruction has typed bindings"
        );
        let object = match &self.0.catalog {
            Some(entries) => PreparedMetadataObject::new(
                "state-page",
                &entries
                    .iter()
                    .map(|(key, binding)| (key, &binding.hash))
                    .collect::<BTreeMap<_, _>>(),
                maximum,
            )?,
            None => PreparedMetadataObject::new(
                "metadata-index",
                &MetadataIndex {
                    prefix: prefix.into(),
                    children: self
                        .0
                        .children
                        .iter()
                        .map(|(digit, child)| (digit.clone(), child.reference().clone()))
                        .collect(),
                },
                maximum,
            )?,
        };
        assert_eq!(
            object.hash,
            self.reference().hash,
            "invariant: typed catalog descriptor reconstructs its exact node"
        );
        Ok(object)
    }
    pub(crate) fn read<'a>(
        reference: &Reference,
        measure: ProofMeasure,
        load: &impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
    ) -> Result<Self> {
        reference.validate()?;
        Self::read_at(reference, "", measure, load)
    }
    fn read_at<'a>(
        reference: &Reference,
        prefix: &str,
        measure: ProofMeasure,
        load: &impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
    ) -> Result<Self> {
        let object = load(reference)?;
        Self::read_node(reference, prefix, measure, object, &mut |child, prefix| {
            Self::read_at(child, prefix, measure, load)
        })
    }
    pub(crate) fn load(
        reference: &Reference,
        measure: ProofMeasure,
        mut load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
    ) -> Result<Self> {
        reference.validate()?;
        Self::load_at(reference, "", measure, &mut load)
    }
    fn load_at(
        reference: &Reference,
        prefix: &str,
        measure: ProofMeasure,
        load: &mut impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
    ) -> Result<Self> {
        let object = load(reference.kind.domain(), reference.hash())?;
        Self::read_node(reference, prefix, measure, &object, &mut |child, prefix| {
            Self::load_at(child, prefix, measure, load)
        })
    }
    fn read_node(
        reference: &Reference,
        prefix: &str,
        measure: ProofMeasure,
        object: &AuthenticatedMetadataObject,
        child: &mut impl FnMut(&Reference, &str) -> Result<Self>,
    ) -> Result<Self> {
        match reference.kind {
            ReferenceKind::Page => {
                let shape = PageShape::read(object)?;
                shape.validate_at(prefix)?;
                let page = object
                    .value()
                    .as_object()
                    .expect("invariant: PageShape admits a map");
                let summary = Summary {
                    entries: shape.entries,
                    entry_bytes: shape.encoded_bytes - 2 - shape.entries.saturating_sub(1),
                };
                let tail = tombstone_tail(
                    page.iter().map(|(key, value)| (key.as_str(), value)),
                    summary,
                    measure,
                )?;
                Ok(Self::leaf(
                    page.keys().cloned().collect(),
                    reference.hash.clone(),
                    summary,
                    tail,
                ))
            }
            ReferenceKind::Index => {
                let index = decode_index(object)?;
                if index.prefix != prefix {
                    return Err(invalid("metadata map routing prefix differs"));
                }
                let children = index
                    .children
                    .iter()
                    .map(|(digit, reference)| {
                        Ok((
                            digit.clone(),
                            child(reference, &format!("{prefix}{digit}"))?,
                        ))
                    })
                    .collect::<Result<_>>()?;
                Self::index(reference.hash.clone(), object.bytes().len(), children)
            }
        }
    }
    pub(crate) fn catalog_binding(&self, key: &str) -> Option<&HistoryBinding> {
        let route = key_route(key);
        let mut root = self;
        let mut depth = 0;
        loop {
            if let Some(entries) = &root.0.catalog {
                return entries.get(key);
            }
            assert!(
                root.0.catalog_bound,
                "invariant: catalog lookup uses a bound catalog descriptor"
            );
            root = root.0.children.get(&route[depth..depth + 1])?;
            depth += 1;
        }
    }
    fn bind_catalog(
        &self,
        lookup: &mut impl FnMut(&str) -> Result<Option<HistoryBinding>>,
    ) -> Result<Self> {
        if self.0.catalog_bound {
            return Ok(self.clone());
        }
        if matches!(self.reference().kind, ReferenceKind::Page) {
            let entries = self
                .keys()
                .map(|key| {
                    Ok((
                        key.clone(),
                        lookup(key)?.expect("invariant: new catalog leaf keys have bindings"),
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            let maximum = entries.values().fold(self.0.bytes, |size, binding| {
                size.max(binding.maximum_object_bytes)
            });
            return Ok(Self(Arc::new(Node {
                reference: self.0.reference.clone(),
                summary: self.0.summary,
                maximum,
                bytes: self.0.bytes,
                catalog_bound: true,
                catalog: Some(entries),
                proof_bytes: self.0.proof_bytes,
                tombstone_tail: None,
                children: BTreeMap::new(),
                keys: Vec::new(),
            })));
        }
        let children = self
            .0
            .children
            .iter()
            .map(|(digit, child)| Ok((digit.clone(), child.bind_catalog(lookup)?)))
            .collect::<Result<_>>()?;
        Self::index(self.reference().hash.clone(), self.0.bytes, children)
    }
}

enum Planned<K> {
    Reused(MapRoot<K>),
    Leaf {
        prefix: String,
        entries: Vec<Entry<K>>,
        summary: Summary,
    },
    Index {
        prefix: String,
        children: BTreeMap<String, Self>,
        summary: Summary,
    },
}
impl<K: Ord + Clone> Planned<K> {
    fn summary(&self) -> &Summary {
        match self {
            Self::Reused(root) => &root.0.summary,
            Self::Leaf { summary, .. } | Self::Index { summary, .. } => summary,
        }
    }
    fn entries(
        self,
        lookup: &mut impl FnMut(&K) -> Result<Option<Value>>,
        key_text: &impl Fn(&K) -> Cow<'_, str>,
    ) -> Result<Vec<Entry<K>>> {
        match self {
            Self::Reused(root) => root.entries(lookup, key_text),
            Self::Leaf { entries, .. } => Ok(entries),
            Self::Index { children, .. } => {
                let mut all = Vec::new();
                for child in children.into_values() {
                    all.extend(child.entries(lookup, key_text)?);
                }
                Ok(all)
            }
        }
    }
    fn encode(
        self,
        encoder: &mut Encoder,
        measure: ProofMeasure,
        reused: &mut impl FnMut(&Reference),
        key_text: &impl Fn(&K) -> Cow<'_, str>,
    ) -> Result<MapRoot<K>> {
        match self {
            Self::Reused(root) => {
                reused(root.reference());
                Ok(root)
            }
            Self::Leaf {
                prefix, entries, ..
            } => encoder.node_root(entries, prefix, measure, key_text),
            Self::Index {
                prefix, children, ..
            } => {
                let children = children
                    .into_iter()
                    .map(|(digit, child)| {
                        Ok((digit, child.encode(encoder, measure, reused, key_text)?))
                    })
                    .collect::<Result<_>>()?;
                encoder.index_root(prefix, children)
            }
        }
    }
}
fn plan<K: Ord + Clone>(
    basis: Option<&MapRoot<K>>,
    prefix: String,
    changes: Vec<(K, String)>,
    lookup: &mut impl FnMut(&K) -> Result<Option<Value>>,
    key_text: &impl Fn(&K) -> Cow<'_, str>,
) -> Result<Planned<K>> {
    if changes.is_empty() {
        return Ok(Planned::Reused(
            basis
                .expect("invariant: unchanged paths have a basis")
                .clone(),
        ));
    }
    if basis.is_none_or(|n| matches!(n.reference().kind, ReferenceKind::Page)) {
        let mut keys = changes
            .into_iter()
            .map(|(key, _)| key)
            .collect::<BTreeSet<_>>();
        if let Some(root) = basis {
            keys.extend(root.keys().cloned());
        }
        let mut map = BTreeMap::new();
        for key in keys {
            if let Some(value) = lookup(&key)? {
                map.insert(key, value);
            }
        }
        let entries = encode_entries(map, key_text)?;
        let summary = Summary::from_entries(&entries)?;
        return Ok(Planned::Leaf {
            prefix,
            entries,
            summary,
        });
    }
    let basis = basis.expect("invariant: non-leaf basis exists");
    let mut buckets = BTreeMap::<String, Vec<(K, String)>>::new();
    for (key, route) in changes {
        buckets
            .entry(route[prefix.len()..prefix.len() + 1].to_owned())
            .or_default()
            .push((key, route));
    }
    let mut children = BTreeMap::new();
    for (digit, child) in &basis.0.children {
        let changes = buckets.remove(digit).unwrap_or_default();
        let next = plan(
            Some(child),
            format!("{prefix}{digit}"),
            changes,
            lookup,
            key_text,
        )?;
        if next.summary().entries != 0 {
            children.insert(digit.clone(), next);
        }
    }
    for (digit, changes) in buckets {
        let next = plan(None, format!("{prefix}{digit}"), changes, lookup, key_text)?;
        if next.summary().entries != 0 {
            children.insert(digit, next);
        }
    }
    let summary = Summary::combine(children.values().map(Planned::summary))?;
    let result = Planned::Index {
        prefix: prefix.clone(),
        children,
        summary,
    };
    if leaf_fits(summary.entries, summary.page_bytes()) {
        Ok(Planned::Leaf {
            prefix,
            entries: result.entries(lookup, key_text)?,
            summary,
        })
    } else {
        Ok(result)
    }
}

/// Frontier references are not durability receipts: callers still verify and
/// install the complete reachable closure before publishing its aggregate root.
pub(crate) struct MapUpdate {
    pub(crate) root: MapRoot,
    pub(crate) reused_roots: Vec<Reference>,
}
impl Encoder {
    pub(crate) fn map_catalog(
        &mut self,
        bindings: &BTreeMap<String, HistoryBinding>,
    ) -> Result<MapRoot> {
        let map = bindings
            .iter()
            .map(|(key, binding)| (key.clone(), Value::String(binding.hash.clone())))
            .collect();
        self.map_root(map, ProofMeasure::Plain)?
            .bind_catalog(&mut |key| Ok(bindings.get(key).cloned()))
    }
    pub(crate) fn update_catalog(
        &mut self,
        basis: &MapRoot,
        changes: BTreeSet<String>,
        mut lookup: impl FnMut(&str) -> Result<Option<HistoryBinding>>,
    ) -> Result<MapRoot> {
        let root = self.update_map(basis, changes, ProofMeasure::Plain, |key| {
            Ok(lookup(key)?.map(|binding| Value::String(binding.hash)))
        })?;
        root.bind_catalog(&mut lookup)
    }
    pub(crate) fn update_map(
        &mut self,
        basis: &MapRoot,
        changes: BTreeSet<String>,
        measure: ProofMeasure,
        mut lookup: impl FnMut(&str) -> Result<Option<Value>>,
    ) -> Result<MapRoot> {
        self.update_typed(
            basis,
            changes,
            measure,
            &|key: &String| Cow::Borrowed(key.as_str()),
            |key| lookup(key),
        )
    }
    pub(crate) fn update_typed<K: Ord + Clone>(
        &mut self,
        basis: &MapRoot<K>,
        changes: BTreeSet<K>,
        measure: ProofMeasure,
        key_text: &impl Fn(&K) -> Cow<'_, str>,
        mut lookup: impl FnMut(&K) -> Result<Option<Value>>,
    ) -> Result<MapRoot<K>> {
        let changes = changes
            .into_iter()
            .map(|key| {
                let route = key_route(&key_text(&key));
                (key, route)
            })
            .collect();
        let planned = plan(Some(basis), String::new(), changes, &mut lookup, key_text)?;
        planned.encode(self, measure, &mut |_| {}, key_text)
    }
    #[cfg(test)]
    pub(crate) fn replace_map_values<'a>(
        &mut self,
        reference: &Reference,
        replacements: Map<String, Value>,
        load: impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
    ) -> Result<MapUpdate> {
        for key in replacements.keys() {
            if lookup_loaded(reference, key, &load)?.is_none() {
                return Err(invalid("replacement key is absent from the published map"));
            }
        }
        self.edit_map_values(
            reference,
            replacements
                .into_iter()
                .map(|(key, value)| (key, Some(value)))
                .collect(),
            load,
        )
    }
    /// Exact insert/update/delete edits, preserving every unchanged source row.
    pub(crate) fn edit_map_values<'a>(
        &mut self,
        reference: &Reference,
        edits: BTreeMap<String, Option<Value>>,
        load: impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
    ) -> Result<MapUpdate> {
        let basis = MapRoot::read(reference, ProofMeasure::Plain, &load)?;
        let keys = edits
            .keys()
            .map(|key| (key.clone(), key_route(key)))
            .collect();
        let planned = plan(
            Some(&basis),
            String::new(),
            keys,
            &mut |key| match edits.get(key) {
                Some(value) => Ok(value.clone()),
                None => lookup_loaded(reference, key, &load),
            },
            &|key: &String| Cow::Borrowed(key.as_str()),
        )?;
        let mut reused_roots = Vec::new();
        let root = planned.encode(
            self,
            ProofMeasure::Plain,
            &mut |reference| reused_roots.push(reference.clone()),
            &|key: &String| Cow::Borrowed(key.as_str()),
        )?;
        Ok(MapUpdate { root, reused_roots })
    }
}
fn lookup_loaded<'a>(
    root: &Reference,
    key: &str,
    load: &impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
) -> Result<Option<Value>> {
    let mut reference = root.clone();
    let route = key_route(key);
    let mut depth = 0;
    loop {
        let object = load(&reference)?;
        match reference.kind {
            ReferenceKind::Page => {
                return Ok(object
                    .value()
                    .as_object()
                    .expect("invariant: admitted map leaf")
                    .get(key)
                    .cloned());
            }
            ReferenceKind::Index => {
                let index = decode_index(object)?;
                let Some(next) = index.children.get(&route[depth..depth + 1]) else {
                    return Ok(None);
                };
                reference = next.clone();
                depth += 1;
            }
        }
    }
}
#[cfg(test)]
#[path = "../../../../test/engine/unit/metadata_map/update_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/metadata_map/path_copy_tests.rs"]
mod path_copy_tests;
