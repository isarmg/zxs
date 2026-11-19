//! The data plane of a tentative automatic full copy. A successful return
//! is not a representation switch, published Node, receipt or erase grant.
//! The caller still needs the original live job/placement/budget and the
//! same regular REG lease; currently no business or network issuer is live.
use super::{Descriptor,LocalVersion,ReadBudget};
use crate::{fs::{CopiedRaw,FreshJob,WitnessRead,invalid},ordinary::flat::logical::{LogicalV1,Whole}};
use std::io::{self,Write};

/// These terms are frozen when the job's own directory and tangible byte
/// allowance are issued. They cannot be supplied again by a source at
/// copy time. No ordinary caller can construct or redirect one today.
pub(in crate::ordinary) struct CopyJob{scratch:FreshJob,source:Descriptor,source_dir:crate::fs::HeldDir,content:LogicalV1}
impl CopyJob{
    #[cfg(test)]
    pub(super) fn create(parent:&crate::fs::HeldDir,slot:&std::ffi::CStr,view:&LocalVersion<'_>,
        content:LogicalV1,credit:u64)->io::Result<Self>{
        let source=view.descriptor;
        WitnessRead::require_parent(view.parent)?;let source_dir=view.parent.duplicate()?;
        if source.logical_bytes!=content.length(){return Err(invalid("original job index and content length do not agree"));}
        let scratch=FreshJob::create(parent,slot,content.length(),credit)?;
        Ok(Self{scratch,source,source_dir,content})
    }
}

/// Neither bytes nor an operation for a caller-selected path are exposed.
/// One job's result cannot be reused under another file/node/representation
/// or a different content commitment; a later decision still needs to bind
/// this exact material and issue its own physical approval.
pub(in crate::ordinary) struct RawReadback<'a>{target:CopiedRaw<'a>,source:Descriptor,content:LogicalV1}
impl RawReadback<'_>{
    pub(in crate::ordinary) fn require(&self,job:&CopyJob,current:Descriptor,content:LogicalV1)->io::Result<()>{
        if !self.target.from_job(&job.scratch)||job.source!=current||job.content!=content||self.source!=current||self.content!=content{return Err(invalid("full-copy feedback belongs to another original task"));}
        self.target.require()
    }
}

impl LocalVersion<'_>{
    pub(in crate::ordinary) fn materialize_in<'j>(&self,job:&'j CopyJob,budget:ReadBudget)->io::Result<RawReadback<'j>>{
        self.materialize_after(job,budget,||Ok(()))
    }

    pub(super) fn materialize_after<'j>(&self,job:&'j CopyJob,budget:ReadBudget,after_writes:impl FnOnce()->io::Result<()>)->io::Result<RawReadback<'j>>{
        let expected=job.content;let n=self.contents.len();
        if job.source!=self.descriptor||expected.length()!=n||job.scratch.exact_bytes()!=n||job.source_dir.path!=self.parent.path||
            job.source_dir.entity!=self.parent.entity||!job.source_dir.same_mount(self.parent)?{return Err(invalid("copy does not match the selected original version/length"));}
        let size=n.min(64*1024).min(budget.buffer_bytes as u64) as usize;
        if n!=0&&size==0{return Err(invalid("insufficient private readback buffer budget"));}
        WitnessRead::require_parent(self.parent)?;
        // One FILE census for the whole tentative copy; no attached ancestor
        // versions and at most one open source payload/table pair at a time.
        let scope=WitnessRead::at(self.parent)?;
        let target=self.contents.copy_to_fresh_job(&scope,&job.scratch,budget,after_writes)?;
        WitnessRead::require_parent(self.parent)?;self.contents.unchanged()?;
        // The actual named destination and its parent were synced and an
        // independent read-only FD opened for THIS original. Read it all,
        // including zero ranges, not an in-memory copy/checksum shortcut.
        let mut bytes=Vec::new();bytes.try_reserve_exact(size).map_err(|_|invalid("cannot allocate full-copy reread buffer"))?;
        bytes.resize(size,0);let mut whole=Whole::new(n)?;let mut offset=0;
        while offset<n{
            let k=(n-offset).min(size as u64) as usize;
            target.read_at(&mut bytes[..k],offset)?;whole.write_all(&bytes[..k])?;offset+=k as u64;
        }
        if whole.finish()?!=expected{return Err(invalid("independent named complete RAW differs from original logical content"));}
        self.contents.unchanged()?;WitnessRead::require_parent(self.parent)?;
        let result=RawReadback{target,source:self.descriptor,content:expected};result.require(job,self.descriptor,expected)?;Ok(result)
    }
}
