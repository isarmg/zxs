//! Merge lazy directory streams by full lookup-path bytes. A directory's
//! descendants start at `key/`, not immediately after `key`: `a-b` and `a.b`
//! precede `a/c`. The frontier retains only unread subtree references and one
//! continuation per open page/index, never all siblings or materialized entries.
//! Open streams are ancestor-prefix intervals of the current winner; unread
//! directory tickets are prefixes awaiting '/'. Memory therefore follows path
//! depth/key length and bounded index height, not namespace or directory width.
use super::{NamespaceNode, NamespaceSource};
use crate::{Error, Result};
use std::{collections::BTreeMap, sync::Arc};
use storage_contract::{ErrorCode, NamespacePageEntry, NamespaceReference};

struct Prefix {
    display: String,
    key: String,
}

enum Pending {
    Unread {
        reference: NamespaceReference,
        prefix: Arc<Prefix>,
    },
    Cursor {
        node: NamespaceNode,
        position: usize,
        prefix: Arc<Prefix>,
    },
}

fn enqueue_cursor(
    frontier: &mut BTreeMap<String, Pending>,
    node: NamespaceNode,
    position: usize,
    prefix: Arc<Prefix>,
) {
    let component = match &node {
        NamespaceNode::Page(page) => page
            .entries
            .get(position)
            .map(|entry| entry.lookup_key().to_owned()),
        NamespaceNode::Index(index) => index
            .children
            .get(position)
            .map(|child| child.first_key.clone()),
    };
    if let Some(component) = component {
        frontier.insert(
            format!("{}{component}", prefix.key),
            Pending::Cursor {
                node,
                position,
                prefix,
            },
        );
    }
}

pub(super) fn walk(
    reference: &NamespaceReference,
    parent: Option<&name_policy::PortablePath>,
    source: &(impl NamespaceSource + ?Sized),
    mut visit: impl FnMut(&name_policy::PortablePath, &NamespacePageEntry) -> Result<bool>,
) -> Result<()> {
    let prefix = Arc::new(Prefix {
        display: parent.map_or_else(String::new, |path| format!("{}/", path.as_str())),
        key: parent.map_or_else(String::new, |path| format!("{}/", path.key())),
    });
    let mut frontier = BTreeMap::from([(
        prefix.key.clone(),
        Pending::Unread {
            reference: reference.clone(),
            prefix,
        },
    )]);
    while let Some((_, pending)) = frontier.pop_first() {
        match pending {
            Pending::Unread { reference, prefix } => {
                enqueue_cursor(&mut frontier, source.node(&reference)?, 0, prefix);
            }
            Pending::Cursor {
                node: NamespaceNode::Index(index),
                position,
                prefix,
            } => {
                // first_key is the admitted lower bound for this child. It
                // wins before reading the child, including at a page boundary.
                let node = source.node(&index.children[position].reference)?;
                enqueue_cursor(&mut frontier, node, 0, Arc::clone(&prefix));
                enqueue_cursor(
                    &mut frontier,
                    NamespaceNode::Index(index),
                    position + 1,
                    prefix,
                );
            }
            Pending::Cursor {
                node: NamespaceNode::Page(page),
                position,
                prefix,
            } => {
                let entry = &page.entries[position];
                let display = format!("{}{}", prefix.display, entry.name());
                let location = name_policy::PortablePath::parse(&display).map_err(|source| {
                    Error::from(source).context(
                        ErrorCode::ObjectCorrupt,
                        "namespace path exceeds the portable path budget",
                    )
                })?;
                if !visit(&location, entry)? {
                    return Ok(());
                }
                if let NamespacePageEntry::Directory { subtree, .. } = entry {
                    let prefix = Arc::new(Prefix {
                        display: format!("{display}/"),
                        key: format!("{}/", location.key()),
                    });
                    frontier.insert(
                        prefix.key.clone(),
                        Pending::Unread {
                            reference: subtree.clone(),
                            prefix,
                        },
                    );
                }
                enqueue_cursor(
                    &mut frontier,
                    NamespaceNode::Page(page),
                    position + 1,
                    prefix,
                );
            }
        }
    }
    Ok(())
}
