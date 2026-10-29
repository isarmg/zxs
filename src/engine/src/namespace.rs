//! Bounded, immutable, per-directory paged namespace roots referenced by
//! commits. Pages hold one directory's direct children sorted by component
//! lookup key; an ordered index locates a page before it is read. A sparse
//! directory overlay and copy-on-write B+ nodes retain unaffected boundaries.
//! Node bytes are deterministic; tree shape may depend on edit history.
use crate::journal::{AuthenticatedMetadataObject, PreparedMetadataObject};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use storage_contract::{
    CommitNamespace, Entry, ErrorCode, NAMESPACE_INDEX_FANOUT, NAMESPACE_INDEX_KIND,
    NAMESPACE_PAGE_ENTRIES, NAMESPACE_PAGE_KIND, NAMESPACE_PAGE_TARGET_BYTES, NamespaceEntryKind,
    NamespaceIndex, NamespaceIndexChild, NamespacePage, NamespacePageEntry, NamespaceRefKind,
    NamespaceReference, canonical_json,
};
mod mutation_order;
use mutation_order::{MutationOrder, Position};

mod page_tree;

mod ordered_walk;

fn corrupt(reason: &'static str) -> Error {
    Error::new(ErrorCode::ObjectCorrupt, reason)
}
fn invalid(reason: &'static str) -> Error {
    Error::new(ErrorCode::StateInvalid, reason)
}

/// A decoded namespace object, shared through bounded caches.
#[derive(Clone)]
pub(crate) enum NamespaceNode {
    Page(Arc<NamespacePage>),
    Index(Arc<NamespaceIndex>),
}
impl NamespaceNode {
    pub(crate) fn decode(
        reference: &NamespaceReference,
        value: &json_codec::Value,
        canonical_bytes: usize,
    ) -> Result<Self> {
        reference.validate()?;
        Ok(match reference.kind {
            NamespaceRefKind::Page => {
                let page: NamespacePage = json_codec::from_value(value.clone())?;
                validate_page_shape(&page, canonical_bytes)?;
                Self::Page(Arc::new(page))
            }
            NamespaceRefKind::Index => {
                let index: NamespaceIndex = json_codec::from_value(value.clone())?;
                validate_index_shape(&index)?;
                Self::Index(Arc::new(index))
            }
        })
    }
}

/// Shape rules every reader enforces before using an object: sorted unique
/// component keys per page, strictly increasing first keys, bounded fanout.
fn validate_page_shape(page: &NamespacePage, canonical_bytes: usize) -> Result<()> {
    if page.entries.len() > NAMESPACE_PAGE_ENTRIES {
        return Err(corrupt("namespace page exceeds the entry bound"));
    }
    if page.entries.len() > 1 && canonical_bytes > NAMESPACE_PAGE_TARGET_BYTES {
        return Err(corrupt("namespace page exceeds the byte bound"));
    }
    let mut previous: Option<String> = None;
    for entry in &page.entries {
        let key = entry.lookup_key();
        if previous
            .as_deref()
            .is_some_and(|previous| previous >= key.as_str())
        {
            return Err(corrupt("namespace page keys are not strictly increasing"));
        }
        previous = Some(key);
    }
    Ok(())
}
fn validate_index_shape(index: &NamespaceIndex) -> Result<()> {
    if index.children.is_empty() || index.children.len() > NAMESPACE_INDEX_FANOUT {
        return Err(corrupt("namespace index fanout is out of bounds"));
    }
    let mut previous: Option<&str> = None;
    for child in &index.children {
        if child.entries == 0 {
            return Err(corrupt("namespace index references an empty child"));
        }
        if previous.is_some_and(|previous| previous >= child.first_key.as_str()) {
            return Err(corrupt("namespace index keys are not strictly increasing"));
        }
        previous = Some(&child.first_key);
    }
    Ok(())
}

/// New immutable objects produced by builds and edits in one transaction,
/// carried to publication where they install before the decision.
#[derive(Default)]
pub(crate) struct NamespaceObjects {
    objects: Vec<PreparedMetadataObject>,
    seen: BTreeSet<(&'static str, String)>,
    decoded: BTreeMap<String, NamespaceNode>,
    encoded_nodes: usize,
    encoded_bytes: usize,
}
impl NamespaceObjects {
    pub(crate) fn new() -> Self {
        Self::default()
    }
    pub(crate) fn take(&mut self) -> Vec<PreparedMetadataObject> {
        self.seen.clear();
        self.decoded.clear();
        self.encoded_nodes = 0;
        self.encoded_bytes = 0;
        std::mem::take(&mut self.objects)
    }
    #[cfg(test)]
    pub(crate) fn objects(&self) -> &[PreparedMetadataObject] {
        &self.objects
    }
    /// Actual page/index object encodings, before content-address deduplication.
    /// Kept separately from installation count for publication cost accounting.
    pub(crate) fn encoding_work(&self) -> (usize, usize) {
        (self.encoded_nodes, self.encoded_bytes)
    }
    fn page(&mut self, page: NamespacePage) -> Result<NamespaceReference> {
        let object =
            PreparedMetadataObject::new(NAMESPACE_PAGE_KIND, &page, crate::disk::MAX_METADATA)?;
        self.encoded_nodes += 1;
        self.encoded_bytes += object.bytes.len();
        let reference = NamespaceReference {
            kind: NamespaceRefKind::Page,
            hash: object.hash.clone(),
        };
        if self.seen.insert((NAMESPACE_PAGE_KIND, object.hash.clone())) {
            self.decoded
                .insert(object.hash.clone(), NamespaceNode::Page(Arc::new(page)));
            self.objects.push(object);
        }
        Ok(reference)
    }
    fn index(&mut self, index: NamespaceIndex) -> Result<NamespaceReference> {
        let object =
            PreparedMetadataObject::new(NAMESPACE_INDEX_KIND, &index, crate::disk::MAX_METADATA)?;
        self.encoded_nodes += 1;
        self.encoded_bytes += object.bytes.len();
        let reference = NamespaceReference {
            kind: NamespaceRefKind::Index,
            hash: object.hash.clone(),
        };
        if self
            .seen
            .insert((NAMESPACE_INDEX_KIND, object.hash.clone()))
        {
            self.decoded
                .insert(object.hash.clone(), NamespaceNode::Index(Arc::new(index)));
            self.objects.push(object);
        }
        Ok(reference)
    }
    fn node(&self, reference: &NamespaceReference) -> Option<NamespaceNode> {
        self.decoded.get(&reference.hash).cloned()
    }
    /// Mark one address as already durable for this transaction: sealing then
    /// skips re-publishing an identical object at that address. The content
    /// stays resolvable from wherever the editor originally read it.
    pub(crate) fn mark_durable(&mut self, address: (NamespaceRefKind, String)) {
        let (kind, hash) = address;
        self.seen.insert((kind.as_str(), hash));
    }
    /// Register one already-authenticated namespace object, decoded and
    /// shape-validated, so publication installs it like a freshly built one.
    /// Imports and re-reads use this; builds go through `page`/`index`.
    pub(crate) fn install(
        &mut self,
        reference: &NamespaceReference,
        object: crate::journal::AuthenticatedMetadataObject,
    ) -> Result<()> {
        reference.validate()?;
        let node = NamespaceNode::decode(reference, object.value(), object.canonical_len())?;
        let kind = reference.kind.as_str();
        if object.kind() != kind || object.hash() != reference.hash {
            return Err(corrupt(
                "namespace object address differs from its reference",
            ));
        }
        if self.seen.insert((kind, reference.hash.clone())) {
            self.decoded.insert(reference.hash.clone(), node);
            self.objects
                .push(object.into_prepared(crate::disk::MAX_METADATA)?);
        }
        Ok(())
    }
    /// Merge another transaction-local object set (deduplicated by address).
    pub(crate) fn absorb(&mut self, other: NamespaceObjects) {
        let NamespaceObjects {
            objects,
            seen: _,
            decoded,
            encoded_nodes,
            encoded_bytes,
        } = other;
        self.encoded_nodes += encoded_nodes;
        self.encoded_bytes += encoded_bytes;
        for object in objects {
            if self.seen.insert((object.kind, object.hash.clone())) {
                self.objects.push(object);
            }
        }
        for (hash, node) in decoded {
            self.decoded.entry(hash).or_insert(node);
        }
    }
}

/// Where readers resolve namespace objects: a transaction's fresh objects
/// first, then the caller's store (bounded cache over the published members).
pub(crate) trait NamespaceSource {
    fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode>;
}
pub(crate) struct LoaderSource<F>(pub(crate) F);
/// A source over an authenticated metadata-object loader (members, evidence
/// reads or a package directory): each node authenticates before decoding.
pub(crate) struct ObjectLoaderSource<F>(pub(crate) std::cell::RefCell<F>);
impl<F> ObjectLoaderSource<F> {
    pub(crate) fn new(load: F) -> Self {
        Self(std::cell::RefCell::new(load))
    }
}
impl<F> NamespaceSource for ObjectLoaderSource<F>
where
    F: FnMut(&'static str, &str) -> Result<crate::journal::AuthenticatedMetadataObject>,
{
    fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode> {
        reference.validate()?;
        let object = (self.0.borrow_mut())(reference.kind.as_str(), &reference.hash)?;
        NamespaceNode::decode(reference, object.value(), object.canonical_len())
    }
}
impl<F: Fn(&NamespaceReference) -> Result<NamespaceNode>> NamespaceSource for LoaderSource<F> {
    fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode> {
        (self.0)(reference)
    }
}
pub(crate) struct DraftSource<'a, S: ?Sized> {
    pub(crate) fresh: &'a NamespaceObjects,
    pub(crate) base: &'a S,
}
impl<S: NamespaceSource + ?Sized> NamespaceSource for DraftSource<'_, S> {
    fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode> {
        if let Some(node) = self.fresh.node(reference) {
            return Ok(node);
        }
        self.base.node(reference)
    }
}
/// Decoded lookup over a transaction's already-detached prepared namespace
/// objects, falling back to the published store.
pub(crate) struct PreparedSource<'a, S: ?Sized> {
    nodes: BTreeMap<&'a str, NamespaceNode>,
    base: &'a S,
}
impl<'a, S: NamespaceSource + ?Sized> PreparedSource<'a, S> {
    pub(crate) fn new(objects: &'a [PreparedMetadataObject], base: &'a S) -> Result<Self> {
        let mut nodes = BTreeMap::new();
        for object in objects {
            let kind = match object.kind {
                storage_contract::NAMESPACE_PAGE_KIND => NamespaceRefKind::Page,
                storage_contract::NAMESPACE_INDEX_KIND => NamespaceRefKind::Index,
                _ => continue,
            };
            let reference = NamespaceReference {
                kind,
                hash: object.hash.clone(),
            };
            nodes.insert(
                object.hash.as_str(),
                NamespaceNode::decode(
                    &reference,
                    &json_codec::from_slice(&object.bytes)?,
                    object.bytes.len(),
                )?,
            );
        }
        Ok(Self { nodes, base })
    }
}
impl<S: NamespaceSource + ?Sized> NamespaceSource for PreparedSource<'_, S> {
    fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode> {
        if let Some(node) = self.nodes.get(reference.hash.as_str()) {
            return Ok(node.clone());
        }
        self.base.node(reference)
    }
}

