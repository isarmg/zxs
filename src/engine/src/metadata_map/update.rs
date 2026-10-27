//! Re-encode only changed canonical map paths; unchanged subtrees stay references.
use super::*;

type Children = BTreeMap<String, Basis>;

// These are deliberately capped leaf-bound comparisons, not persisted counts
// or capacity evidence. Capping always means "must remain an index" except for
// the existing indivisible-single-entity rule.
#[derive(Clone, Copy)]
struct Summary {
    entries: usize,
    entry_bytes: usize,
}
impl Summary {
    fn page_bytes(self) -> usize {
        2 + self.entries.saturating_sub(1) + self.entry_bytes
    }

    fn from_entries(entries: &[Entry]) -> Self {
        Self {
            entries: entries.len().min(PAGE_ENTRIES + 1),
            entry_bytes: entries.iter().fold(0, |total, entry| {
                (total + entry.encoded_bytes).min(PAGE_TARGET_BYTES + 1)
            }),
        }
    }

    fn combine(summaries: impl Iterator<Item = Self>) -> Self {
        summaries.fold(
            Self {
                entries: 0,
                entry_bytes: 0,
            },
            |total, item| Self {
                entries: (total.entries + item.entries).min(PAGE_ENTRIES + 1),
                entry_bytes: (total.entry_bytes + item.entry_bytes).min(PAGE_TARGET_BYTES + 1),
            },
        )
    }
}

struct Basis {
    reference: Reference,
    summary: Summary,
    children: Children,
}
impl Basis {
    fn read<'a>(
        reference: Reference,
        prefix: &str,
        load: &impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
    ) -> Result<Self> {
        reference.validate()?;
        let object = load(&reference)?;
        let (summary, children) = match reference.kind {
            ReferenceKind::Page => {
                let shape = PageShape::read(object)?;
                shape.validate_at(prefix)?;
                let summary = Summary {
                    entries: shape.entries,
                    // The authenticated bytes are canonical, including only
                    // braces and one comma between adjacent entity entries.
                    entry_bytes: (shape.encoded_bytes - 2 - shape.entries.saturating_sub(1))
                        .min(PAGE_TARGET_BYTES + 1),
                };
                (summary, BTreeMap::new())
            }
            ReferenceKind::Index => {
                let index = decode_index(object)?;
                if index.prefix != prefix {
                    return Err(invalid("invalid history graph index"));
                }
                let mut children = BTreeMap::new();
                for (digit, child) in index.children {
                    let child = Self::read(child, &format!("{prefix}{digit}"), load)?;
                    children.insert(digit, child);
                }
                let summary = Summary::combine(children.values().map(|child| child.summary));
                (summary, children)
            }
        };
        Ok(Self {
            reference,
            summary,
            children,
        })
    }

    fn entries<'a>(
        self,
        load: &impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
    ) -> Result<Vec<Entry>> {
        if matches!(self.reference.kind, ReferenceKind::Page) {
            let page = load(&self.reference)?
                .value()
                .as_object()
                .expect("invariant: a map basis leaf has passed the authenticated page boundary");
            return encode_entries(page.clone());
        }
        let mut entries = Vec::new();
        for child in self.children.into_values() {
            entries.extend(child.entries(load)?);
        }
        Ok(entries)
    }
}

enum PlannedNode {
    Reused(Basis),
    Leaf {
        prefix: String,
        entries: Vec<Entry>,
        summary: Summary,
    },
    Index {
        prefix: String,
        children: BTreeMap<String, Self>,
        summary: Summary,
    },
}
impl PlannedNode {
    fn summary(&self) -> Summary {
        match self {
            Self::Reused(basis) => basis.summary,
            Self::Leaf { summary, .. } | Self::Index { summary, .. } => *summary,
        }
    }

