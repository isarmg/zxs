//! Transaction-local copy-on-write B+ nodes. Only edited routes and the one
//! sibling needed for underflow repair are opened; sealing encodes dirty nodes
//! once, after every operation in the directory overlay has been applied.
use super::*;

const MIN_INDEX_CHILDREN: usize = NAMESPACE_INDEX_FANOUT / 2;
const PAGE_ENVELOPE_BYTES: usize = b"{\"entries\":[],\"format\":\"namespace-page\"}".len();

struct Leaf {
    entries: Vec<NamespacePageEntry>,
    sizes: Vec<usize>,
}
impl Leaf {
    fn new(entries: Vec<NamespacePageEntry>) -> Result<Self> {
        let sizes = entries
            .iter()
            .map(|entry| Ok(canonical_json(entry)?.len()))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { entries, sizes })
    }
    fn bytes(&self) -> usize {
        // Canonical empty page envelope, including the two array brackets.
        PAGE_ENVELOPE_BYTES
            + self.sizes.iter().sum::<usize>()
            + self.entries.len().saturating_sub(1)
    }
    fn fits(&self) -> bool {
        self.entries.len() <= NAMESPACE_PAGE_ENTRIES
            && (self.entries.len() <= 1 || self.bytes() <= NAMESPACE_PAGE_TARGET_BYTES)
    }
    fn split_off(&mut self, at: usize) -> Self {
        Self {
            entries: self.entries.split_off(at),
            sizes: self.sizes.split_off(at),
        }
    }
    fn append(&mut self, other: Self) {
        self.entries.extend(other.entries);
        self.sizes.extend(other.sizes);
    }
    fn split(&mut self) -> Vec<Self> {
        if self.fits() {
            return Vec::new();
        }
        // Prefer the most even legal two-way split. A single oversized entry
        // in the middle can require three leaves; isolate it without scanning
        // any other page. Every other entry was already bounded by a page.
        let envelope = PAGE_ENVELOPE_BYTES;
        let total: usize = self.sizes.iter().sum();
        let mut prefix = 0;
        let mut best = None;
        for at in 1..self.entries.len() {
            prefix += self.sizes[at - 1];
            let right_len = self.entries.len() - at;
            let left_bytes = envelope + prefix + at - 1;
            let right_bytes = envelope + total - prefix + right_len - 1;
            if at <= NAMESPACE_PAGE_ENTRIES
                && right_len <= NAMESPACE_PAGE_ENTRIES
                && (at == 1 || left_bytes <= NAMESPACE_PAGE_TARGET_BYTES)
                && (right_len == 1 || right_bytes <= NAMESPACE_PAGE_TARGET_BYTES)
            {
                let score = left_bytes
                    .max(right_bytes)
                    .max(at.max(right_len) * NAMESPACE_PAGE_TARGET_BYTES / NAMESPACE_PAGE_ENTRIES);
                if best.is_none_or(|(_, previous)| score < previous) {
                    best = Some((at, score));
                }
            }
        }
        if let Some((at, _)) = best {
            return vec![self.split_off(at)];
        }
        // A large middle value can prevent *both* two-way cuts even when it
        // fits alone. Greedily take one bounded prefix, then partition the
        // remaining bounded suffix (at most two further pages for one edit).
        let mut bytes = envelope + self.sizes[0];
        let mut at = 1;
        while at < self.entries.len()
            && at < NAMESPACE_PAGE_ENTRIES
            && bytes + 1 + self.sizes[at] <= NAMESPACE_PAGE_TARGET_BYTES
        {
            bytes += 1 + self.sizes[at];
            at += 1;
        }
        let mut right = self.split_off(at);
        let tail = right.split();
        let mut result = vec![right];
        result.extend(tail);
        result
    }
}