/// One directory's complete direct-child list in component-key order.
fn directory_children(
    reference: &NamespaceReference,
    source: &(impl NamespaceSource + ?Sized),
    out: &mut Vec<NamespacePageEntry>,
) -> Result<()> {
    match source.node(reference)? {
        NamespaceNode::Page(page) => out.extend(page.entries.iter().cloned()),
        NamespaceNode::Index(index) => {
            for child in &index.children {
                directory_children(&child.reference, source, out)?;
            }
        }
    }
    Ok(())
}

/// Deterministic packing: fill pages greedily by the format bounds, then
/// group pages into index levels of bounded fanout. Partition shape is a
/// pure function of the sorted child list.
fn pack_directory(
    children: &[NamespacePageEntry],
    out: &mut NamespaceObjects,
) -> Result<(NamespaceReference, u64, u64)> {
    let mut entries_total = 0u64;
    let mut files_total = 0u64;
    for child in children {
        match child {
            NamespacePageEntry::File { .. } => {
                entries_total += 1;
                files_total += 1;
            }
            NamespacePageEntry::Directory { entries, files, .. } => {
                entries_total = entries_total
                    .checked_add(1)
                    .and_then(|total| total.checked_add(*entries))
                    .ok_or_else(|| invalid("namespace entry total overflows"))?;
                files_total = files_total
                    .checked_add(*files)
                    .ok_or_else(|| invalid("namespace file total overflows"))?;
            }
        }
    }
    // Page boundaries: entry count or canonical byte target, whichever first;
    // a single oversized entry still occupies its own page.
    let mut pages: Vec<(String, u64, u64, NamespaceReference)> = Vec::new();
    let mut current: Vec<NamespacePageEntry> = Vec::new();
    let empty_page_bytes = canonical_json(&NamespacePage {
        format: Default::default(),
        entries: Vec::new(),
    })?
    .len();
    let mut current_bytes = empty_page_bytes;
    let flush = |current: &mut Vec<NamespacePageEntry>,
                 current_bytes: &mut usize,
                 out: &mut NamespaceObjects,
                 pages: &mut Vec<(String, u64, u64, NamespaceReference)>|
     -> Result<()> {
        if current.is_empty() {
            return Ok(());
        }
        let first_key = current[0].lookup_key();
        let mut entries = 0u64;
        let mut files = 0u64;
        for entry in current.iter() {
            match entry {
                NamespacePageEntry::File { .. } => {
                    entries += 1;
                    files += 1;
                }
                NamespacePageEntry::Directory {
                    entries: sub,
                    files: sub_files,
                    ..
                } => {
                    entries += 1 + sub;
                    files += sub_files;
                }
            }
        }
        let reference = out.page(NamespacePage {
            format: Default::default(),
            entries: std::mem::take(current),
        })?;
        *current_bytes = empty_page_bytes;
        pages.push((first_key, entries, files, reference));
        Ok(())
    };
    for child in children {
        let bytes = canonical_json(child)?.len();
        if !current.is_empty()
            && (current.len() == NAMESPACE_PAGE_ENTRIES
                || current_bytes + 1 + bytes > NAMESPACE_PAGE_TARGET_BYTES)
        {
            flush(&mut current, &mut current_bytes, out, &mut pages)?;
        }
        current_bytes += bytes + usize::from(!current.is_empty());
        current.push(child.clone());
    }
    flush(&mut current, &mut current_bytes, out, &mut pages)?;
    if pages.is_empty() {
        // An empty directory is one explicit empty page.
        let reference = out.page(NamespacePage {
            format: Default::default(),
            entries: Vec::new(),
        })?;
        return Ok((reference, 0, 0));
    }
    // Group bottom-up into bounded-fanout index levels.
    let mut level = pages;
    let mut depth = 0usize;
    while level.len() > 1 {
        depth += 1;
        if depth >= storage_contract::NAMESPACE_INDEX_DEPTH {
            return Err(invalid("namespace directory exceeds the index depth"));
        }
        let mut next = Vec::new();
        let mut remaining = level.as_slice();
        while !remaining.is_empty() {
            // Avoid a short non-root index at the end of a bulk-built level.
            let take = if remaining.len() > NAMESPACE_INDEX_FANOUT
                && remaining.len() < NAMESPACE_INDEX_FANOUT * 3 / 2
            {
                remaining.len() / 2
            } else {
                remaining.len().min(NAMESPACE_INDEX_FANOUT)
            };
            let (group, rest) = remaining.split_at(take);
            remaining = rest;
            let first_key = group[0].0.clone();
            let entries = group.iter().map(|(_, entries, _, _)| entries).sum();
            let files = group.iter().map(|(_, _, files, _)| files).sum();
            let reference = out.index(NamespaceIndex {
                format: Default::default(),
                children: group
                    .iter()
                    .map(
                        |(first_key, entries, files, reference)| NamespaceIndexChild {
                            first_key: first_key.clone(),
                            entries: *entries,
                            files: *files,
                            reference: reference.clone(),
                        },
                    )
                    .collect(),
            })?;
            next.push((first_key, entries, files, reference));
        }
        level = next;
    }
    let (_, _, _, reference) = level.pop().expect("invariant: one root remains");
    Ok((reference, entries_total, files_total))
}

