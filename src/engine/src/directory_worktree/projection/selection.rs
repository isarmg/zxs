//! A selected occurrence identifies the effective *FILE branch*. It explicitly
//! does not decide that branch's HEAD, the read route, or a working BODY.
use super::{Content, SnapshotEntryContent, SnapshotFileSource, SnapshotReference, SparseProjection};
use name_policy::PortablePath;
use storage_contract::{BranchId, FileId, FileNodeId, OperationId};
use storage_contract::snapshot_worktree::additions::WorktreeFileTarget;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Occurrence {
    Original { source: SnapshotReference, ordinal: usize, path: PortablePath },
    Added { target: WorktreeFileTarget, birth: OperationId, first: OperationId },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileBranchOccurrence {
    pub(crate) path: PortablePath,
    pub(crate) occurrence: Occurrence,
    pub(crate) file: FileId,
    pub(crate) branch: BranchId,
    /// This proves branch routing. It is never the default selected HEAD.
    pub(crate) routed_from: FileNodeId,
}
impl SparseProjection<'_> {
    pub(crate) fn select_file_branch(&self,path: &PortablePath) -> Option<FileBranchOccurrence> {
        let ordinal = self.entries.binary_search_by(|e| e.path.key().cmp(path.key())).ok()?;
        let row = &self.entries[ordinal];
        // A user selected the spelling returned by the previous admitted listing.
        if row.path != *path { return None; }
        let (occurrence,file,source) = match row.content {
            Content::Directory => return None,
            Content::File { original, .. } => {
                let entry = self.source.entries().get(original)?;
                let SnapshotEntryContent::File {material,source} = entry.content else { return None; };
                let (file,source) = self.results.get(&original).map_or((material.file_id,source), |saved|
                    (saved.material.file_id,saved.source));
                if file != material.file_id { return None; }
                (Occurrence::Original { source: self.source.reference(),ordinal: original,path: entry.path.clone() },file,source)
            }
            Content::Added { entry } => {
                let saved = self.added_results.get(&entry)?;
                let added = self.added.get(&entry)?;
                if saved.target != added.target || saved.birth != added.birth || saved.target.path != row.path
                    || saved.target.file != saved.material.file_id { return None; }
                (Occurrence::Added { target: saved.target.clone(),birth:saved.birth,first:saved.file_operation },
                    saved.target.file,saved.source)
            }
        };
        let SnapshotFileSource::Container {branch,node} = source else { return None; };
        Some(FileBranchOccurrence { path: row.path.clone(),occurrence,file,branch,routed_from:node })
    }
}

/// Native paths are sorted. Bound a single parent's descendant interval, then
/// visit only its immediate entries. Never normalize or scan unrelated roots.
pub(in crate::directory_worktree) fn children<'a,T>(rows:&'a [T], key:Option<String>,
    path:impl Fn(&T)->&PortablePath + Copy + 'a) -> impl Iterator<Item=&'a T> + 'a where T:'a, {
    let range=match &key {
        None => rows,
        Some(key) => {
            let prefix=format!("{key}/");
            let start=rows.partition_point(|r|path(r).key()<prefix.as_str());
            let suffix=&rows[start..];
            &suffix[..suffix.partition_point(|r|path(r).key().starts_with(&prefix))]
        }
    };
    range.iter().filter(move |r|path(*r).key().rsplit_once('/').map(|(p,_)|p)==key.as_deref())
}

impl SparseProjection<'_> {
    /// None is a directory, an admitted unborn addition or a genuinely
    /// snapshot-only FILE. A configured FILE node is never replaced by HEAD.
    pub(in crate::directory_worktree) fn configured_children<'a>(&'a self,key:Option<String>)
        -> impl Iterator<Item=(&'a PortablePath,Option<(FileId,FileNodeId)>)>+'a {
        children(&self.entries,key,|row|&row.path).map(|row| {
            let selected=match row.content {
                Content::Directory => None,
                Content::Added {entry} => self.added_results.get(&entry).map(|result|(result.material.file_id,result.source)),
                Content::File {original,material,source} => Some(self.results.get(&original)
                    .map_or((material.file_id,source),|result|(result.material.file_id,result.source))),
            };
            (&row.path,selected.and_then(|(file,source)|match source {
                SnapshotFileSource::Container {node,..}=>Some((file,node)), SnapshotFileSource::SnapshotOnly=>None,
            }))
        })
    }
}
