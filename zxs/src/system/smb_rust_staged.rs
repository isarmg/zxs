//! Same actual private staging/FLUSH as Native, driven only by original
//! Rust SMB members. Only the immutable buffer can leave the staged borrow.
use super::{DraftStage,Scratch};
use crate::{OperationId,fs::invalid,smb::RustSessionWitness,ordinary::working_round::Used,
    system::reader::smb::{denied,file_number::SmbFileIndex,first_work::{RustFileName,RustReadFile},runtime::RustRoot,work_change::RustWorkWriter}};
use std::{io,rc::{Rc,Weak}};

#[must_use]
pub(crate) struct RustStagedWriter{
    // Release confirmed read resources and anonymous writes before FILE EX.
    base:RustReadFile,
    draft:DraftStage,
    writer:RustWorkWriter,
    own:Rc<()>,
    revision:u64,
    // A failed write-through did not acknowledge its staged bytes. They
    // remain private and immutable until the exact existing draft is flushed.
    awaiting_through:bool,
}
#[must_use]
pub(crate) struct RustStagedState{open:Weak<()>,revision:u64}
impl RustStagedState{
    fn require_same(&self,expected:&Self)->io::Result<()>{
        if !self.open.ptr_eq(&expected.open)||self.revision!=expected.revision{
            return Err(invalid("not this original staged FILE state"));
        }
        Ok(())
    }
}
#[must_use]
pub(crate) struct RustStagedReadProof{state:RustStagedState,used:Used}
/// One observed logical EOF of this original writer, not a SAVE receipt,
/// allocation claim or a means to create/use another writer.
#[must_use]
pub(crate) struct RustStagedStandard{state:RustStagedState,eof:u64}
impl RustStagedStandard{
    pub(crate) fn eof(&self)->u64{self.eof}
}
/// A read-only spelling tied to the actual held writer, not its client
/// path, its scratch inode, the separately held base reader or a new OPEN.
#[must_use]
pub(crate) struct RustStagedName{state:RustStagedState,position:(OperationId,i64),path:String}
impl RustStagedName{
    pub(crate) fn path(&self)->&str{&self.path}
    fn require_identity(&self,state:&RustStagedState,position:(OperationId,i64))->io::Result<()>{
        self.state.require_same(state)?;
        if self.position!=position{return Err(invalid("original writable FILE position changed"));}
        Ok(())
    }
}
impl RustStagedWriter{
    pub(crate) fn open(writer:RustWorkWriter,tree:&RustRoot,member:&RustSessionWitness)->io::Result<Self>{
        if !member.is_create(){return Err(denied());}
        writer.require_current(tree,member)?;
        let scratch=Scratch::before_birth(writer.actual_staging_parent())?;
        Self::prepared(writer,tree,member,scratch)
    }
    pub(in crate::system::reader::smb) fn prepared(
        writer:RustWorkWriter,tree:&RustRoot,member:&RustSessionWitness,scratch:Scratch,
    )->io::Result<Self>{
        if !member.is_create(){return Err(denied());}
        let base=RustReadFile::for_writer(&writer,tree,member)?;
        let draft=DraftStage::new(scratch,base.eof());
        Ok(Self{base,draft,writer,own:Rc::new(()),revision:0,awaiting_through:false})
    }
    pub(crate) fn file(&self)->crate::FileId{self.writer.file()}
    pub(crate) fn belongs_to(&self,tree:&RustRoot)->bool{self.writer.belongs_to(tree)}
    pub(crate) fn eof(&self)->u64{self.draft.eof}
    pub(crate) fn smb_index(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<SmbFileIndex>{
        self.writer.smb_index(tree,member)
    }
    pub(in crate::system::reader::smb) fn actual_writer(&self)->&RustWorkWriter{&self.writer}
    pub(crate) fn state(&self)->RustStagedState{
        RustStagedState{open:Rc::downgrade(&self.own),revision:self.revision}
    }
    fn require_same_state(&self,state:&RustStagedState)->io::Result<()>{
        self.state().require_same(state)
    }
    pub(crate) fn require_state(&self,tree:&RustRoot,member:&RustSessionWitness,state:&RustStagedState)->io::Result<()>{
        self.require_same_state(state)?;
        self.no_unconfirmed_through()?;
        self.require_current(tree,member)
    }
    fn no_unconfirmed_through(&self)->io::Result<()>{
        if self.awaiting_through{
            return Err(io::Error::new(io::ErrorKind::WouldBlock,"complete the original write-through before using staged data"));
        }
        Ok(())
    }
    fn invalidate_prepared(&mut self)->io::Result<()>{
        self.revision=self.revision.checked_add(1).ok_or_else(||invalid("staged FILE state exhausted"))?;
        Ok(())
    }
    pub(crate) fn require_current(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<()>{
        self.writer.require_current(tree,member)
    }
    pub(crate) fn standard(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<RustStagedStandard>{
        let snapshot=RustStagedStandard{state:self.state(),eof:self.eof()};
        self.require_standard(tree,member,&snapshot)?;
        Ok(snapshot)
    }
    pub(crate) fn require_standard(&self,tree:&RustRoot,member:&RustSessionWitness,snapshot:&RustStagedStandard)
        ->io::Result<()>{
        if !member.is_query_info(){return Err(denied());}
        self.require_same_state(&snapshot.state)?;
        self.metadata_ready()?;
        if self.eof()!=snapshot.eof{return Err(invalid("original staged FILE EOF changed"));}
        // Healthy private staged writes are visible to this OPEN without
        // refreshing it, flushing it, or reading an unrelated FILE's size.
        // The original writer still checks its single selected placement.
        self.require_current(tree,member)
    }
    fn metadata_ready(&self)->io::Result<()>{
        self.no_unconfirmed_through()?;
        if self.draft.flush_op.is_some()||self.draft.refresh_op.is_some()||self.draft.spool.poisoned{
            return Err(io::Error::new(io::ErrorKind::WouldBlock,"original staged FILE cannot provide a settled view"));
        }
        Ok(())
    }
    pub(crate) fn normalized_name(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<RustStagedName>{
        if !member.is_query_info(){return Err(denied());}
        self.metadata_ready()?;
        self.require_current(tree,member)?;
        let place=self.writer.original_place();
        let path=RustFileName::format(std::iter::once(self.writer.label()).chain(self.writer.route().components())
            .chain(std::iter::once(place.name.as_str())))?;
        Ok(RustStagedName{state:self.state(),position:(place.op,place.generation),path})
    }
    pub(crate) fn require_normalized_name(&self,tree:&RustRoot,member:&RustSessionWitness,name:&RustStagedName)
        ->io::Result<()>{
        if !member.is_query_info(){return Err(denied());}
        let p=self.writer.original_place();
        name.require_identity(&self.state(),(p.op,p.generation))?;
        self.metadata_ready()?;
        self.require_current(tree,member)
    }
    fn refresh(&mut self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<()>{
        let writer=&self.writer;
        if let Some(latest)=self.draft.refresh_with(||{
            let base=RustReadFile::for_writer(writer,tree,member)?;
            let op=base.working_original();let eof=base.eof();
            Ok((base,op,eof))
        })?{self.base=latest;}
        Ok(())
    }
    fn changing(&mut self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<()>{
        self.no_unconfirmed_through()?;
        self.require_current(tree,member)?;self.refresh(tree,member)?;
        if self.draft.flush_op.is_some(){
            return Err(io::Error::new(io::ErrorKind::WouldBlock,"repeat original FLUSH before more staged writes"));
        }
        Ok(())
    }
    pub(crate) fn checked_write_eof(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<u64>{
        if !member.is_write(){return Err(denied());}
        self.no_unconfirmed_through()?;
        self.require_current(tree,member)?;
        Ok(self.eof())
    }
    pub(crate) fn write(&mut self,tree:&RustRoot,member:&RustSessionWitness,offset:u64,data:&[u8])->io::Result<u32>{
        if !member.is_write(){return Err(denied());}
        if data.is_empty(){
            self.no_unconfirmed_through()?;
            self.require_current(tree,member)?;
            if self.draft.flush_op.is_some()||self.draft.refresh_op.is_some(){
                return Err(io::Error::new(io::ErrorKind::WouldBlock,"settle the original FLUSH before an empty WRITE"));
            }
            return Ok(0);
        }
        self.changing(tree,member)?;
        let count=u32::try_from(data.len()).map_err(|_|invalid("staged WRITE count overflow"))?;
        self.invalidate_prepared()?;
        let writer=&self.writer;
        self.draft.write(offset,data,||writer.require_current(tree,member),||writer.work_space(tree,member))?;
        Ok(count)
    }
    pub(crate) fn resize(&mut self,tree:&RustRoot,member:&RustSessionWitness,eof:u64)->io::Result<()>{
        if !member.is_set_info(){return Err(denied());}
        self.changing(tree,member)?;
        self.invalidate_prepared()?;
        let writer=&self.writer;
        self.draft.resize(eof,||writer.require_current(tree,member))
    }
    pub(crate) fn flush(&mut self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<()>{
        if !self.belongs_to(tree)||!(member.is_write()||member.is_close()||member.is_flush()){return Err(denied());}
        // WRITE-through reaches this only after the same paid WRITE was
        // staged. Freeze before any possible FLUSH failure, including
        // failures before there is an issued operation id.
        if member.is_write(){self.awaiting_through=true;}
        let result=self.flush_original(tree,member);
        if result.is_ok(){self.awaiting_through=false;}
        result
    }
    fn flush_original(&mut self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<()>{
        self.require_current(tree,member)?;
        self.invalidate_prepared()?;
        if self.draft.refresh_op.is_some(){let _=self.refresh(tree,member);return Ok(());} // known committed
        let writer=&self.writer;
        let original=self.base.working_original();
        let saved=self.draft.flush(self.base.eof(),writer.actual_staging_parent(),
            ||writer.work_space(tree,member),|batch|writer.save_packed(tree,member,batch.op,original,
                batch.retained,batch.eof,batch.spans,batch.input).map(drop))?;
        if !saved{return Ok(());}
        let _=self.refresh(tree,member);
        // SAVE is already known committed here. The original socket
        // sender still fences publication on this actual current OPEN.
        Ok(())
    }
    pub(crate) fn read(&mut self,tree:&RustRoot,member:&RustSessionWitness,offset:u64,count:u32)
        ->io::Result<(Vec<u8>,RustStagedReadProof)>{
        if !member.is_read(){return Err(denied());}
        self.no_unconfirmed_through()?;
        self.require_current(tree,member)?;self.refresh(tree,member)?;
        let (data,used)=self.draft.read_buffer(offset,count,|at,len,out|self.base.working_range(tree,member,at,len,out))?;
        let proof=RustStagedReadProof{state:self.state(),used};
        self.require_read(tree,member,&proof)?;
        Ok((data,proof))
    }
    pub(crate) fn require_read(&self,tree:&RustRoot,member:&RustSessionWitness,proof:&RustStagedReadProof)->io::Result<()>{
        if !member.is_read()||self.draft.refresh_op.is_some(){return Err(denied());}
        self.no_unconfirmed_through()?;
        self.require_same_state(&proof.state)?;
        self.base.require_working_range(tree,member,&proof.used)?;
        self.require_current(tree,member)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn staged_normalized_name_rejects_another_writer_revision_or_placement_even_with_the_same_spelling(){
        let owner=Rc::new(());let other=Rc::new(());
        let state=|r,own:&Rc<()>|RustStagedState{open:Rc::downgrade(own),revision:r};
        let original=OperationId([0x73;16]);
        let name=RustStagedName{state:state(4,&owner),position:(original,2),
            path:RustFileName::format(["Data-é","Archive🚀","Final.TXt"].into_iter()).unwrap()};
        assert_eq!(name.path(),r"\Data-é\Archive🚀\Final.TXt");
        assert!(name.require_identity(&state(4,&owner),(original,2)).is_ok());
        for (view,position) in [
            (state(4,&other),(original,2)),
            (state(5,&owner),(original,2)),
            (state(4,&owner),(original,3)),
            (state(4,&owner),(OperationId([0x74;16]),2)),
        ]{
            assert_eq!(name.require_identity(&view,position).unwrap_err().kind(),io::ErrorKind::InvalidInput);
        }
    }
}