enum Body {
    Page(Leaf),
    Index(Vec<TreeNode>),
}
struct TreeNode {
    original: Option<NamespaceIndexChild>,
    body: Option<Body>,
    dirty: bool,
}
impl TreeNode {
    fn sealed(child: NamespaceIndexChild) -> Self {
        Self {
            original: Some(child),
            body: None,
            dirty: false,
        }
    }
    fn fresh(body: Body) -> Self {
        Self {
            original: None,
            body: Some(body),
            dirty: true,
        }
    }
    fn open(&mut self, source: &(impl NamespaceSource + ?Sized)) -> Result<()> {
        if self.body.is_none() {
            let reference = &self
                .original
                .as_ref()
                .expect("invariant: unopened node has an address")
                .reference;
            self.body = Some(match source.node(reference)? {
                NamespaceNode::Page(page) => Body::Page(Leaf::new(page.entries.clone())?),
                NamespaceNode::Index(index) => {
                    Body::Index(index.children.iter().cloned().map(Self::sealed).collect())
                }
            });
        }
        Ok(())
    }
    fn first_key(&self) -> String {
        match &self.body {
            Some(Body::Page(page)) => page
                .entries
                .first()
                .expect("invariant: live index children are nonempty")
                .lookup_key(),
            Some(Body::Index(children)) => children
                .first()
                .expect("invariant: live index children are nonempty")
                .first_key(),
            None => self
                .original
                .as_ref()
                .expect("invariant: sealed address exists")
                .first_key
                .clone(),
        }
    }
    fn empty(&self) -> bool {
        match &self.body {
            Some(Body::Page(page)) => page.entries.is_empty(),
            Some(Body::Index(children)) => children.is_empty(),
            None => false,
        }
    }
    fn underfull(&self) -> bool {
        match &self.body {
            Some(Body::Page(page)) => {
                page.entries.len() < NAMESPACE_PAGE_ENTRIES / 2
                    && page.bytes() < NAMESPACE_PAGE_TARGET_BYTES / 2
            }
            Some(Body::Index(children)) => children.len() < MIN_INDEX_CHILDREN,
            None => false,
        }
    }
    fn split(&mut self) -> Vec<Self> {
        match self.body.as_mut().expect("invariant: edited node is open") {
            Body::Page(page) => page
                .split()
                .into_iter()
                .map(|page| Self::fresh(Body::Page(page)))
                .collect(),
            Body::Index(children) if children.len() > NAMESPACE_INDEX_FANOUT => {
                let right = children.split_off(children.len() / 2);
                vec![Self::fresh(Body::Index(right))]
            }
            Body::Index(_) => Vec::new(),
        }
    }
    fn edit(
        &mut self,
        key: &str,
        value: Option<NamespacePageEntry>,
        source: &(impl NamespaceSource + ?Sized),
    ) -> Result<Vec<Self>> {
        self.open(source)?;
        self.dirty = true;
        match self.body.as_mut().expect("invariant: node was opened") {
            Body::Page(page) => {
                let at = page
                    .entries
                    .binary_search_by(|entry| entry.lookup_key().as_str().cmp(key));
                match (at, value) {
                    (Ok(at), Some(value)) => {
                        page.sizes[at] = canonical_json(&value)?.len();
                        page.entries[at] = value;
                    }
                    (Err(at), Some(value)) => {
                        page.sizes.insert(at, canonical_json(&value)?.len());
                        page.entries.insert(at, value);
                    }
                    (Ok(at), None) => {
                        page.entries.remove(at);
                        page.sizes.remove(at);
                    }
                    (Err(_), None) => {
                        unreachable!("invariant: directory overlay deletes an existing key")
                    }
                }
            }
            Body::Index(children) => {
                let at = children
                    .partition_point(|child| child.first_key().as_str() <= key)
                    .saturating_sub(1);
                let right = children[at].edit(key, value, source)?;
                children.splice(at + 1..at + 1, right);
                if children[at].empty() {
                    children.remove(at);
                } else if children[at].underfull() && children.len() > 1 {
                    Self::rebalance(children, at, source)?;
                }
            }
        }
        Ok(self.split())
    }
    fn rebalance(
        children: &mut Vec<Self>,
        at: usize,
        source: &(impl NamespaceSource + ?Sized),
    ) -> Result<()> {
        let left = if at + 1 < children.len() { at } else { at - 1 };
        children[left].open(source)?;
        children[left + 1].open(source)?;
        let right = children.remove(left + 1);
        let node = &mut children[left];
        match (
            node.body.as_mut().expect("invariant: left is open"),
            right.body.expect("invariant: right is open"),
        ) {
            (Body::Page(left), Body::Page(right)) => left.append(right),
            (Body::Index(left), Body::Index(right)) => left.extend(right),
            _ => unreachable!("invariant: admitted balanced siblings retain equal heights"),
        }
        node.dirty = true;
        let right = node.split();
        children.splice(left + 1..left + 1, right);
        Ok(())
    }
    fn height(&mut self, source: &(impl NamespaceSource + ?Sized)) -> Result<usize> {
        self.open(source)?;
        match self.body.as_mut().expect("invariant: node is open") {
            Body::Page(_) => Ok(1),
            Body::Index(children) => Ok(1 + children[0].height(source)?),
        }
    }
    fn seal(self, out: &mut NamespaceObjects) -> Result<NamespaceIndexChild> {
        if !self.dirty {
            return Ok(self
                .original
                .expect("invariant: clean node retains its address"));
        }
        let (first_key, entries, files, reference) =
            match self.body.expect("invariant: dirty node is open") {
                Body::Page(page) => {
                    let first_key = page
                        .entries
                        .first()
                        .map(NamespacePageEntry::lookup_key)
                        .unwrap_or_default();
                    let (entries, files) = entry_totals(&page.entries);
                    let reference = out.page(NamespacePage {
                        format: Default::default(),
                        entries: page.entries,
                    })?;
                    (first_key, entries, files, reference)
                }
                Body::Index(children) => {
                    let children = children
                        .into_iter()
                        .map(|child| child.seal(out))
                        .collect::<Result<Vec<_>>>()?;
                    let first_key = children[0].first_key.clone();
                    let (entries, files) = index_totals(&children);
                    let reference = out.index(NamespaceIndex {
                        format: Default::default(),
                        children,
                    })?;
                    (first_key, entries, files, reference)
                }
            };
        Ok(NamespaceIndexChild {
            first_key,
            entries,
            files,
            reference,
        })
    }
}

