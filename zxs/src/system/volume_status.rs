//! A single authenticated, on-demand observation of one registered volume.
//! No filesystem information is used as ownership, health or an I/O grant.
use super::{WebAdmin,manual_work::{Site,Who}};
use super::super::{Catalog,InitialReader,volume::{OrdinaryRoot,RecycleRoot}};
use crate::{Role,VolumeId,fs::{HeldDir,invalid},system::ClaimRow};
use std::{fmt,fs::File,io::{self,Read},os::{fd::{AsRawFd,FromRawFd},raw::c_int,unix::fs::MetadataExt},
    time::{Duration,Instant}};

unsafe extern "C"{
    fn zxs_storage_capacity_status(fd:c_int,total:*mut u64,available:*mut u64,read_only:*mut c_int)->c_int;
    fn zxs_storage_uuid(fd:c_int,uuid:*mut u8)->c_int;
    fn zxs_open_ext4_observer(fd:c_int)->c_int;
    fn zxs_open_ext4_counter(observer:c_int,which:c_int)->c_int;
    fn zxs_mount_id(fd:c_int,out:*mut u64)->c_int;
}

/// Exact bytes returned by FS_IOC_GETFSUUID; a short identifier is not filled
/// out to a UUID or reinterpreted as a disk serial.
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct FilesystemId{bytes:[u8;16],len:u8}
impl FilesystemId{
    fn observed(mut bytes:[u8;16],len:c_int)->Option<Self>{
        let len=usize::try_from(len).ok()?;
        let value=bytes.get(..len)?;
        if !value.iter().any(|&b|b!=0){return None;}
        bytes[len..].fill(0); // outside the reported length is not part of the identity
        Some(Self{bytes,len:len as u8})
    }
    pub(crate) fn len(&self)->u8{self.len}
}
impl fmt::Display for FilesystemId{
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{
        for (i,b) in self.bytes[..usize::from(self.len)].iter().enumerate(){
            if self.len==16&&matches!(i,4|6|8|10){f.write_str("-")?;}
            write!(f,"{b:02x}")?;
        }
        Ok(())
    }
}
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct Reading{
    pub(crate) bytes:u64,
    pub(crate) available:u64,
    pub(crate) mount_read_only:bool,
    pub(crate) filesystem_id:Option<FilesystemId>,
    pub(crate) ext4:Ext4Status,
}
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct Report{
    pub(crate) volume:VolumeId,
    pub(crate) role:Role,
    pub(crate) current:Option<Reading>,
}
fn claim(c:&Catalog,id:VolumeId)->io::Result<&ClaimRow>{
    c.rows().iter().find(|p|p.volume==id).ok_or_else(||io::ErrorKind::NotFound.into())
}
enum Disk<'a>{System(&'a InitialReader),File(OrdinaryRoot<'a>),Recycle(RecycleRoot<'a>)}
impl<'a> Disk<'a>{
    fn open(src:&'a InitialReader,c:&Catalog,r:&ClaimRow)->io::Result<Self>{
        match r.role{
            Role::System=>Ok(Self::System(src)), // Already held and selected by the original S cut.
            Role::File=>Ok(Self::File(OrdinaryRoot::for_body(src,c,r.volume)?)), // No FILE or namespace lock.
            Role::Recycle=>Ok(Self::Recycle(RecycleRoot::at(src,c)?)),
        }
    }
    fn root(&self)->&HeldDir{
        match self{
            Self::System(s)=>&s.dir.root,
            Self::File(f)=>&f.root,
            Self::Recycle(r)=>r.root_for_read(),
        }
    }
    fn require(&self)->io::Result<()>{
        match self{
            Self::System(s)=>{s.dir.require()?;s.marker.require(&s.dir.root)},
            Self::File(f)=>f.require_streaming(),
            Self::Recycle(r)=>r.require(),
        }
    }
}
#[derive(Debug,Default,Eq,PartialEq)]
pub(crate) struct Ext4Status{
    pub(crate) recorded_errors:Option<u32>,
    pub(crate) writes:Option<WriteWindow>,
}
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct WriteWindow{
    pub(crate) millis:u16,
    pub(crate) kib:u64,
    pub(crate) kib_per_second:u64,
}
impl WriteWindow{
    fn observed(first:u64,last:u64,span:Duration)->Option<Self>{
        if !(Duration::from_millis(200)..=Duration::from_secs(1)).contains(&span)||
            last>u64::MAX/1024{return None;}
        let kib=last.checked_sub(first)?;
        let kib_per_second=u64::try_from(u128::from(kib)*1_000_000_000/span.as_nanos()).ok()?;
        Some(Self{millis:u16::try_from(span.as_millis()).ok()?,kib,kib_per_second})
    }
}
fn fresh_fd(raw:c_int)->io::Result<File>{
    if raw<0{return Err(io::Error::last_os_error());}
    // SAFETY: only the two native observation openers call this with their
    // unique fd on success, never a borrowed or user-supplied value.
    Ok(unsafe{File::from_raw_fd(raw)})
}
fn ext4_observer(source:&File)->io::Result<File>{
    fresh_fd(unsafe{zxs_open_ext4_observer(source.as_raw_fd())})
}
fn ext4_identity(observer:&File)->io::Result<(u64,u64,u64)>{
    let mut mount=0;
    if unsafe{zxs_mount_id(observer.as_raw_fd(),&mut mount)}<0{return Err(io::Error::last_os_error());}
    let m=observer.metadata()?;
    if mount==0||m.ino()==0{return Err(invalid("ext4 observation directory has no identity"));}
    Ok((m.dev(),m.ino(),mount))
}
#[repr(i32)]
enum Counter{Errors=0,Writes=1}
fn decimal_counter(raw:&[u8])->Option<u64>{
    let n=raw.strip_suffix(b"\n")?;
    if n.is_empty()||n.len()>20||(n[0]==b'0'&&n.len()!=1)||!n.iter().all(u8::is_ascii_digit){return None;}
    std::str::from_utf8(n).ok()?.parse().ok()
}
fn ext4_counter(observer:&File,which:Counter)->Option<u64>{
    let f=fresh_fd(unsafe{zxs_open_ext4_counter(observer.as_raw_fd(),which as i32)}).ok()?;
    let mut raw=Vec::with_capacity(22);f.take(22).read_to_end(&mut raw).ok()?;
    decimal_counter(&raw)
}
fn write_sample(observer:&File)->Option<(Instant,u64)>{
    let a=Instant::now();
    let kib=ext4_counter(observer,Counter::Writes)?;
    let elapsed=Instant::now().checked_duration_since(a)?;
    if elapsed>Duration::from_millis(50){return None;}
    Some((a.checked_add(elapsed/2)?,kib))
}
fn ext4_status(root:&HeldDir,live:&impl Fn()->io::Result<()>)->io::Result<Ext4Status>{
    let Ok(observer)=ext4_observer(&root.file)else{return Ok(Ext4Status::default());};
    let Ok(original)=ext4_identity(&observer)else{return Ok(Ext4Status::default());};
    live()?;
    let recorded_errors=ext4_counter(&observer,Counter::Errors).and_then(|n|u32::try_from(n).ok());
    let mut writes=None;
    if cfg!(target_pointer_width="64"){
        live()?;
        if let Some((a,before))=write_sample(&observer){
            live()?;
            // Passive only; no benchmark or data I/O. This is the one existing
            // on-demand volume worker and it holds no SQLite transaction.
            std::thread::sleep(Duration::from_millis(250));
            live()?;
            writes=write_sample(&observer).and_then(|(b,after)|
                WriteWindow::observed(before,after,b.checked_duration_since(a)?));
        }
    }
    live()?;
    // The original filesystem directory stays open across both cuts. Get its
    // own live observation directory again and recheck it, never adopt a new
    // sysfs mount or a similarly named instance.
    let exact=ext4_observer(&root.file).and_then(|now|ext4_identity(&now)).ok()==Some(original);
    Ok(if exact{Ext4Status{recorded_errors,writes}}else{Ext4Status::default()})
}
fn read(root:&HeldDir,live:&impl Fn()->io::Result<()>)->io::Result<Reading>{
    let(mut bytes,mut available,mut read_only,mut uuid)=(0,0,-1,[0;16]);
    let fd=root.file.as_raw_fd();
    // The actual proved held directory FD; no drive path, file body or fd from HTTP.
    if unsafe{zxs_storage_capacity_status(fd,&mut bytes,&mut available,&mut read_only)}<0{return Err(io::Error::last_os_error());}
    let mount_read_only=match read_only{0=>false,1=>true,_=>return Err(invalid("invalid kernel mount observation"))};
    live()?;
    let len=unsafe{zxs_storage_uuid(fd,uuid.as_mut_ptr())};
    live()?;
    let ext4=ext4_status(root,live)?;
    Ok(Reading{bytes,available,mount_read_only,filesystem_id:FilesystemId::observed(uuid,len),ext4})
}

