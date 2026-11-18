//! Derived only while replaying the same original SYSTEM prefix. No credit
//! is a physical placement, access decision, persisted counter or promise
//! of live disk space. No current FILE opcode can mint a retention credit.
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
mod tests{
    use super::*;
    fn deletion(file:u8,op:u8,hash:u8)->Deletion{Deletion{file:FileId([file;16]),operation:OperationId([op;16]),decision:[hash;32]}}
    fn retain(k:Deletion)->Effect{Effect::Retain{file:k.file,operation:k.operation}}
    #[test]
    fn successful_delete_needs_room_for_itself_and_one_worst_size_return_in_both_dimensions(){
        let c=Credits::default();let k=deletion(1,2,3);
        for(n,used)in [(MAX_DECISIONS-1,0),(32,MAX_JOURNAL-2*MAX_FRAME+1)]{
            assert!(c.room(n,used,MAX_FRAME,Effect::Ordinary).is_ok());
            assert!(c.room(n,used,MAX_FRAME,retain(k)).is_err());
        }
        assert!(c.room(MAX_DECISIONS-2,MAX_JOURNAL-2*MAX_FRAME,MAX_FRAME,retain(k)).is_ok());
        assert!(c.room(usize::MAX,0,1,Effect::Ordinary).is_err());
        assert!(c.room(0,usize::MAX,1,Effect::Ordinary).is_err());
        for wrong in [0,MAX_FRAME+1]{assert!(c.room(0,0,wrong,Effect::Ordinary).is_err());}
    }
    #[test]
    fn two_distinct_originals_keep_both_returns_at_each_limit_and_cannot_be_spent_by_an_unrelated_writer(){
        let(a,b)=(deletion(1,10,41),deletion(2,20,42));
        for(n,used)in [(MAX_DECISIONS-2,4096),(100,MAX_JOURNAL-2*MAX_FRAME)]{
            let mut c=Credits::default();c.apply(retain(a),a.decision).unwrap();c.apply(retain(b),b.decision).unwrap();
            assert!(c.room(n,used,MAX_FRAME,Effect::Ordinary).is_err());
            assert!(c.room(n,used,MAX_FRAME,retain(deletion(3,30,43))).is_err());
            for stale in [deletion(1,11,41),deletion(1,10,40),deletion(2,10,41)]{
                assert!(c.room(n,used,MAX_FRAME,Effect::Restore(stale)).is_err());
            }
            c.room(n,used,MAX_FRAME,Effect::Restore(a)).unwrap();c.apply(Effect::Restore(a),[51;32]).unwrap();
            assert!(c.room(n+1,used+MAX_FRAME,MAX_FRAME,Effect::Ordinary).is_err());
            assert!(c.room(n+1,used+MAX_FRAME,1,Effect::Restore(a)).is_err());
            c.room(n+1,used+MAX_FRAME,MAX_FRAME,Effect::Restore(b)).unwrap();
        }
    }
    #[test]
    fn a_complete_but_unconfirmed_last_frame_never_releases_its_credit(){
        let(a,later)=(deletion(1,2,3),deletion(1,4,5));let mut c=Credits::default();
        c.apply(retain(a),a.decision).unwrap();c.before_unconfirmed(retain(a),a.decision).unwrap();
        assert!(c.resulting(Effect::Restore(a)).is_err());
        c.apply(retain(a),a.decision).unwrap();c.apply(Effect::Restore(a),[81;32]).unwrap();
        c.before_unconfirmed(Effect::Restore(a),[81;32]).unwrap();
        assert!(c.resulting(retain(later)).is_err());assert!(c.resulting(Effect::Restore(later)).is_err());
        assert!(c.room(MAX_DECISIONS-1,10,16,Effect::Ordinary).is_err());
        c.room(MAX_DECISIONS-1,10,MAX_FRAME,Effect::Restore(a)).unwrap();
        c.apply(Effect::Restore(a),[81;32]).unwrap();c.apply(retain(later),later.decision).unwrap();
        assert!(c.resulting(Effect::Restore(a)).is_err());
    }
}