/// Mutable sparse overlay of the directory tree: only observed child keys
/// are loaded; untouched siblings and subtrees retain their references.
enum DraftChild {
    File {
        entry_id: String,
        name: String,
        file_id: String,
        version_id: String,
        size: u64,
    },
    /// A directory not opened by this transaction; its subtree is shared.
    Sealed {
        entry_id: String,
        name: String,
        entries: u64,
        files: u64,
        subtree: NamespaceReference,
    },
    Open {
        entry_id: String,
        name: String,
        directory: DirectoryDraft,
    },
}
#[derive(Default)]
struct DirectoryDraft {
    // Only keys observed by this transaction are materialized. Tombstones
    // also cache negative point lookups, so repeating an edit does not read
    // the same route again. `load_all` is reserved for explicit subtree work.
    base: Option<NamespaceIndexChild>,
    children: BTreeMap<String, DraftChild>,
    original: BTreeMap<String, NamespacePageEntry>,
    absent: BTreeSet<String>,
    all_loaded: bool,
}
impl DraftChild {
    fn from_entry(entry: NamespacePageEntry) -> Self {
        match entry {
            NamespacePageEntry::File {
                entry_id,
                name,
                file_id,
                version_id,
                size,
            } => Self::File {
                entry_id,
                name,
                file_id,
                version_id,
                size,
            },
            NamespacePageEntry::Directory {
                entry_id,
                name,
                entries,
                files,
                subtree,
            } => Self::Sealed {
                entry_id,
                name,
                entries,
                files,
                subtree,
            },
        }
    }
    fn entries(&self) -> u64 {
        match self {
            Self::File { .. } => 1,
            Self::Sealed { entries, .. } => 1 + entries,
            Self::Open { directory, .. } => 1 + directory.entry_count(),
        }
    }
    fn seal(
        self,
        source: &(impl NamespaceSource + ?Sized),
        out: &mut NamespaceObjects,
    ) -> Result<NamespacePageEntry> {
        Ok(match self {
            Self::File {
                entry_id,
                name,
                file_id,
                version_id,
                size,
            } => NamespacePageEntry::File {
                entry_id,
                name,
                file_id,
                version_id,
                size,
            },
            Self::Sealed {
                entry_id,
                name,
                entries,
                files,
                subtree,
            } => NamespacePageEntry::Directory {
                entry_id,
                name,
                entries,
                files,
                subtree,
            },
            Self::Open {
                entry_id,
                name,
                directory,
            } => {
                let (subtree, entries, files) = directory.seal(source, out)?;
                NamespacePageEntry::Directory {
                    entry_id,
                    name,
                    entries,
                    files,
                    subtree,
                }
            }
        })
    }
}
impl DirectoryDraft {
    fn load(reference: &NamespaceReference, entries: u64, files: u64) -> Self {
        Self {
            base: Some(NamespaceIndexChild {
                first_key: String::new(),
                entries,
                files,
                reference: reference.clone(),
            }),
            ..Default::default()
        }
    }
    fn load_child(&mut self, key: &str, source: &(impl NamespaceSource + ?Sized)) -> Result<()> {
        if self.children.contains_key(key) || self.absent.contains(key) || self.all_loaded {
            return Ok(());
        }
        let entry = match &self.base {
            Some(base) => locate_child(&base.reference, key, source)?,
            None => None,
        };
        if let Some(entry) = entry {
            self.original.insert(key.to_owned(), entry.clone());
            self.children
                .insert(key.to_owned(), DraftChild::from_entry(entry));
        } else {
            self.absent.insert(key.to_owned());
        }
        Ok(())
    }
    fn child(
        &mut self,
        key: &str,
        source: &(impl NamespaceSource + ?Sized),
    ) -> Result<Option<&DraftChild>> {
        self.load_child(key, source)?;
        Ok(self.children.get(key))
    }
    fn load_all(&mut self, source: &(impl NamespaceSource + ?Sized)) -> Result<()> {
        if self.all_loaded {
            return Ok(());
        }
        if let Some(base) = &self.base {
            let mut entries = Vec::new();
            directory_children(&base.reference, source, &mut entries)?;
            for entry in entries {
                let key = entry.lookup_key();
                if self.original.contains_key(&key) || self.absent.contains(&key) {
                    continue;
                }
                self.original.insert(key.clone(), entry.clone());
                self.children.insert(key, DraftChild::from_entry(entry));
            }
        }
        self.all_loaded = true;
        Ok(())
    }
    fn remove(&mut self, key: &str) -> Option<DraftChild> {
        self.absent.insert(key.to_owned());
        self.children.remove(key)
    }
    fn entry_count(&self) -> u64 {
        let original: u64 = self
            .original
            .values()
            .map(|entry| match entry {
                NamespacePageEntry::File { .. } => 1,
                NamespacePageEntry::Directory { entries, .. } => 1 + entries,
            })
            .sum();
        self.base.as_ref().map_or(0, |base| base.entries) - original
            + self.children.values().map(DraftChild::entries).sum::<u64>()
    }
    fn open_child(
        &mut self,
        key: &str,
        source: &(impl NamespaceSource + ?Sized),
    ) -> Result<Option<&mut DirectoryDraft>> {
        self.load_child(key, source)?;
        let Some(child) = self.children.get_mut(key) else {
            return Ok(None);
        };
        if let DraftChild::Sealed {
            entry_id,
            name,
            entries,
            files,
            subtree,
        } = child
        {
            let directory = DirectoryDraft::load(subtree, *entries, *files);
            *child = DraftChild::Open {
                entry_id: std::mem::take(entry_id),
                name: std::mem::take(name),
                directory,
            };
        }
        match child {
            DraftChild::Open { directory, .. } => Ok(Some(directory)),
            DraftChild::File { .. } => Ok(None),
            DraftChild::Sealed { .. } => unreachable!("invariant: sealed child was opened"),
        }
    }
    fn seal(
        self,
        source: &(impl NamespaceSource + ?Sized),
        out: &mut NamespaceObjects,
    ) -> Result<(NamespaceReference, u64, u64)> {
        let mut tree = page_tree::PageTree::new(self.base);
        let mut original = self.original;
        for (key, child) in self.children {
            let entry = child.seal(source, out)?;
            if original.remove(&key).as_ref() != Some(&entry) {
                tree.edit(&key, Some(entry), source)?;
            }
        }
        for (key, _) in original {
            tree.edit(&key, None, source)?;
        }
        tree.seal(source, out)
    }
}

/// A path-copy editor over one base root. Edits mirror the committed
/// namespace semantics: file upserts, mkdir, delete, delete-tree, rename.
/// Records every address a transaction's editor actually read, so sealing can
/// skip re-publishing pages whose re-encoded bytes are already durable.
pub(crate) struct RecordingSource<'s, S: ?Sized> {
    inner: &'s S,
    loaded: std::cell::RefCell<BTreeSet<(NamespaceRefKind, String)>>,
}
impl<S: NamespaceSource + ?Sized> NamespaceSource for RecordingSource<'_, S> {
    fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode> {
        let node = self.inner.node(reference)?;
        self.loaded
            .borrow_mut()
            .insert((reference.kind, reference.hash.clone()));
        Ok(node)
    }
}