// Published totals were admitted against the namespace limit. The private
// overlay only moves/removes existing children or adds addressable entries;
// these are sums of the actual current tree, not unverified wire counters.
fn entry_totals(entries: &[NamespacePageEntry]) -> (u64, u64) {
    entries.iter().fold((0, 0), |(total, file_total), entry| {
        let (entries, files) = match entry {
            NamespacePageEntry::File { .. } => (1, 1),
            NamespacePageEntry::Directory { entries, files, .. } => (1 + entries, *files),
        };
        (total + entries, file_total + files)
    })
}
fn index_totals(children: &[NamespaceIndexChild]) -> (u64, u64) {
    children.iter().fold((0, 0), |(entries, files), child| {
        (entries + child.entries, files + child.files)
    })
}

pub(super) struct PageTree(TreeNode);
impl PageTree {
    pub(super) fn new(base: Option<NamespaceIndexChild>) -> Self {
        Self(match base {
            Some(base) => TreeNode::sealed(base),
            None => TreeNode::fresh(Body::Page(Leaf {
                entries: Vec::new(),
                sizes: Vec::new(),
            })),
        })
    }
    pub(super) fn edit(
        &mut self,
        key: &str,
        value: Option<NamespacePageEntry>,
        source: &(impl NamespaceSource + ?Sized),
    ) -> Result<()> {
        let right = self.0.edit(key, value, source)?;
        if !right.is_empty() {
            let left = std::mem::replace(&mut self.0, TreeNode::fresh(Body::Index(Vec::new())));
            let mut children = vec![left];
            children.extend(right);
            self.0 = TreeNode::fresh(Body::Index(children));
        }
        // An empty root is the one empty page; a single-child index is always
        // collapsed, even after cascading underflow at several levels.
        loop {
            // A promoted sibling can still be sealed. Its interior fanout of
            // one was valid before promotion, but is not a valid root shape.
            self.0.open(source)?;
            match self.0.body.as_mut().expect("invariant: root is open") {
                Body::Index(children) if children.is_empty() => {
                    self.0 = TreeNode::fresh(Body::Page(Leaf {
                        entries: Vec::new(),
                        sizes: Vec::new(),
                    }));
                }
                Body::Index(children) if children.len() == 1 => {
                    self.0 = children.pop().expect("invariant: one child remains");
                }
                Body::Page(_) | Body::Index(_) => break,
            }
        }
        Ok(())
    }
    pub(super) fn seal(
        mut self,
        source: &(impl NamespaceSource + ?Sized),
        out: &mut NamespaceObjects,
    ) -> Result<(NamespaceReference, u64, u64)> {
        if self.0.dirty && self.0.height(source)? > storage_contract::NAMESPACE_INDEX_DEPTH {
            return Err(invalid("namespace directory exceeds the index depth"));
        }
        let root = self.0.seal(out)?;
        Ok((root.reference, root.entries, root.files))
    }
}
