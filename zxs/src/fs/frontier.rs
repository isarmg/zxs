//! The original SYSTEM register's acknowledged lower bound. It is a small
//! attribute of that very inode, not another journal or a way to reconstruct
//! a missing frame. Only the business publisher may advance it after frame sync.
use super::*;

pub(crate) const FRONTIER_LEN:usize=48;
pub(crate) type Frontier=[u8;FRONTIER_LEN];
pub(crate) fn frontier(seq:u64,frame_bytes:usize,head:[u8;32])->Frontier{
    let mut out=[0;FRONTIER_LEN];
    out[..8].copy_from_slice(&seq.to_be_bytes());
    out[8..16].copy_from_slice(&(frame_bytes as u64).to_be_bytes());
    out[16..].copy_from_slice(&head);
    out
}
unsafe extern "C" {
    fn zxs_get_frontier(fd:c_int,out:*mut u8,len:usize)->isize;
    fn zxs_set_frontier(fd:c_int,value:*const u8,len:usize,create:c_int)->c_int;
    fn zxs_get_stage(fd:c_int,out:*mut u8,len:usize)->isize;
    fn zxs_set_stage(fd:c_int,value:*const u8,len:usize,create:c_int)->c_int;
}
fn read(file:&File)->io::Result<Frontier>{
    let mut bytes=[0;FRONTIER_LEN];
    // SAFETY: only a validated original private regular file is passed here.
    let n=unsafe{zxs_get_frontier(file.as_raw_fd(),bytes.as_mut_ptr(),bytes.len())};
    if n<0{return Err(io::Error::last_os_error());}
    if n!=FRONTIER_LEN as isize{return Err(invalid("invalid acknowledged SYSTEM frontier length"));}
    Ok(bytes)
}
impl stream::PinnedRegular{
    /// Read the attribute of the actual held file description rather than
    /// opening a second regular FD to stand in for its lifetime flock.
    pub(crate) fn opening_frontier(&self)->io::Result<Frontier>{
        self.unchanged()?;let value=read(&self.file)?;self.unchanged()?;Ok(value)
    }
}
pub(crate) fn initial_frontier(file:&File,genesis:[u8;32])->io::Result<()>{
    if file.metadata()?.nlink()!=0{return Err(invalid("cannot initialize a frontier on a named SYSTEM file"));}
    let first=frontier(0,0,genesis);
    // SAFETY: only our fresh unpublished anonymous genesis; never an upgrade
    // of already named media. Its contents and extended attribute sync together.
    os_result(unsafe{zxs_set_frontier(file.as_raw_fd(),first.as_ptr(),first.len(),1)})
}

pub(crate) fn initial_attempt_binding(file:&File,unbound:&[u8])->io::Result<()>{
    if file.metadata()?.nlink()!=0{return Err(invalid("cannot initialize a named FILE allocation"));}
    // SAFETY: fixed attribute on this new anonymous reservation only.
    os_result(unsafe{zxs_set_stage(file.as_raw_fd(),unbound.as_ptr(),unbound.len(),1)})
}
fn read_stage(file:&File)->io::Result<Vec<u8>>{
    let mut bytes=[0u8;256];
    // SAFETY: fixed name and bounded buffer on the verified original inode.
    let n=unsafe{zxs_get_stage(file.as_raw_fd(),bytes.as_mut_ptr(),bytes.len())};
    if n<0{return Err(io::Error::last_os_error());}
    Ok(bytes.get(..n as usize).ok_or_else(||invalid("oversized original stage binding"))?.to_vec())
}