pub(crate) struct NamespaceEditor<'s, S: NamespaceSource + ?Sized> {
    root: DirectoryDraft,
    source: RecordingSource<'s, S>,
    order: MutationOrder,
}
impl<'s, S: NamespaceSource + ?Sized> NamespaceEditor<'s, S> {
    pub(crate) fn open(base: &CommitNamespace, source: &'s S) -> Result<Self> {
        base.root.validate()?;
        let source = RecordingSource {
            inner: source,
            loaded: Default::default(),
        };
        let root = DirectoryDraft::load(&base.root, base.entries, 0);
        Ok(Self {
            root,
            source,
            order: MutationOrder::default(),
        })
    }
    fn directory_mut(
        &mut self,
        parent_keys: &[&str],
        leaf: &str,
    ) -> Result<(&mut DirectoryDraft, String)> {
        let mut current = &mut self.root;
        let mut display = String::new();
        for key in parent_keys {
            let name = match current.child(key, &self.source)? {
                Some(DraftChild::Sealed { name, .. } | DraftChild::Open { name, .. }) => name,
                Some(DraftChild::File { .. }) | None => {
                    return Err(Error::new(
                        ErrorCode::DirectoryNotFound,
                        "namespace parent directory is absent",
                    ));
                }
            };
            if !display.is_empty() {
                display.push('/');
            }
            display.push_str(name);
            current = current
                .open_child(key, &self.source)?
                .expect("invariant: the parent child is a directory");
        }
        current.load_child(leaf, &self.source)?;
        Ok((current, display))
    }
    /// Insert or replace one file entry; parents must already exist.
    pub(crate) fn upsert_file(&mut self, entry: &Entry) -> Result<()> {
        let (parents, leaf) = split_key(entry.lookup_key());
        let (directory, parent_display) = self.directory_mut(&parents, leaf)?;
        if let Some(DraftChild::Sealed { .. } | DraftChild::Open { .. }) =
            directory.children.get(leaf)
        {
            return Err(invalid("a directory already uses this name"));
        }
        validate_composed_path(&parent_display, entry.location())?;
        directory.children.insert(
            leaf.to_owned(),
            DraftChild::File {
                entry_id: entry.entry_id.clone(),
                name: entry.name().to_owned(),
                file_id: entry.file_id.clone(),
                version_id: entry.version_id.clone(),
                size: entry.size,
            },
        );
        self.order.append(entry.lookup_key());
        Ok(())
    }
    /// Register one directory entry; parents must already exist.
    pub(crate) fn upsert_directory(&mut self, entry: &Entry) -> Result<()> {
        let (parents, leaf) = split_key(entry.lookup_key());
        let (directory, parent_display) = self.directory_mut(&parents, leaf)?;
        match directory.children.get_mut(leaf) {
            None => {
                validate_composed_path(&parent_display, entry.location())?;
                directory.children.insert(
                    leaf.to_owned(),
                    DraftChild::Open {
                        entry_id: entry.entry_id.clone(),
                        name: entry.name().to_owned(),
                        directory: DirectoryDraft::default(),
                    },
                );
                self.order.append(entry.lookup_key());
                Ok(())
            }
            Some(DraftChild::File { .. }) => Err(invalid("a file already uses this name")),
            // Re-registering an existing directory keeps its subtree.
            Some(DraftChild::Sealed { .. } | DraftChild::Open { .. }) => Ok(()),
        }
    }
    /// Remove one entry; a directory must be empty unless `tree` is set.
    pub(crate) fn delete(&mut self, key: &str, tree: bool) -> Result<()> {
        let (parents, leaf) = split_key(key);
        let (directory, _) = self.directory_mut(&parents, leaf)?;
        match directory.children.get_mut(leaf) {
            None => Err(Error::new(
                ErrorCode::FileNotFound,
                "namespace entry is absent",
            )),
            Some(DraftChild::File { .. }) => {
                directory.remove(leaf);
                self.order.remove(key);
                Ok(())
            }
            Some(child) => {
                if !tree {
                    let empty = match child {
                        DraftChild::Sealed { entries, .. } => *entries == 0,
                        DraftChild::Open { directory, .. } => directory.entry_count() == 0,
                        DraftChild::File { .. } => unreachable!("invariant: matched above"),
                    };
                    if !empty {
                        return Err(Error::new(
                            ErrorCode::DirectoryNotEmpty,
                            "directory still has entries",
                        ));
                    }
                }
                directory.remove(leaf);
                self.order.remove(key);
                Ok(())
            }
        }
    }
    /// Move one entry to a new absolute location, keeping its identity and —
    /// for a directory — its entire subtree reference unchanged.
    pub(crate) fn rename(&mut self, source_key: &str, destination: &Entry) -> Result<()> {
        let (parents, leaf) = split_key(source_key);
        let (directory, _) = self.directory_mut(&parents, leaf)?;
        if !directory.children.contains_key(leaf) {
            return Err(Error::new(
                ErrorCode::FileNotFound,
                "namespace source entry is absent",
            ));
        }
        let (target_parents, target_leaf) = split_key(destination.lookup_key());
        let (target, parent_display) = self.directory_mut(&target_parents, target_leaf)?;
        if source_key != destination.lookup_key() && target.children.contains_key(target_leaf) {
            return Err(invalid("namespace destination is already used"));
        }
        validate_composed_path(&parent_display, destination.location())?;
        let (directory, _) = self.directory_mut(&parents, leaf)?;
        let child = directory
            .remove(leaf)
            .expect("invariant: the validated rename source is present");
        let renamed = match child {
            DraftChild::File {
                entry_id,
                file_id,
                version_id,
                size,
                ..
            } => DraftChild::File {
                entry_id,
                name: destination.name().to_owned(),
                file_id,
                version_id,
                size,
            },
            DraftChild::Sealed {
                entry_id,
                entries,
                files,
                subtree,
                ..
            } => DraftChild::Sealed {
                entry_id,
                name: destination.name().to_owned(),
                entries,
                files,
                subtree,
            },
            DraftChild::Open {
                entry_id,
                directory,
                ..
            } => DraftChild::Open {
                entry_id,
                name: destination.name().to_owned(),
                directory,
            },
        };
        let (target, _) = self.directory_mut(&target_parents, target_leaf)?;
        target.children.insert(target_leaf.to_owned(), renamed);
        self.order.rename(source_key, destination.lookup_key());
        Ok(())
    }
    pub(crate) fn seal(self, out: &mut NamespaceObjects) -> Result<CommitNamespace> {
        // Read-only and untouched nodes retain their references without encoding.
        // Recreated bytes may also already be durable after a reverted edit.
        for address in self.source.loaded.borrow().iter().cloned() {
            out.mark_durable(address);
        }
        let (root, entries, _) = self.root.seal(&self.source, out)?;
        let (encoded_nodes, encoded_bytes) = out.encoding_work();
        tracing::trace!(
            encoded_nodes,
            encoded_bytes,
            "namespace encoding work before deduplication"
        );
        Ok(CommitNamespace { root, entries })
    }
    /// Kind of the entry at a normalized full-path key: None when it is
    /// absent, or when an intermediate component is absent or a file.
    pub(crate) fn entry_kind(
        &mut self,
        key: &str,
    ) -> Result<Option<storage_contract::NamespaceEntryKind>> {
        let (parents, leaf) = split_key(key);
        let mut current = &mut self.root;
        for component in parents {
            match current.open_child(component, &self.source)? {
                Some(next) => current = next,
                None => return Ok(None),
            }
        }
        Ok(current.child(leaf, &self.source)?.map(|child| match child {
            DraftChild::File { .. } => storage_contract::NamespaceEntryKind::File,
            DraftChild::Sealed { .. } | DraftChild::Open { .. } => {
                storage_contract::NamespaceEntryKind::Directory
            }
        }))
    }
    /// Create any absent ancestors of `path` as fresh directory entries; an
    /// existing non-directory ancestor fails like the flat-list editor did.
    pub(crate) fn ensure_parents(&mut self, path: &name_policy::PortablePath) -> Result<()> {
        let mut parents = Vec::new();
        let mut parent = path.parent();
        while let Some(ancestor) = parent {
            parent = ancestor.parent();
            parents.push(ancestor);
        }
        for ancestor in parents.into_iter().rev() {
            match self.entry_kind(ancestor.key())? {
                Some(storage_contract::NamespaceEntryKind::Directory) => {}
                Some(storage_contract::NamespaceEntryKind::File) => {
                    return Err(Error::new(
                        ErrorCode::ParentNotDirectory,
                        "parent is not a directory",
                    ));
                }
                None => self.upsert_directory(&Entry {
                    entry_id: storage_contract::new_id("entry"),
                    path: ancestor,
                    kind: storage_contract::NamespaceEntryKind::Directory,
                    file_id: String::new(),
                    version_id: String::new(),
                    size: 0,
                })?,
            }
        }
        Ok(())
    }
    /// Preserve the legacy mutation buffer's deletion order, including moves
    /// and entries appended earlier in this transaction. The target need not
    /// precede descendants moved into a newly created ancestor.
    fn subtree_deletions(&mut self, key: &str) -> Result<Vec<storage_contract::DeletedPath>> {
        fn collect(
            draft: &mut DirectoryDraft,
            prefix: &str,
            source: &(impl NamespaceSource + ?Sized),
            order: &MutationOrder,
            out: &mut Vec<(Position, storage_contract::DeletedPath)>,
        ) -> Result<()> {
            draft.load_all(source)?;
            let keys: Vec<String> = draft.children.keys().cloned().collect();
            for child_key in keys {
                let (name, file) = match draft.children.get(&child_key) {
                    Some(DraftChild::File { name, .. }) => (name.clone(), true),
                    Some(DraftChild::Sealed { name, .. } | DraftChild::Open { name, .. }) => {
                        (name.clone(), false)
                    }
                    None => continue,
                };
                let display = format!("{prefix}/{name}");
                let full_key = name_policy::path_key(&display);
                out.push((
                    order.position(&full_key),
                    storage_contract::DeletedPath {
                        path: display.clone(),
                        kind: if file {
                            storage_contract::NamespaceEntryKind::File
                        } else {
                            storage_contract::NamespaceEntryKind::Directory
                        },
                    },
                ));
                if !file {
                    let next = draft
                        .open_child(&child_key, source)?
                        .expect("invariant: a directory child opens");
                    collect(next, &display, source, order, out)?;
                }
            }
            Ok(())
        }
        let source = &self.source;
        let (parents, leaf) = split_key(key);
        let mut current = &mut self.root;
        let mut prefix = String::new();
        for component in parents {
            let name = match current.child(component, source)? {
                Some(
                    DraftChild::File { name, .. }
                    | DraftChild::Sealed { name, .. }
                    | DraftChild::Open { name, .. },
                ) => name.clone(),
                None => return Ok(Vec::new()),
            };
            prefix = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            match current.open_child(component, source)? {
                Some(next) => current = next,
                None => return Ok(Vec::new()),
            }
        }
        current.load_child(leaf, source)?;
        let Some(child) = current.children.get_mut(leaf) else {
            return Ok(Vec::new());
        };
        let (name, file) = match child {
            DraftChild::File { name, .. } => (name.clone(), true),
            DraftChild::Sealed { name, .. } | DraftChild::Open { name, .. } => {
                (name.clone(), false)
            }
        };
        let display = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let mut out = Vec::new();
        if file {
            out.push(storage_contract::DeletedPath {
                path: display,
                kind: storage_contract::NamespaceEntryKind::File,
            });
            return Ok(out);
        }
        let mut descendants = vec![(
            self.order.position(key),
            storage_contract::DeletedPath {
                path: display.clone(),
                kind: storage_contract::NamespaceEntryKind::Directory,
            },
        )];
        if let DraftChild::Sealed {
            entry_id,
            name,
            subtree,
            entries,
            files,
        } = child
        {
            let directory = DirectoryDraft::load(subtree, *entries, *files);
            *child = DraftChild::Open {
                entry_id: std::mem::take(entry_id),
                name: std::mem::take(name),
                directory,
            };
        }
        if let DraftChild::Open { directory, .. } = child {
            collect(directory, &display, source, &self.order, &mut descendants)?;
        }
        descendants.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        out.extend(descendants.into_iter().map(|(_, deleted)| deleted));
        Ok(out)
    }
    /// Apply one committed namespace operation with the flat-list editor's
    /// exact validation order, errors and recorded deletion facts.
    pub(crate) fn apply(
        &mut self,
        op: &storage_contract::NamespaceOperation,
    ) -> Result<Vec<storage_contract::DeletedPath>> {
        use storage_contract::{NamespaceOperation, NamespaceOperationKind};
        let key = op.key();
        let mut deleted = Vec::new();
        match op {
            NamespaceOperation::Mkdir { path } => {
                if self.entry_kind(key)?.is_some() {
                    return Err(Error::new(
                        ErrorCode::NameCollision,
                        "directory target exists",
                    ));
                }
                self.ensure_parents(path)?;
                self.upsert_directory(&Entry {
                    entry_id: storage_contract::new_id("entry"),
                    path: path.clone(),
                    kind: storage_contract::NamespaceEntryKind::Directory,
                    file_id: String::new(),
                    version_id: String::new(),
                    size: 0,
                })?;
            }
            NamespaceOperation::Delete { .. } | NamespaceOperation::DeleteTree { .. } => {
                let Some(kind) = self.entry_kind(key)? else {
                    return Err(Error::new(
                        ErrorCode::EntryNotFound,
                        "delete target missing",
                    ));
                };
                if op.kind() == NamespaceOperationKind::Delete
                    && kind == storage_contract::NamespaceEntryKind::Directory
                    && !self.directory_is_empty(key)?
                {
                    return Err(Error::new(
                        ErrorCode::DirectoryNotEmpty,
                        "directory contains children",
                    ));
                }
                deleted = self.subtree_deletions(key)?;
                self.delete(key, op.kind() == NamespaceOperationKind::DeleteTree)?;
            }
            NamespaceOperation::Rename {
                destination: target,
                ..
            } => {
                let to_key = target.key();
                let Some(kind) = self.entry_kind(key)? else {
                    return Err(Error::new(
                        ErrorCode::EntryNotFound,
                        "rename source missing",
                    ));
                };
                if is_descendant_key(to_key, key) {
                    return Err(Error::new(
                        ErrorCode::InvalidMove,
                        "cannot move a directory into itself",
                    ));
                }
                if key != to_key && self.entry_kind(to_key)?.is_some() {
                    return Err(Error::new(
                        ErrorCode::NameCollision,
                        "rename destination exists",
                    ));
                }
                let old_prefix = if kind == NamespaceEntryKind::Directory {
                    Some(self.existing_display_path(key)?)
                } else {
                    None
                };
                self.ensure_parents(target)?;
                self.rename(
                    key,
                    &Entry {
                        entry_id: String::new(),
                        path: target.clone(),
                        kind,
                        file_id: String::new(),
                        version_id: String::new(),
                        size: 0,
                    },
                )?;
                if let Some(old_prefix) = old_prefix {
                    let new_prefix = self.existing_display_path(to_key)?;
                    // Validated component names stay valid. Only aggregate
                    // budgets can change under rebasing; a non-growing actual
                    // display prefix preserves every admitted descendant's
                    // budget without reading it. Request spelling is not this
                    // proof (case folding can change component length).
                    if new_prefix.len() > old_prefix.len()
                        || new_prefix.encode_utf16().count() > old_prefix.encode_utf16().count()
                        || new_prefix.split('/').count() > old_prefix.split('/').count()
                    {
                        self.validate_subtree_paths(to_key)?;
                    }
                }
            }
        }
        Ok(deleted)
    }
    /// Resolve a key already proven present in this editor, retaining the
    /// stored display spelling of its ancestors rather than the caller's.
    fn existing_display_path(&mut self, key: &str) -> Result<String> {
        let mut current = &mut self.root;
        let mut components = key.split('/').peekable();
        let mut display = String::new();
        while let Some(component) = components.next() {
            let child = current
                .child(component, &self.source)?
                .expect("invariant: the resolved key is present");
            let name = match child {
                DraftChild::File { name, .. }
                | DraftChild::Sealed { name, .. }
                | DraftChild::Open { name, .. } => name,
            };
            if !display.is_empty() {
                display.push('/');
            }
            display.push_str(name);
            if components.peek().is_some() {
                current = current
                    .open_child(component, &self.source)?
                    .expect("invariant: the resolved key's ancestors are directories");
            }
        }
        Ok(display)
    }
    fn directory_is_empty(&mut self, key: &str) -> Result<bool> {
        let (parents, leaf) = split_key(key);
        let mut current = &mut self.root;
        for component in parents {
            match current.open_child(component, &self.source)? {
                Some(next) => current = next,
                None => return Ok(true),
            }
        }
        Ok(match current.child(leaf, &self.source)? {
            Some(DraftChild::Sealed { entries, .. }) => *entries == 0,
            Some(DraftChild::Open { directory, .. }) => directory.entry_count() == 0,
            _ => true,
        })
    }
    /// Re-check the portable path budget of every display path under `key`
    /// (read-only traversal: shared subtree references stay shared).
    fn validate_subtree_paths(&mut self, key: &str) -> Result<()> {
        fn check(
            draft: &mut DirectoryDraft,
            prefix: &str,
            source: &(impl NamespaceSource + ?Sized),
        ) -> Result<()> {
            draft.load_all(source)?;
            let keys: Vec<String> = draft.children.keys().cloned().collect();
            for child_key in keys {
                let (name, file) = match draft.children.get(&child_key) {
                    Some(DraftChild::File { name, .. }) => (name.clone(), true),
                    Some(DraftChild::Sealed { name, .. } | DraftChild::Open { name, .. }) => {
                        (name.clone(), false)
                    }
                    None => continue,
                };
                let display = format!("{prefix}/{name}");
                name_policy::PortablePath::parse(&display)?;
                if !file {
                    let next = draft
                        .open_child(&child_key, source)?
                        .expect("invariant: a directory child opens");
                    check(next, &display, source)?;
                }
            }
            Ok(())
        }
        let source = &self.source;
        let (parents, leaf) = split_key(key);
        let mut current = &mut self.root;
        let mut prefix = String::new();
        for component in parents {
            let name = match current.child(component, source)? {
                Some(
                    DraftChild::File { name, .. }
                    | DraftChild::Sealed { name, .. }
                    | DraftChild::Open { name, .. },
                ) => name.clone(),
                None => return Ok(()),
            };
            prefix = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            match current.open_child(component, source)? {
                Some(next) => current = next,
                None => return Ok(()),
            }
        }
        let Some(child) = current.child(leaf, source)? else {
            return Ok(());
        };
        let name = match child {
            DraftChild::File { name, .. }
            | DraftChild::Sealed { name, .. }
            | DraftChild::Open { name, .. } => name.clone(),
        };
        let display = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        name_policy::PortablePath::parse(&display)?;
        if let Some(next) = current.open_child(leaf, source)? {
            check(next, &display, source)?;
        }
        Ok(())
    }
}