#[cfg(test)]
mod tests{
    use super::{Duration,FilesystemId,WriteWindow,decimal_counter};
    #[test]
    fn a_short_kernel_id_keeps_its_bytes_and_is_not_padded_to_a_uuid(){
        let kernel=[0x89,0,0x12,0xff,1,2,3,4,5,6,7,8,9,10,11,12];
        let short=FilesystemId::observed(kernel,4).unwrap();
        assert_eq!(short.len(),4);
        assert_eq!(short.to_string(),"890012ff");
        let full=FilesystemId::observed(kernel,16).unwrap();
        assert_eq!(full.to_string(),"890012ff-0102-0304-0506-0708090a0b0c");
        for n in [-1,0,17]{assert!(FilesystemId::observed(kernel,n).is_none());}
        assert!(FilesystemId::observed([0;16],16).is_none());
        let mut zero_prefix=kernel;zero_prefix[..4].fill(0);
        assert!(FilesystemId::observed(zero_prefix,4).is_none()); // no fallback to trailing data
    }
    #[test]
    fn a_rate_requires_two_monotonic_samples_in_a_bounded_window(){
        let good=Duration::from_millis(250);
        assert_eq!(WriteWindow::observed(41,53,good),
            Some(WriteWindow{millis:250,kib:12,kib_per_second:48}));
        assert_eq!(WriteWindow::observed(41,41,good).unwrap().kib,0);
        assert!(WriteWindow::observed(41,40,good).is_none()); // wrap or reset
        assert!(WriteWindow::observed(0,u64::MAX,good).is_none());
        assert!(WriteWindow::observed(0,1,Duration::ZERO).is_none());
        assert!(WriteWindow::observed(0,1,Duration::from_millis(1001)).is_none());
        assert_eq!(decimal_counter(b"0\n"),Some(0)); // positive return of a zero counter
        assert_eq!(decimal_counter(b"18446744073709551615\n"),Some(u64::MAX));
        for bad in [b"1".as_slice(),b"01\n",b"1\n2\n",b" 1\n",b"18446744073709551616\n"]{
            assert!(decimal_counter(bad).is_none());
        }
    }
}
impl Site<'_>{
    pub(crate) fn observe(&self,who:Who,id:VolumeId,live:impl Fn()->io::Result<()>)->io::Result<Report>{
        live()?;
        self.with_admin(who,|admin|admin.observe_after(id,live,|_|{}))
    }
}
impl WebAdmin<'_>{
    pub(in crate::system::reader) fn observe_after(&self,id:VolumeId,live:impl Fn()->io::Result<()>,after:impl FnOnce(&HeldDir))->io::Result<Report>{
        let catalog=self.source.at(|db,c|{self.current(db,&c)?;claim(&c,id)?;Ok(c)})?;
        let original=claim(&catalog,id)?;
        live()?;
        // Neither the SQLite cut nor a lock on a data directory is held during F/R I/O.
        let disk=Disk::open(self.source,&catalog,original);
        let mut current=None;
        if let Ok(held)=&disk{
            live()?;
            if let Ok(value)=read(held.root(),&live){
                after(held.root());
                if held.require().is_ok(){current=Some(value);}
            }
        }
        // Hold that same physical sample through the final short S cut. A false/
        // missing physical observation is reported only to this same current admin.
        self.source.at(|db,c|{
            self.current(db,&c)?;
            if claim(&c,id)?!=original{return Err(invalid("registered volume changed during the observation"));}
            live()
        })?;
        live()?;
        let _held=disk;
        Ok(Report{volume:id,role:original.role,current})
    }
}