impl HeldDir{
    pub(crate) fn read_attempt_binding(&self,name:&CStr,original:&OriginalStamp,max:usize)->io::Result<Vec<u8>>{
        let held=self.held_frontier_file(name,original,max,false)?;let value=read_stage(&held)?;
        self.confirm_original_name(name,original,max)?;Ok(value)
    }
    pub(crate) fn bind_attempt(&self,name:&CStr,original:&OriginalStamp,max:usize,empty:&[u8],bound:&[u8])->io::Result<()>{
        self.bind_attempt_at(name,original,max,empty,bound,||Ok(()))
    }
    pub(crate) fn bind_attempt_at(&self,name:&CStr,original:&OriginalStamp,max:usize,empty:&[u8],bound:&[u8],
        after_set:impl FnOnce()->io::Result<()>)->io::Result<()>{
        let held=self.held_frontier_file(name,original,max,true)?;
        if read_stage(&held)?.as_slice()!=empty{return Err(invalid("cannot rebind a different FILE stage"));}
        // SAFETY: exclusive SYSTEM, compare the original phase first. Replace
        // only its existing value on this inode; never create a missing binding.
        os_result(unsafe{zxs_set_stage(held.as_raw_fd(),bound.as_ptr(),bound.len(),0)})?;
        after_set()?; // visible phase does not yet prove its inode was synced
        held.sync_all()?;
        if read_stage(&held)?.as_slice()!=bound{return Err(invalid("original stage binding readback differs"));}
        let after=OriginalStamp{stat:record_stamp(&held,max as u64)?,birth:original.birth.clone()};
        self.confirm_original_name(name,&after,max)
    }
    fn held_frontier_file(&self,name:&CStr,original:&OriginalStamp,max:usize,write:bool)->io::Result<File>{
        let pin=self.pin_record(name)?.ok_or_else(||invalid("original SYSTEM register missing"))?;
        let m=mounts::id(&self.file)?;
        if record_stamp(&pin,max as u64)?!=original.stat || mounts::id(&pin)?!=m{
            return Err(invalid("SYSTEM frontier no longer on the original named file"));
        }
        // SAFETY: exact original regular, same-mount inode, opened via held O_PATH.
        let fd=unsafe{zxs_open_pinned_record(pin.as_raw_fd(),if write {1}else{0})};
        if fd<0{return Err(io::Error::last_os_error());}
        let held=unsafe{File::from_raw_fd(fd)};
        if record_stamp(&held,max as u64)?!=original.stat || mounts::id(&held)?!=m{
            return Err(invalid("SYSTEM frontier pin differs"));
        }
        original.birth.require(&Birth::read(&held)?)?;
        Ok(held)
    }
    pub(crate) fn read_frontier(&self,name:&CStr,original:&OriginalStamp,max:usize)->io::Result<Frontier>{
        let held=self.held_frontier_file(name,original,max,false)?;
        let value=read(&held)?;
        self.confirm_original_name(name,original,max)?;
        Ok(value)
    }
    pub(crate) fn advance_frontier(&self,name:&CStr,original:&OriginalStamp,max:usize,
        expected:&Frontier,next:&Frontier,after_set:impl FnOnce()->io::Result<()>)->io::Result<()>{
        let held=self.held_frontier_file(name,original,max,true)?;
        if read(&held)?!=*expected{return Err(invalid("SYSTEM acknowledgement changed since its original frame was verified"));}
        // SAFETY: same original inode and exclusive SYSTEM lease, after exact
        // old and new frame bytes were re-read and fsynced. REPLACE cannot add
        // a missing checkpoint. One xattr is replaced atomically by the kernel.
        os_result(unsafe{zxs_set_frontier(held.as_raw_fd(),next.as_ptr(),next.len(),0)})?;
        after_set()?; // exact interruption point before the acknowledging inode sync
        held.sync_all()?;
        if read(&held)?!=*next{return Err(invalid("SYSTEM acknowledgement readback differs"));}
        let after=OriginalStamp{stat:record_stamp(&held,max as u64)?,birth:original.birth.clone()};
        if after.stat.entity()!=original.stat.entity()||after.stat.size()!=original.stat.size(){
            return Err(invalid("SYSTEM register changed during acknowledgement"));
        }
        self.confirm_original_name(name,&after,max)
    }
}