/// Composing a typed leaf with stored parent spelling is a new path boundary:
/// case folding can make the parent longer than the request's spelling. The
/// unchanged request path already carries the complete PortablePath proof.
fn validate_composed_path(parent: &str, requested: &name_policy::PortablePath) -> Result<()> {
    let requested_parent = requested
        .as_str()
        .rsplit_once('/')
        .map_or("", |(parent, _)| parent);
    if parent != requested_parent {
        let actual = format!("{parent}/{}", requested.file_name());
        name_policy::PortablePath::parse(&actual)?;
    }
    Ok(())
}

/// Compare already-normalized keys at a complete path-component boundary.
fn is_descendant_key(key: &str, ancestor: &str) -> bool {
    key.strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn split_key(key: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = key.split('/').collect();
    let leaf = parts.pop().unwrap_or(key);
    (parts, leaf)
}

/// Full build from a flat entry list with parents present and unique keys,
/// e.g. a legacy-shaped draft. The input is re-ordered component-wise here, so
/// every directory's subtree is contiguous even when full-path byte order
/// interleaves siblings between a directory and its descendants ("a", "a.b",
/// "a/x"); callers may pass any order.
pub(crate) fn build_from_entries(
    entries: &[Entry],
    out: &mut NamespaceObjects,
) -> Result<CommitNamespace> {
    struct Pending {
        entry_id: String,
        name: String,
        children: Vec<NamespacePageEntry>,
    }
    fn close(stack: &mut Vec<Pending>, out: &mut NamespaceObjects) -> Result<()> {
        let closed = stack.pop().expect("invariant: close keeps the root frame");
        let (subtree, entries, files) = pack_directory(&closed.children, out)?;
        stack
            .last_mut()
            .expect("invariant: the root frame remains")
            .children
            .push(NamespacePageEntry::Directory {
                entry_id: closed.entry_id,
                name: closed.name,
                entries,
                files,
                subtree,
            });
        Ok(())
    }
    let mut stack = vec![Pending {
        entry_id: String::new(),
        name: String::new(),
        children: Vec::new(),
    }];
    let mut path_stack: Vec<String> = Vec::new();
    // Component-wise order groups each directory with its complete subtree,
    // which full-path byte order does not guarantee.
    let mut ordered: Vec<&Entry> = entries.iter().collect();
    ordered.sort_by(|a, b| a.lookup_key().split('/').cmp(b.lookup_key().split('/')));
    for entry in ordered {
        let key = entry.lookup_key();
        while !path_stack.is_empty() {
            let prefix = path_stack.join("/");
            if key.len() > prefix.len() + 1
                && key.starts_with(&prefix)
                && key.as_bytes()[prefix.len()] == b'/'
            {
                break;
            }
            close(&mut stack, out)?;
            path_stack.pop();
        }
        // In component-wise order a directory's descendants immediately
        // follow it, so closing on prefix mismatch is exact.
        let parent = path_stack.join("/");
        let leaf = key.rsplit('/').next().unwrap_or(key);
        let expected = if parent.is_empty() {
            leaf.to_owned()
        } else {
            format!("{parent}/{leaf}")
        };
        if expected != key {
            return Err(invalid("entries are not in canonical namespace form"));
        }
        let parent_display = stack
            .iter()
            .skip(1)
            .map(|frame| frame.name.as_str())
            .collect::<Vec<_>>()
            .join("/");
        validate_composed_path(&parent_display, entry.location())?;
        match entry.kind {
            NamespaceEntryKind::Directory => {
                stack.push(Pending {
                    entry_id: entry.entry_id.clone(),
                    name: entry.name().to_owned(),
                    children: Vec::new(),
                });
                path_stack.push(
                    key.rsplit('/')
                        .next()
                        .expect("invariant: a key has a final component")
                        .to_owned(),
                );
            }
            NamespaceEntryKind::File => {
                stack
                    .last_mut()
                    .expect("invariant: the root frame remains")
                    .children
                    .push(NamespacePageEntry::File {
                        entry_id: entry.entry_id.clone(),
                        name: entry.name().to_owned(),
                        file_id: entry.file_id.clone(),
                        version_id: entry.version_id.clone(),
                        size: entry.size,
                    });
            }
        }
    }
    while stack.len() > 1 {
        close(&mut stack, out)?;
        path_stack.pop();
    }
    let root = stack.pop().expect("invariant: the root frame remains");
    // Component-wise input order already emits every level's children in
    // component-key order, matching the editor's sealed page order exactly.
    let (reference, entries, _) = pack_directory(&root.children, out)?;
    Ok(CommitNamespace {
        root: reference,
        entries,
    })
}

/// Read-only queries over one published namespace root.
pub(crate) struct NamespaceView<'s, S: NamespaceSource + ?Sized> {
    root: CommitNamespace,
    source: &'s S,
}
/// One resolved entry with its reconstructed absolute location.
pub(crate) fn materialized_entry(
    location: name_policy::PortablePath,
    entry: &NamespacePageEntry,
) -> Entry {
    match entry {
        NamespacePageEntry::File {
            entry_id,
            file_id,
            version_id,
            size,
            ..
        } => Entry {
            entry_id: entry_id.clone(),
            path: location,
            kind: NamespaceEntryKind::File,
            file_id: file_id.clone(),
            version_id: version_id.clone(),
            size: *size,
        },
        NamespacePageEntry::Directory { entry_id, .. } => Entry {
            entry_id: entry_id.clone(),
            path: location,
            kind: NamespaceEntryKind::Directory,
            file_id: String::new(),
            version_id: String::new(),
            size: 0,
        },
    }
}
impl<'s, S: NamespaceSource + ?Sized> NamespaceView<'s, S> {
    pub(crate) fn new(root: &CommitNamespace, source: &'s S) -> Self {
        Self {
            root: root.clone(),
            source,
        }
    }
    /// Resolve one normalized full-path key by descending components; index
    /// levels binary search their first keys before one page is read.
    pub(crate) fn find_key(&self, key: &str) -> Result<Option<NamespacePageEntry>> {
        if key.is_empty() {
            return Ok(None);
        }
        let mut reference = self.root.root.clone();
        let mut components = key.split('/').peekable();
        while let Some(component) = components.next() {
            let Some(entry) = locate_child(&reference, component, self.source)? else {
                return Ok(None);
            };
            if components.peek().is_none() {
                return Ok(Some(entry));
            }
            match entry {
                NamespacePageEntry::Directory { subtree, .. } => reference = subtree,
                NamespacePageEntry::File { .. } => return Ok(None),
            }
        }
        Ok(None)
    }
    /// Resolve one normalized full-path key to a materialized entry carrying
    /// its stored display path, reconstructed from the components the descent
    /// actually visited.
    pub(crate) fn resolve_key(&self, key: &str) -> Result<Option<Entry>> {
        Ok(self
            .resolve_key_raw(key)?
            .map(|(location, entry)| materialized_entry(location, &entry)))
    }
    fn resolve_key_raw(
        &self,
        key: &str,
    ) -> Result<Option<(name_policy::PortablePath, NamespacePageEntry)>> {
        if key.is_empty() {
            return Ok(None);
        }
        let mut reference = self.root.root.clone();
        let mut display = String::new();
        let mut components = key.split('/').peekable();
        while let Some(component) = components.next() {
            let Some(entry) = locate_child(&reference, component, self.source)? else {
                return Ok(None);
            };
            if display.is_empty() {
                display = entry.name().to_owned();
            } else {
                display = format!("{display}/{}", entry.name());
            }
            if components.peek().is_none() {
                let location = name_policy::PortablePath::parse(&display)
                    .map_err(|_| corrupt("namespace path exceeds the portable path budget"))?;
                return Ok(Some((location, entry)));
            }
            match entry {
                NamespacePageEntry::Directory { subtree, .. } => reference = subtree,
                NamespacePageEntry::File { .. } => return Ok(None),
            }
        }
        Ok(None)
    }
    /// The ancestor directory entries and the final entry along one key
    /// path, materialized in order; empty when the path does not resolve.
    /// The materialized entries along `key`'s existing prefix: every ancestor
    /// (and the entry at `key` itself) that resolves, stopping at the first
    /// absent or non-directory component. Unlike `path_closure`, a missing
    /// leaf still yields its existing ancestors: new-path admission checks
    /// fence on the directories a write would land under.
    pub(crate) fn ancestor_closure(&self, key: &str) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        if key.is_empty() {
            return Ok(out);
        }
        let mut reference = self.root.root.clone();
        let mut display = String::new();
        for component in key.split('/') {
            let Some(entry) = locate_child(&reference, component, self.source)? else {
                return Ok(out);
            };
            if display.is_empty() {
                display = entry.name().to_owned();
            } else {
                display = format!("{display}/{}", entry.name());
            }
            let location = name_policy::PortablePath::parse(&display)
                .map_err(|_| corrupt("namespace path exceeds the portable path budget"))?;
            out.push(materialized_entry(location, &entry));
            match entry {
                NamespacePageEntry::Directory { subtree, .. } => reference = subtree,
                NamespacePageEntry::File { .. } => return Ok(out),
            }
        }
        Ok(out)
    }
    pub(crate) fn path_closure(&self, key: &str) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        if key.is_empty() {
            return Ok(out);
        }
        let mut reference = self.root.root.clone();
        let mut display = String::new();
        let mut components = key.split('/').peekable();
        while let Some(component) = components.next() {
            let Some(entry) = locate_child(&reference, component, self.source)? else {
                return Ok(Vec::new());
            };
            if display.is_empty() {
                display = entry.name().to_owned();
            } else {
                display = format!("{display}/{}", entry.name());
            }
            let location = name_policy::PortablePath::parse(&display)
                .map_err(|_| corrupt("namespace path exceeds the portable path budget"))?;
            out.push(materialized_entry(location, &entry));
            if components.peek().is_none() {
                return Ok(out);
            }
            match entry {
                NamespacePageEntry::Directory { subtree, .. } => reference = subtree,
                NamespacePageEntry::File { .. } => return Ok(Vec::new()),
            }
        }
        Ok(out)
    }
    /// The first file entry carrying `file_id`, materialized; a linear walk
    /// with an early stop, for namespaces without a path hint.
    pub(crate) fn find_file(&self, file_id: &str) -> Result<Option<Entry>> {
        let mut found = None;
        self.walk(|location, entry| {
            if let NamespacePageEntry::File { file_id: id, .. } = entry
                && id == file_id
            {
                found = Some(materialized_entry(location.clone(), entry));
                return Ok(false);
            }
            Ok(true)
        })?;
        Ok(found)
    }
    /// Canonical full-path traversal restricted to the subtree at `key` (the
    /// entry itself first), with reconstructed absolute paths.
    pub(crate) fn walk_subtree(
        &self,
        key: &str,
        mut visit: impl FnMut(&name_policy::PortablePath, &Entry) -> Result<bool>,
    ) -> Result<()> {
        let Some((location, entry)) = self.resolve_key_raw(key)? else {
            return Ok(());
        };
        let materialized = materialized_entry(location.clone(), &entry);
        if !visit(&location, &materialized)? {
            return Ok(());
        }
        if let NamespacePageEntry::Directory { subtree, .. } = &entry {
            ordered_walk::walk(subtree, Some(&location), self.source, |location, entry| {
                visit(location, &materialized_entry(location.clone(), entry))
            })?;
        }
        Ok(())
    }
    /// The directory at `key` plus every descendant directory entry,
    /// materialized with absolute display paths. O(subtree directories).
    pub(crate) fn subtree_directories(&self, key: &str) -> Result<Vec<Entry>> {
        fn descend<S: NamespaceSource + ?Sized>(
            reference: &NamespaceReference,
            prefix: &str,
            source: &S,
            out: &mut Vec<Entry>,
        ) -> Result<()> {
            match source.node(reference)? {
                NamespaceNode::Page(page) => {
                    for entry in &page.entries {
                        if let NamespacePageEntry::Directory { subtree, .. } = entry {
                            let display = format!("{prefix}/{}", entry.name());
                            let location =
                                name_policy::PortablePath::parse(&display).map_err(|_| {
                                    corrupt("namespace path exceeds the portable path budget")
                                })?;
                            out.push(materialized_entry(location, entry));
                            descend(subtree, &display, source, out)?;
                        }
                    }
                }
                NamespaceNode::Index(index) => {
                    for child in &index.children {
                        descend(&child.reference, prefix, source, out)?;
                    }
                }
            }
            Ok(())
        }
        let mut out = Vec::new();
        let Some((location, entry)) = self.resolve_key_raw(key)? else {
            return Ok(out);
        };
        let NamespacePageEntry::Directory { subtree, .. } = &entry else {
            return Ok(out);
        };
        let prefix = location.as_str().to_owned();
        out.push(materialized_entry(location, &entry));
        descend(subtree, &prefix, self.source, &mut out)?;
        Ok(out)
    }
    /// The direct children of one directory ("" is the root), strictly after
    /// `after` in component-key order, located through the index first.
    /// One bounded page of a directory's direct children, materialized with
    /// their absolute display paths. `after` is the last seen component key.
    /// None when the key does not name a directory.
    pub(crate) fn directory_page_entries(
        &self,
        directory_key: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Option<Vec<Entry>>> {
        let display = if directory_key.is_empty() {
            String::new()
        } else {
            match self.resolve_key(directory_key)? {
                Some(entry) if entry.kind == NamespaceEntryKind::Directory => {
                    entry.path().to_owned()
                }
                _ => return Ok(None),
            }
        };
        let Some(children) = self.list_directory_page(directory_key, after, limit)? else {
            return Ok(None);
        };
        let mut out = Vec::with_capacity(children.len());
        for child in &children {
            let path = if display.is_empty() {
                child.name().to_owned()
            } else {
                format!("{display}/{}", child.name())
            };
            let location = name_policy::PortablePath::parse(&path)
                .map_err(|_| corrupt("namespace path exceeds the portable path budget"))?;
            out.push(materialized_entry(location, child));
        }
        Ok(Some(out))
    }
    /// Recorded recursive totals (entries, files) of one directory; the root's
    /// totals sum its own level. None when the key does not name a directory.
    pub(crate) fn directory_totals(&self, directory_key: &str) -> Result<Option<(u64, u64)>> {
        if directory_key.is_empty() {
            let mut entries = 0u64;
            let mut files = 0u64;
            fn level(
                reference: &NamespaceReference,
                source: &(impl NamespaceSource + ?Sized),
                entries: &mut u64,
                files: &mut u64,
            ) -> Result<()> {
                match source.node(reference)? {
                    NamespaceNode::Page(page) => {
                        for entry in &page.entries {
                            match entry {
                                NamespacePageEntry::File { .. } => {
                                    *entries += 1;
                                    *files += 1;
                                }
                                NamespacePageEntry::Directory {
                                    entries: sub,
                                    files: sub_files,
                                    ..
                                } => {
                                    *entries += 1 + sub;
                                    *files += sub_files;
                                }
                            }
                        }
                    }
                    NamespaceNode::Index(index) => {
                        for child in &index.children {
                            *entries += child.entries;
                            *files += child.files;
                        }
                    }
                }
                Ok(())
            }
            level(&self.root.root, self.source, &mut entries, &mut files)?;
            return Ok(Some((entries, files)));
        }
        Ok(match self.find_key(directory_key)? {
            Some(NamespacePageEntry::Directory { entries, files, .. }) => Some((entries, files)),
            Some(NamespacePageEntry::File { .. }) | None => None,
        })
    }
    pub(crate) fn list_directory_page(
        &self,
        directory_key: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Option<Vec<NamespacePageEntry>>> {
        let reference = if directory_key.is_empty() {
            self.root.root.clone()
        } else {
            match self.find_key(directory_key)? {
                Some(NamespacePageEntry::Directory { subtree, .. }) => subtree,
                Some(NamespacePageEntry::File { .. }) | None => return Ok(None),
            }
        };
        let mut out = Vec::new();
        collect_page(&reference, after, limit, self.source, &mut out)?;
        Ok(Some(out))
    }
    /// Traverse in canonical full-path lookup-key order, merging lazy bounded
    /// page/index cursors. The callback receives reconstructed absolute paths
    /// and can stop before later subtrees or index children are read.
    pub(crate) fn walk(
        &self,
        visit: impl FnMut(&name_policy::PortablePath, &NamespacePageEntry) -> Result<bool>,
    ) -> Result<()> {
        ordered_walk::walk(&self.root.root, None, self.source, visit)
    }
    /// Materialize the complete flat entry list in the legacy canonical
    /// order (full-path lookup-key bytes). Retains O(total) entries; traversal
    /// merges the bounded frontier rather than sorting the complete result.
    pub(crate) fn collect_entries(&self) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        self.walk(|location, entry| {
            entries.push(materialized_entry(location.clone(), entry));
            Ok(true)
        })?;
        Ok(entries)
    }
}
fn locate_child(
    reference: &NamespaceReference,
    component_key: &str,
    source: &(impl NamespaceSource + ?Sized),
) -> Result<Option<NamespacePageEntry>> {
    let mut reference = reference.clone();
    loop {
        match source.node(&reference)? {
            NamespaceNode::Page(page) => {
                return Ok(page
                    .entries
                    .binary_search_by(|entry| entry.lookup_key().as_str().cmp(component_key))
                    .ok()
                    .map(|found| page.entries[found].clone()));
            }
            NamespaceNode::Index(index) => {
                let position = index
                    .children
                    .partition_point(|child| child.first_key.as_str() <= component_key);
                let Some(child) = position.checked_sub(1).map(|p| &index.children[p]) else {
                    return Ok(None);
                };
                reference = child.reference.clone();
            }
        }
    }
}
fn collect_page(
    reference: &NamespaceReference,
    after: Option<&str>,
    limit: usize,
    source: &(impl NamespaceSource + ?Sized),
    out: &mut Vec<NamespacePageEntry>,
) -> Result<()> {
    if out.len() >= limit {
        return Ok(());
    }
    match source.node(reference)? {
        NamespaceNode::Page(page) => {
            let start = match after {
                Some(after) => page
                    .entries
                    .partition_point(|entry| entry.lookup_key().as_str() <= after),
                None => 0,
            };
            out.extend(
                page.entries[start..]
                    .iter()
                    .take(limit - out.len())
                    .cloned(),
            );
        }
        NamespaceNode::Index(index) => {
            // Seek: skip every child whose whole range precedes the cursor.
            let start = match after {
                Some(after) => index
                    .children
                    .partition_point(|child| child.first_key.as_str() <= after)
                    .saturating_sub(1),
                None => 0,
            };
            for child in &index.children[start..] {
                if out.len() >= limit {
                    break;
                }
                collect_page(&child.reference, after, limit, source, out)?;
            }
        }
    }
    Ok(())
}