    fn entries<'a>(
        self,
        load: &impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
    ) -> Result<Vec<Entry>> {
        match self {
            Self::Reused(basis) => basis.entries(load),
            Self::Leaf { entries, .. } => Ok(entries),
            Self::Index { children, .. } => {
                let mut entries = Vec::new();
                for child in children.into_values() {
                    entries.extend(child.entries(load)?);
                }
                Ok(entries)
            }
        }
    }

    fn encode(self, encoder: &mut Encoder, reused: &mut Vec<Reference>) -> Result<Reference> {
        match self {
            Self::Reused(basis) => {
                reused.push(basis.reference.clone());
                Ok(basis.reference)
            }
            Self::Leaf {
                prefix, entries, ..
            } => encoder.node(entries, prefix),
            Self::Index {
                prefix, children, ..
            } => {
                let children = children
                    .into_iter()
                    .map(|(digit, child)| Ok((digit, child.encode(encoder, reused)?)))
                    .collect::<Result<_>>()?;
                Ok(Reference {
                    kind: ReferenceKind::Index,
                    hash: encoder.object("history-index", &HistoryIndex { prefix, children })?,
                })
            }
        }
    }
}

fn replace<'a>(
    basis: Basis,
    prefix: String,
    changes: Vec<Entry>,
    load: &impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
) -> Result<PlannedNode> {
    if changes.is_empty() {
        return Ok(PlannedNode::Reused(basis));
    }
    if matches!(basis.reference.kind, ReferenceKind::Page) {
        let mut entries = basis
            .entries(load)?
            .into_iter()
            .map(|entry| (entry.key.clone(), entry))
            .collect::<BTreeMap<_, _>>();
        for change in changes {
            let entry = entries
                .get_mut(&change.key)
                .ok_or_else(|| invalid("replacement key is absent from the published map"))?;
            *entry = change;
        }
        let entries = entries.into_values().collect::<Vec<_>>();
        return Ok(PlannedNode::Leaf {
            prefix,
            summary: Summary::from_entries(&entries),
            entries,
        });
    }
    replace_index(basis, prefix, changes, load)
}

fn replace_index<'a>(
    basis: Basis,
    prefix: String,
    changes: Vec<Entry>,
    load: &impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
) -> Result<PlannedNode> {
    let mut buckets = BTreeMap::<String, Vec<Entry>>::new();
    for change in changes {
        let digit = change.route[prefix.len()..prefix.len() + 1].to_owned();
        buckets.entry(digit).or_default().push(change);
    }
    let mut children = BTreeMap::new();
    for (digit, child) in basis.children {
        let changes = buckets.remove(&digit).unwrap_or_default();
        children.insert(
            digit.clone(),
            replace(child, format!("{prefix}{digit}"), changes, load)?,
        );
    }
    if !buckets.is_empty() {
        return Err(invalid("replacement key is absent from the published map"));
    }
    let summary = Summary::combine(children.values().map(PlannedNode::summary));
    let planned = PlannedNode::Index {
        prefix: prefix.clone(),
        children,
        summary,
    };
    if leaf_fits(summary.entries, summary.page_bytes()) {
        Ok(PlannedNode::Leaf {
            prefix,
            entries: planned.entries(load)?,
            summary,
        })
    } else {
        Ok(planned)
    }
}

/// The returned frontier is not a durability receipt. The caller still reads,
/// validates and stores every reachable old dependency under the current policy.
pub(crate) struct MapUpdate {
    pub(crate) reference: Reference,
    pub(crate) reused_roots: Vec<Reference>,
}

impl Encoder {
    pub(crate) fn replace_map_values<'a>(
        &mut self,
        reference: &Reference,
        replacements: Map<String, Value>,
        load: impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
    ) -> Result<MapUpdate> {
        let basis = Basis::read(reference.clone(), "", &load)?;
        let planned = replace(basis, String::new(), encode_entries(replacements)?, &load)?;
        let mut reused_roots = Vec::new();
        let reference = planned.encode(self, &mut reused_roots)?;
        Ok(MapUpdate {
            reference,
            reused_roots,
        })
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/metadata_map/update_tests.rs"]
mod tests;
