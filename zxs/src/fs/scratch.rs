//! A newly created, held private job directory and its single disposable
//! RAW candidate. Never adopt, truncate, replace or remove an existing
//! name. The enclosing service must select the parent/slot from its original
//! task, capacity grant and current placement before it creates the job.
use super::*;
use std::os::unix::fs::FileExt;

const RAW:&CStr=c"RAW";

unsafe extern "C"{fn zxs_create_new_regular(parent:c_int,name:*const c_char)->c_int;}

/// Only a successful exclusive mkdir can produce this value. In particular
/// a caller cannot feed an existing empty FILE, SYMBOLIC LINK, R or SYSTEM
/// directory to the Raw writer as though it were this run's new scratch.
pub(crate) struct FreshJob{dir:HeldDir,exact_bytes:u64}
impl FreshJob{
    // The live portable job issuer is not enabled. Only source fixtures may
    // presently choose a new scratch parent/slot; callers with an arbitrary
    // existing HeldDir cannot turn it into a write target.
    #[cfg(test)]
    pub(crate) fn create(parent:&HeldDir,slot:&CStr,exact_bytes:u64,credit:u64)->io::Result<Self>{
        if exact_bytes>credit||exact_bytes>i64::MAX as u64{return Err(invalid("original task does not fund its full RAW"));}
        // Naming authority is the caller's; the ordinary filesystem decides
        // if this fresh name is free. No open-existing or reuse-on-EEXIST.
        WitnessRead::require_parent(parent)?;
        let dir=parent.new_child_directory(slot)?;
        dir.exclusive()?;WitnessRead::require_parent(&dir)?;dir.only(None)?;
        dir.file.sync_all()?;parent.file.sync_all()?;
        WitnessRead::require_parent(parent)?;dir.require_named()?;Ok(Self{dir,exact_bytes})
    }
    pub(crate) fn exact_bytes(&self)->u64{self.exact_bytes}
    pub(crate) fn new_raw(&self)->io::Result<RawWriter<'_>>{
        WitnessRead::require_parent(&self.dir)?;self.dir.only(None)?;
        self.dir.checked_child_path(RAW)?;
        // SAFETY: a single constant name in the actually retained job. No
        // write occurs before getting a new O_EXCL result and checking it.
        let fd=unsafe{zxs_create_new_regular(self.dir.file.as_raw_fd(),RAW.as_ptr())};
        if fd<0{return Err(io::Error::last_os_error());}
        let file=unsafe{File::from_raw_fd(fd)};file.try_lock().map_err(io::Error::from)?;
        let original=record_stamp(&file,0)?.entity();
        let w=RawWriter{job:self,file,original,at:0,limit:self.exact_bytes};
        w.named(0)?;Ok(w)
    }
}

/// This is deliberately the only sink accepted by the complete-copy path.
/// Caller-provided &File/Write could be a hard link to a source or a prior job.
pub(crate) struct RawWriter<'a>{job:&'a FreshJob,file:File,original:Entity,at:u64,limit:u64}
impl RawWriter<'_>{
    fn named(&self,size:u64)->io::Result<FileStamp>{
        let dir=&self.job.dir;WitnessRead::require_parent(dir)?;dir.only(Some(RAW))?;
        let stamp=record_stamp(&self.file,size)?;
        let pin=dir.pin_record(RAW)?.ok_or_else(||invalid("fresh RAW lost its only job name"))?;
        if stamp.entity()!=self.original||stamp.size()!=size||record_stamp(&pin,size)?!=stamp||
            !dir.shares_file_mount(&pin)?||!dir.shares_file_mount(&self.file)?{
            return Err(invalid("private RAW name, owner, size or original changed"));}
        Ok(stamp)
    }
}
impl Write for RawWriter<'_>{
    fn write(&mut self,bytes:&[u8])->io::Result<usize>{
        let n=u64::try_from(bytes.len()).map_err(|_|invalid("copy write too large"))?;
        if self.at.checked_add(n).is_none_or(|v|v>self.limit){return Err(invalid("copy exceeds fixed logical EOF"));}
        // Explicit writes even for all-zero logical intervals. Never seek
        // over holes, preallocate, clone, or use a filesystem copy shortcut.
        let used=self.file.write_at(bytes,self.at)?;
        self.at+=used as u64;Ok(used)
    }
    fn flush(&mut self)->io::Result<()>{self.file.sync_all()}
}

/// Local evidence only. No operation here can rename/install, advance a
/// ledger, delete a source, or remove the original. Failure keeps the job.
pub(crate) struct CopiedRaw<'a>{writer:RawWriter<'a>,seen:File,stamp:FileStamp}
impl<'a> RawWriter<'a>{
    pub(crate) fn synced(self)->io::Result<CopiedRaw<'a>>{
        if self.at!=self.limit{return Err(invalid("copy stopped before its exact EOF"));}
        self.file.sync_all()?;let stamp=self.named(self.limit)?;
        self.job.dir.file.sync_all()?;
        let pin=self.job.dir.pin_record(RAW)?.ok_or_else(||invalid("no named RAW for independent reread"))?;
        if record_stamp(&pin,self.limit)?!=stamp||!self.job.dir.shares_file_mount(&pin)?{
            return Err(invalid("independent RAW name no longer identifies the held copy"));}
        // SAFETY: O_PATH pinned the checked ordinary named object; an
        // independent read-only open description, not a dup of the writer.
        let fd=unsafe{zxs_open_pinned_record(pin.as_raw_fd(),0)};if fd<0{return Err(io::Error::last_os_error());}
        let seen=unsafe{File::from_raw_fd(fd)};let read=CopiedRaw{writer:self,seen,stamp};
        read.require()?;Ok(read)
    }
}
impl CopiedRaw<'_>{
    /// A held private copy cannot supply feedback for a different newly
    /// created job, even when two callers selected the same logical node.
    pub(crate) fn from_job(&self,job:&FreshJob)->bool{std::ptr::eq(self.writer.job,job)}
    pub(crate) fn require(&self)->io::Result<()>{
        let n=self.writer.limit;
        if self.writer.named(n)?!=self.stamp||record_stamp(&self.seen,n)?!=self.stamp||
            !self.writer.job.dir.shares_file_mount(&self.seen)?{return Err(invalid("named copied RAW changed"));}
        Ok(())
    }
    pub(crate) fn read_at(&self,b:&mut[u8],at:u64)->io::Result<()>{
        if at.checked_add(b.len() as u64).is_none_or(|end|end>self.stamp.size()){return Err(invalid("copy reread exceeds EOF"));}
        self.seen.read_exact_at(b,at)
    }
}