/// Full admission of one commit's namespace graph: every object read and
/// verified, recorded totals re-summed, per-directory order and uniqueness,
/// index boundaries and balance, the portable path budget, and the commit's recorded
/// entry total. Returns the verified total entry count.
pub(crate) fn verify_namespace(
    root: &CommitNamespace,
    source: &(impl NamespaceSource + ?Sized),
    max_entries: u64,
) -> Result<u64> {
    #[derive(Default)]
    struct Summary {
        first_key: Option<String>,
        entries: u64,
        files: u64,
        index_height: usize,
    }
    struct Verification<'s, S: ?Sized> {
        source: &'s S,
        remaining: u64,
    }
    impl<S: NamespaceSource + ?Sized> Verification<'_, S> {
        fn directory(&mut self, reference: &NamespaceReference, prefix: &str) -> Result<Summary> {
            self.level(reference, prefix, &mut None, 0)
        }

        fn page(
            &mut self,
            page: &NamespacePage,
            prefix: &str,
            previous: &mut Option<String>,
        ) -> Result<Summary> {
            let mut summary = Summary::default();
            for child in &page.entries {
                // Charge the entry before allocating its reconstructed path or
                // descending into its subtree. Never collect a whole directory.
                if self.remaining == 0 {
                    return Err(invalid("namespace exceeds the entry limit"));
                }
                self.remaining -= 1;
                let key = child.lookup_key();
                if previous
                    .as_deref()
                    .is_some_and(|previous| previous >= key.as_str())
                {
                    return Err(corrupt(
                        "namespace directory keys are not unique and sorted",
                    ));
                }
                summary.first_key.get_or_insert_with(|| key.clone());
                *previous = Some(key);
                let display = if prefix.is_empty() {
                    child.name().to_owned()
                } else {
                    format!("{prefix}/{}", child.name())
                };
                let location = name_policy::PortablePath::parse(&display).map_err(|source| {
                    Error::from(source).context(
                        ErrorCode::ObjectCorrupt,
                        "namespace path exceeds the portable path budget",
                    )
                })?;
                summary.entries += 1;
                match child {
                    NamespacePageEntry::File { .. } => summary.files += 1,
                    NamespacePageEntry::Directory {
                        entries,
                        files,
                        subtree,
                        ..
                    } => {
                        let verified = self.directory(subtree, location.as_str())?;
                        if verified.entries != *entries || verified.files != *files {
                            return Err(corrupt(
                                "namespace recorded totals differ from the verified subtree",
                            ));
                        }
                        summary.entries += verified.entries;
                        summary.files += verified.files;
                    }
                }
            }
            Ok(summary)
        }

        fn level(
            &mut self,
            reference: &NamespaceReference,
            prefix: &str,
            previous: &mut Option<String>,
            depth: usize,
        ) -> Result<Summary> {
            if depth >= storage_contract::NAMESPACE_INDEX_DEPTH {
                return Err(corrupt("namespace index exceeds the depth bound"));
            }
            match self.source.node(reference)? {
                NamespaceNode::Page(page) => {
                    if page.entries.is_empty() && depth != 0 {
                        return Err(corrupt("a namespace index references an empty page"));
                    }
                    self.page(&page, prefix, previous)
                }
                NamespaceNode::Index(index) => {
                    if depth == 0 && index.children.len() < 2 {
                        return Err(corrupt("namespace index fanout is out of bounds"));
                    }
                    let mut summary = Summary::default();
                    for child in &index.children {
                        // Every index child must be nonempty, so an exhausted
                        // admission budget needs no further object reads.
                        if self.remaining == 0 {
                            return Err(invalid("namespace exceeds the entry limit"));
                        }
                        let verified = self.level(&child.reference, prefix, previous, depth + 1)?;
                        if verified.first_key.as_deref() != Some(child.first_key.as_str()) {
                            return Err(corrupt(
                                "namespace index first key differs from its subgraph",
                            ));
                        }
                        if verified.entries != child.entries || verified.files != child.files {
                            return Err(corrupt("namespace index totals differ from its subgraph"));
                        }
                        let index_height = verified.index_height + 1;
                        if summary.first_key.is_some() && summary.index_height != index_height {
                            return Err(corrupt("namespace index children have different heights"));
                        }
                        summary.index_height = index_height;
                        summary.first_key.get_or_insert(child.first_key.clone());
                        // All added counts were charged against one u64 budget;
                        // recorded, untrusted counts never enter this arithmetic.
                        summary.entries += verified.entries;
                        summary.files += verified.files;
                    }
                    Ok(summary)
                }
            }
        }
    }
    let summary = Verification {
        source,
        remaining: max_entries,
    }
    .directory(&root.root, "")?;
    if summary.entries != root.entries {
        return Err(corrupt(
            "namespace recorded entry total differs from the verified graph",
        ));
    }
    Ok(summary.entries)
}

