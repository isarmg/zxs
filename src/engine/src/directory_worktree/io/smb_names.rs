//! SMB presents the exact configured FILE node's immutable logical basename.
//! Keep its separate internal occurrence path for handles and writes. No FILE
//! read, physical rename, branch-head selection or persistent index is issued.
use super::*;
use crate::directory_worktree::projection::selection::children;
use crate::working_tree::snapshot::SnapshotFileSource;
use std::collections::{BTreeMap,BTreeSet};
use storage_contract::{FileId,FileNodeId};

impl HeldReadView<'_> {
    fn smb_names(&self,state:&crate::State,parent:Option<&LookupPath>,maximum:usize)
        -> Result<BTreeMap<String,(PortablePath,PortablePath)>,ViewError> {
        self.ledger.admit_read_view().map_err(ViewError::Ledger)?;
        if let Some(parent)=parent && !matches!(self.admitted_content(parent)?,EntryContent::Directory) {
            return Err(ViewError::NotDirectory);
        }
        let mut names=BTreeMap::new();
        let mut insert=|path:&PortablePath, configured:Option<(FileId,FileNodeId)>| {
            if names.len()>=maximum {return Err(ViewError::Budget);}
            let display=display_path(state,path,configured)?;
            if names.insert(display.key().to_owned(),(display,path.clone())).is_some(){return Err(ViewError::DisplayCollision);}
            Ok(())
        };
        match &self.backing {
            Backing::Sparse {projection,..}=>for (path,configured) in projection.configured_children(parent.map(|p|p.key().to_owned())){insert(path,configured)?;},
            Backing::Published {node}=>for row in children(node.manifest().entries(),parent.map(|p|p.key().to_owned()),|entry|&entry.path){
                let configured=match row.content {
                    SnapshotEntryContent::File {material,source:SnapshotFileSource::Container {node,..}}=>Some((material.file_id,node)),
                    SnapshotEntryContent::File {source:SnapshotFileSource::SnapshotOnly,..}|SnapshotEntryContent::Directory {}=>None,
                };
                insert(&row.path,configured)?;
            },
        }
        Ok(names)
    }

    pub(crate) fn smb_entry(&self,state:&crate::State,path:&LookupPath,maximum:usize)
        -> Result<(PortablePath,Entry),ViewError> {
        let parent=path.as_str().rsplit_once('/').map(|(p,_)|LookupPath::parse(p)).transpose().map_err(ViewError::DisplayPath)?;
        let (display,source)=self.smb_names(state,parent.as_ref(),maximum)?.remove(path.key()).ok_or(ViewError::Missing)?;
        let content=self.admitted_content(&LookupPath::from(&source))?;
        Ok((display,Entry {path:source,content}))
    }
    pub(crate) fn smb_directory(&self,state:&crate::State,parent:Option<&LookupPath>,cursor:Option<&Cursor>,maximum:usize)
        -> Result<(Vec<(PortablePath,Entry)>,Option<Cursor>),ViewError> {
        if let Some(c)=cursor && (!c.smb || c.stamp != self.stamp || c.parent.as_ref()!=parent){return Err(ViewError::Cursor);}
        let mut output=Vec::new(); let mut bytes=2usize;
        for (_, (display,source)) in self.smb_names(state,parent,maximum)? {
            if cursor.is_some_and(|c|display.key()<=c.after.key()){continue;}
            let content=self.admitted_content(&LookupPath::from(&source))?;
            let entry=Entry {path:source,content};
            let size=storage_contract::canonical_json(&(&display,&entry)).map_err(ViewError::Encoding)?.len();
            let next=bytes.checked_add(size).and_then(|n|n.checked_add(1)).ok_or(ViewError::Budget)?;
            if output.len()==usize::from(MAX_PAGE_ENTRIES) || next>MAX_PAGE_BYTES {
                let last:&(PortablePath,Entry)=output.last().ok_or(ViewError::Budget)?;
                let c=Cursor {smb:true,stamp:self.stamp,parent:parent.cloned(),after:LookupPath::from(&last.0)};
                return Ok((output,Some(c)));
            }
            bytes=next; output.push((display,entry));
        }
        Ok((output,None))
    }
}

fn display_path(state:&crate::State,path:&PortablePath,configured:Option<(FileId,FileNodeId)>) -> Result<PortablePath,ViewError> {
    let Some((file,node))=configured else{return Ok(path.clone());};
    let name=crate::business::file_nodes::recorded_name(state,file,node).map_err(ViewError::DisplayNode)?;
    match path.parent(){Some(prefix)=>prefix.join_name(&name),None=>PortablePath::from_name(&name)}.map_err(ViewError::DisplayPath)
}

/// A fresh native producer must reject a future ambiguous or unservable
/// display before its own decision, not rely on a conflicted SMB listing later.
pub(crate) fn require_configured_manifest(state:&crate::State,manifest:&crate::working_tree::snapshot::AdmittedSnapshotManifest,
    mount:Option<&name_policy::Name>) -> Result<(),ViewError> {
    let parent=mount.map(PortablePath::from_name).transpose().map_err(ViewError::DisplayPath)?;
    let mut keys=BTreeSet::new();
    for row in manifest.entries(){
        let configured=match row.content {
            SnapshotEntryContent::File {material,source:SnapshotFileSource::Container {node,..}}=>Some((material.file_id,node)),
            SnapshotEntryContent::File {source:SnapshotFileSource::SnapshotOnly,..}|SnapshotEntryContent::Directory {}=>None,
        };
        let display=display_path(state,&row.path,configured)?;
        if let Some(prefix)=&parent {display.rebase(0,prefix).map_err(ViewError::DisplayPath)?;}
        if !keys.insert(display.key().to_owned()){return Err(ViewError::DisplayCollision);}
    }
    Ok(())
}
