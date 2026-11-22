//! TreeIds are owned by the real published Session. There is one
//! application share; a client may make distinct connections to it.
use crate::{smb::connection::{Fault,Result},system::reader::smb::runtime::RustRoot};
use std::num::NonZeroU32;

const PER_SESSION:usize=16;

/// Chosen only by the original session's table, not by the client's echo
/// or backend. A proposed number alone is neither published nor writable.
pub(in crate::smb) struct ProposedTreeId(NonZeroU32);
impl ProposedTreeId {
    pub(in crate::smb) fn get(&self)->u32 {self.0.get()}
}
pub(in crate::smb::connection::sessions) struct TreeTable {
    next:u32,
    reserved:Option<NonZeroU32>,
    live:Vec<(NonZeroU32,RustRoot)>,
}
impl TreeTable {
    pub(in crate::smb::connection::sessions) fn new()->Self{
        Self{next:1,reserved:None,live:Vec::new()}
    }
    pub(super) fn propose(&self)->Result<ProposedTreeId>{
        if self.live.len()>=PER_SESSION{return Err(Fault::Resources);}
        if self.reserved.is_some(){return Err(Fault::SessionState);}
        let id=NonZeroU32::new(self.next).filter(|n|n.get()!=u32::MAX).ok_or(Fault::Exhausted)?;
        Ok(ProposedTreeId(id))
    }
    pub(super) fn reserve(&mut self,v:&ProposedTreeId)->Result<()>{
        if self.propose()?.0!=v.0{return Err(Fault::SessionState);}
        self.live.try_reserve(1).map_err(|_|Fault::Resources)?;
        self.reserved=Some(v.0);self.next+=1;
        Ok(())
    }
    pub(super) fn waiting(&self,id:&ProposedTreeId)->bool{self.reserved==Some(id.0)}
    pub(super) fn publish(&mut self,id:ProposedTreeId,root:RustRoot)->Result<()>{
        if !self.waiting(&id){return Err(Fault::SessionState);}
        self.reserved=None;
        self.live.push((id.0,root)); // capacity already reserved by the same owner
        Ok(())
    }
    pub(super) fn root(&self,id:u32)->Option<&RustRoot>{
        self.live.iter().find(|(n,_)|n.get()==id).map(|(_,r)|r)
    }
    pub(super) fn retire(&mut self,id:u32)->Result<()>{
        if self.reserved.is_some(){return Err(Fault::SessionState);}
        let at=self.live.iter().position(|(n,_)|n.get()==id).ok_or(Fault::SessionState)?;
        drop(self.live.swap_remove(at));Ok(())
    }
}

#[cfg(test)]
#[path="../../../../../tests/smb/tree_table.rs"]
mod tests;

#[cfg(test)]
include!("../../../../../tests/unit/attached/smb/connection/sessions/ordinary/trees/items.rs");