/// Every directory entry id reachable from one root, pruned by shared
/// subtree references across calls through the caller's visited set.
pub(crate) fn collect_directory_entry_ids(
    reference: &NamespaceReference,
    source: &(impl NamespaceSource + ?Sized),
    visited: &mut BTreeSet<String>,
    out: &mut BTreeSet<String>,
) -> Result<()> {
    if !visited.insert(reference.hash.clone()) {
        return Ok(());
    }
    match source.node(reference)? {
        NamespaceNode::Page(page) => {
            for entry in &page.entries {
                if let NamespacePageEntry::Directory {
                    entry_id, subtree, ..
                } = entry
                {
                    out.insert(entry_id.clone());
                    collect_directory_entry_ids(subtree, source, visited, out)?;
                }
            }
        }
        NamespaceNode::Index(index) => {
            for child in &index.children {
                collect_directory_entry_ids(&child.reference, source, visited, out)?;
            }
        }
    }
    Ok(())
}

/// Which of the watched versions are still referenced under one root, pruned
/// by shared subtree references across calls through the caller's visited set.
pub(crate) fn collect_used_versions(
    reference: &NamespaceReference,
    source: &(impl NamespaceSource + ?Sized),
    visited: &mut BTreeSet<String>,
    watched: &BTreeMap<String, String>,
    out: &mut BTreeSet<String>,
) -> Result<()> {
    if !visited.insert(reference.hash.clone()) {
        return Ok(());
    }
    match source.node(reference)? {
        NamespaceNode::Page(page) => {
            for entry in &page.entries {
                match entry {
                    NamespacePageEntry::File { version_id, .. } => {
                        if watched.contains_key(version_id) {
                            out.insert(version_id.clone());
                        }
                    }
                    NamespacePageEntry::Directory { subtree, .. } => {
                        collect_used_versions(subtree, source, visited, watched, out)?;
                    }
                }
            }
        }
        NamespaceNode::Index(index) => {
            for child in &index.children {
                collect_used_versions(&child.reference, source, visited, watched, out)?;
            }
        }
    }
    Ok(())
}

