//! A frozen, explicitly selected published representation. Not the current
//! working view, a network login, or permission to see the rest of its parent.
use super::{FormalNode,ParentCut,PinnedParent};
use crate::{DirectoryParent,FileId,FileInfo,NodeId,PoolId,pool::lease::RegisteredRead};
use std::{io::{self,Write},path::Path};

/// Keeps the original REGISTRATION shared OFD and the actual selected node
/// materials until dropped, including idle time between separate offset reads.
/// No native file descriptor, SYSTEM path or mutable authority is returned.
pub struct PublishedVersion(RegisteredRead<PinnedParent>);

impl PublishedVersion {
    /// Storage half of an explicit version open. The caller has to authorize
    /// the subject, pool, parent and entry separately. A listing is only a
    /// coordinate and may be stale or caller-modified; never treat it as an
    /// ACL or use this function as a fallback for an ordinary current read.
    pub fn open_listed(system:&Path,pool:PoolId,parent:DirectoryParent,listed:&FileInfo,node:NodeId)->io::Result<Self>{
        let missing=||io::Error::new(io::ErrorKind::NotFound,"no such listed FILE version in this pool and parent");
        if listed.parent!=parent||listed.volume!=parent.volume(){return Err(missing());}
        crate::ordinary::storage_name::FileName::parse(listed.name.as_bytes())?;
        ParentCut::capture(system,|cut|{
            let (real,journal)=cut.originals();
            if real.id()!=pool{return Err(missing());}
            let row=journal.lookup_at(parent,&listed.name)?.ok_or_else(missing)?;
            if FileInfo::at(row,parent)!=*listed{return Err(missing());}
            // The original file and its exact node must belong to this same
            // cut. Historical admission opens the actual current ordinary
            // parent and its native materials; the listing supplies no FD.
            cut.historical(row.create.file,node)
        }).map(Self)
    }
    pub(super) fn by_id(system:&Path,file:FileId,node:NodeId)->io::Result<Self>{
        ParentCut::capture(system,|cut|cut.historical(file,node)).map(Self)
    }
    /// The authenticated original node metadata, not proof of a fresh login
    /// or of the health of every untouched byte.
    pub fn receipt(&self)->&FormalNode{self.0.get().receipt()}
    pub(crate) fn require_held(&self)->io::Result<()>{self.0.get().unchanged()}
    pub(crate) fn finish_headers(&self,out:&mut impl Write)->io::Result<()>{self.0.get().finish_output(out)}
    pub fn read_range_to(&self,offset:u64,length:u64,mut out:impl Write)->io::Result<()>{
        let body=self.0.get();body.write_range(offset,length,&mut out)?;body.finish_output(&mut out)
    }
    pub fn read_to(&self,mut out:impl Write)->io::Result<()>{
        let body=self.0.get();body.write_to(&mut out)?;body.finish_output(&mut out)
    }
    pub(super) fn into_receipt(self)->io::Result<FormalNode>{
        self.0.deliver(|body|Ok(body.into_receipt()))
    }
}
