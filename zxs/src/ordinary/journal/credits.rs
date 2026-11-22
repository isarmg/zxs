//! Derived only while replaying the same original SYSTEM prefix. No credit
//! is a physical placement, access decision, persisted counter or promise
//! of live disk space. Only a confirmed, admitted original deletion creates one.
use super::{MAX_DECISIONS,MAX_JOURNAL};
use crate::{FileId,OperationId,fs::invalid,ordinary::wire::MAX_FRAME};
use std::{collections::BTreeMap,io};

/// One outstanding return *from this deletion*, not a generic slot that
/// another operation, a later same-name FILE or a stale restore can spend.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Deletion{pub file:FileId,pub operation:OperationId,pub decision:[u8;32]}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) enum Effect{Ordinary,Retain{file:FileId,operation:OperationId},Restore(Deletion)}
#[derive(Default)]
pub(super) struct Credits{by_file:BTreeMap<FileId,Deletion>}
impl Credits{
    fn resulting(&self,e:Effect)->io::Result<usize>{
        let count=self.by_file.len();
        match e{
            Effect::Ordinary=>Some(count),
            Effect::Retain{file,..} if !self.by_file.contains_key(&file)=>count.checked_add(1),
            Effect::Restore(k) if self.by_file.get(&k.file)==Some(&k)=>count.checked_sub(1),
            _=>None
        }.ok_or_else(||invalid("operation cannot acquire or consume this original FILE restore credit"))
    }
    pub(super) fn room(&self,n:usize,used:usize,size:usize,e:Effect)->io::Result<()>{
        if size==0||size>MAX_FRAME{return Err(invalid("a FILE decision must fit one bounded original frame"));}
        let k=self.resulting(e)?;
        let count=n.checked_add(1).and_then(|n|n.checked_add(k));
        let bytes=used.checked_add(size).and_then(|n|k.checked_mul(MAX_FRAME).and_then(|reserve|n.checked_add(reserve)));
        if !count.is_some_and(|n|n<=MAX_DECISIONS)||!bytes.is_some_and(|n|n<=MAX_JOURNAL){
            return Err(invalid("SYSTEM budget cannot fit this decision plus one return for every retained FILE"));
        }Ok(())
    }
    /// Called after the original frame passed native admission at its prefix.
    pub(super) fn apply(&mut self,e:Effect,decision:[u8;32])->io::Result<()>{
        self.resulting(e)?;
        match e{
            Effect::Ordinary=>{},
            Effect::Retain{file,operation}=>{self.by_file.insert(file,Deletion{file,operation,decision});},
            Effect::Restore(k)=>{self.by_file.remove(&k.file);}
        } Ok(())
    }
    /// When the reader has proved that just the last, fully encoded frame is
    /// ahead of the durable lower bound, expose the preceding credits only.
    pub(super) fn before_unconfirmed(&mut self,e:Effect,decision:[u8;32])->io::Result<()>{
        match e{
            Effect::Ordinary=>{},
            Effect::Retain{file,operation}=>{
                if self.by_file.get(&file)!=Some(&Deletion{file,operation,decision}){return Err(invalid("unconfirmed DELETE has a different original credit"));}
                self.by_file.remove(&file);
            }
            Effect::Restore(k)=>{
                if self.by_file.contains_key(&k.file){return Err(invalid("unconfirmed RESTORE would replace a different credit"));}
                self.by_file.insert(k.file,k);
            }
        }Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/attached/ordinary/journal/credits/tests.rs"]
mod tests;