/// A published-object source over borrowed disk plumbing; each read builds
/// the bounded-cache reader on the spot.
pub(crate) fn resources_source<'a>(
    resources: &'a crate::disk::DiskResources,
    members: &'a [crate::DiskConfig],
    caches: &'a crate::segment_store::SegmentCaches,
) -> LoaderSource<impl Fn(&NamespaceReference) -> Result<NamespaceNode> + 'a> {
    LoaderSource(move |reference: &NamespaceReference| {
        crate::segment_store::SegmentReader {
            resources,
            members,
            caches,
        }
        .namespace_node(reference)
    })
}

/// Signed directory-entry reference deltas between two commit namespaces for
/// the derived owner index: `apply(entry_id, history, ±1)` per occurrence.
/// With an unchanged history identity, shared subtree references contribute
/// identical occurrences on both sides and are pruned without being read; a
/// history change (a split moved the commit) re-counts both complete sides,
/// which is the split's own O(subtree) cost.
pub(crate) fn directory_reference_deltas(
    before: Option<(&CommitNamespace, &str)>,
    after: Option<(&CommitNamespace, &str)>,
    source: &(impl NamespaceSource + ?Sized),
    apply: &mut impl FnMut(&str, &str, i64) -> Result<()>,
) -> Result<()> {
    fn count_subtree(
        reference: &NamespaceReference,
        history: &str,
        sign: i64,
        source: &(impl NamespaceSource + ?Sized),
        apply: &mut impl FnMut(&str, &str, i64) -> Result<()>,
    ) -> Result<()> {
        let mut children = Vec::new();
        directory_children(reference, source, &mut children)?;
        for child in children {
            if let NamespacePageEntry::Directory {
                entry_id, subtree, ..
            } = child
            {
                apply(&entry_id, history, sign)?;
                count_subtree(&subtree, history, sign, source, apply)?;
            }
        }
        Ok(())
    }
    fn diff_directory(
        before: &NamespaceReference,
        after: &NamespaceReference,
        history: &str,
        source: &(impl NamespaceSource + ?Sized),
        apply: &mut impl FnMut(&str, &str, i64) -> Result<()>,
    ) -> Result<()> {
        if before == after {
            return Ok(());
        }
        fn directories(entries: &[NamespacePageEntry]) -> BTreeMap<&str, &NamespaceReference> {
            entries
                .iter()
                .filter_map(|entry| match entry {
                    NamespacePageEntry::Directory {
                        entry_id, subtree, ..
                    } => Some((entry_id.as_str(), subtree)),
                    NamespacePageEntry::File { .. } => None,
                })
                .collect()
        }
        let mut old = Vec::new();
        directory_children(before, source, &mut old)?;
        let mut new = Vec::new();
        directory_children(after, source, &mut new)?;
        let old = directories(&old);
        let new = directories(&new);
        // Match by entity identity: a renamed directory keeps its id and its
        // shared subtree reference, so a pure rename prunes completely.
        for (entry_id, subtree) in &old {
            match new.get(entry_id) {
                Some(next) => diff_directory(subtree, next, history, source, apply)?,
                None => {
                    apply(entry_id, history, -1)?;
                    count_subtree(subtree, history, -1, source, apply)?;
                }
            }
        }
        for (entry_id, subtree) in &new {
            if !old.contains_key(entry_id) {
                apply(entry_id, history, 1)?;
                count_subtree(subtree, history, 1, source, apply)?;
            }
        }
        Ok(())
    }
    match (before, after) {
        (None, None) => Ok(()),
        (Some((namespace, history)), None) => {
            count_subtree(&namespace.root, history, -1, source, apply)
        }
        (None, Some((namespace, history))) => {
            count_subtree(&namespace.root, history, 1, source, apply)
        }
        (Some((old, old_history)), Some((new, new_history))) => {
            if old_history != new_history {
                count_subtree(&old.root, old_history, -1, source, apply)?;
                return count_subtree(&new.root, new_history, 1, source, apply);
            }
            diff_directory(&old.root, &new.root, old_history, source, apply)
        }
    }
}

/// Visit every reachable namespace object of one root (indexes before their
/// children), for repair, scrub and package closures.
pub(crate) fn visit_namespace_objects(
    root: &NamespaceReference,
    load: &mut impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
    visited: &mut BTreeSet<(NamespaceRefKind, String)>,
) -> Result<()> {
    if !visited.insert((root.kind, root.hash.clone())) {
        return Ok(());
    }
    root.validate()?;
    let object = load(root.kind.as_str(), &root.hash)?;
    match NamespaceNode::decode(root, object.value(), object.canonical_len())? {
        NamespaceNode::Page(page) => {
            for entry in &page.entries {
                if let NamespacePageEntry::Directory { subtree, .. } = entry {
                    visit_namespace_objects(subtree, load, visited)?;
                }
            }
        }
        NamespaceNode::Index(index) => {
            for child in &index.children {
                visit_namespace_objects(&child.reference, load, visited)?;
            }
        }
    }
    Ok(())
}

/// Test fixture: build a namespace and keep its objects for a local source.
/// Absent ancestor directories are synthesized with deterministic identities,
/// so loose fixture entry lists stay usable; the production builder itself
/// fails closed on a missing parent.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn fixture_namespace(entries: &[Entry]) -> (CommitNamespace, NamespaceObjects) {
    let mut complete: Vec<Entry> = entries.to_vec();
    let mut present: BTreeSet<String> = entries
        .iter()
        .map(|entry| entry.lookup_key().to_owned())
        .collect();
    for entry in entries {
        let mut parent = entry.location().parent();
        while let Some(ancestor) = parent {
            parent = ancestor.parent();
            if !present.insert(ancestor.key().to_owned()) {
                continue;
            }
            let fold = ancestor.key().bytes().fold(0u128, |acc, byte| {
                acc.wrapping_mul(131).wrapping_add(byte as u128)
            });
            complete.push(Entry {
                entry_id: format!("entry_{fold:032x}"),
                path: ancestor,
                kind: NamespaceEntryKind::Directory,
                file_id: String::new(),
                version_id: String::new(),
                size: 0,
            });
        }
    }
    let mut out = NamespaceObjects::new();
    let namespace = build_from_entries(&complete, &mut out).expect("fixture namespace builds");
    (namespace, out)
}

/// A process-wide, content-addressed object store for synthetic state
/// fixtures that have no disks: tests build commit namespaces through it and
/// resolve them with `FixtureSource`. Content addressing makes cross-test
/// sharing harmless.
#[cfg(test)]
pub(crate) mod fixture_store {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn store() -> &'static Mutex<BTreeMap<String, NamespaceNode>> {
        static STORE: OnceLock<Mutex<BTreeMap<String, NamespaceNode>>> = OnceLock::new();
        STORE.get_or_init(Default::default)
    }

    pub(crate) fn register(objects: NamespaceObjects) {
        let mut store = store().lock().expect("fixture store lock");
        for (hash, node) in objects.decoded {
            store.insert(hash, node);
        }
    }

    pub(crate) fn namespace(entries: &[Entry]) -> CommitNamespace {
        let (namespace, objects) = super::fixture_namespace(entries);
        register(objects);
        namespace
    }

    pub(crate) struct FixtureSource;
    impl NamespaceSource for FixtureSource {
        fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode> {
            store()
                .lock()
                .expect("fixture store lock")
                .get(&reference.hash)
                .cloned()
                .ok_or_else(|| super::corrupt("namespace object is absent from the fixture store"))
        }
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/namespace/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/namespace/mutation_order_tests.rs"]
mod mutation_order_tests;
