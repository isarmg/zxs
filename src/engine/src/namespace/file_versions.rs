//! Traverse only namespace object references admitted by a caller's derived
//! first-publisher index. Each source read authenticates its bounded page/index.
use super::*;

pub(crate) fn visit_file_versions(
    root: &CommitNamespace,
    source: &(impl NamespaceSource + ?Sized),
    visit_reference: &mut impl FnMut(&NamespaceReference) -> bool,
    visit_version: &mut impl FnMut(&str),
) -> Result<()> {
    fn visit(
        reference: &NamespaceReference,
        source: &(impl NamespaceSource + ?Sized),
        visit_reference: &mut impl FnMut(&NamespaceReference) -> bool,
        visit_version: &mut impl FnMut(&str),
    ) -> Result<()> {
        if !visit_reference(reference) {
            return Ok(());
        }
        match source.node(reference)? {
            NamespaceNode::Index(index) => {
                for child in &index.children {
                    visit(&child.reference, source, visit_reference, visit_version)?;
                }
            }
            NamespaceNode::Page(page) => {
                for entry in &page.entries {
                    match entry {
                        NamespacePageEntry::File { version_id, .. } => visit_version(version_id),
                        NamespacePageEntry::Directory { subtree, .. } => {
                            visit(subtree, source, visit_reference, visit_version)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    visit(&root.root, source, visit_reference, visit_version)
}
