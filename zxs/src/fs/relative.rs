//! Original-parent operations. No caller-supplied pathname or purported inode
//! can authorize a move, release of an intent, or an append to SYSTEM.
use super::*;
use std::{ffi::OsStr, os::unix::fs::FileExt};

fn leaf(name:&CStr)->io::Result<&OsStr>{
    let b=name.to_bytes();
    if b.is_empty()||b.len()>255||b==b"."||b==b".."||b.contains(&b'/'){
        return Err(invalid("not a single bounded native name"));
    }
    Ok(OsStr::from_bytes(b))
}

impl PathBudget{
    pub(crate) fn child(self,name:&CStr)->io::Result<Self>{
        let width=leaf(name)?.as_bytes().len();
        let bytes=self.bytes.checked_add(1).and_then(|n|n.checked_add(width)).filter(|n|*n<=MAX_PATH_BYTES)
            .ok_or_else(||invalid("child exceeds original absolute path byte limit"))?;
        let depth=self.depth.checked_add(1).filter(|n|*n<=MAX_PATH_DEPTH)
            .ok_or_else(||invalid("child exceeds original absolute path depth limit"))?;
        Ok(Self{bytes,depth})
    }
}
impl HeldDir {
    /// Observe one bounded child name without opening its contents or
    /// returning a handle for an untrusted device, FIFO, or symlink.
    pub(crate) fn entry_present(&self,name:&CStr)->io::Result<bool>{
        leaf(name)?;self.require_named()?;let present=self.pin_record(name)?.is_some();self.require_named()?;Ok(present)
    }
    /// The directory API may accept an alias that the business namespace
    /// does not. Never treat such a lookup as vacancy or the requested name.
    /// O_PATH occupancy never opens an unknown special file for content.
    pub(crate) fn exact_presence(&self,listed:&[Vec<u8>],name:&CStr)->io::Result<bool>{
        let listed=listed.iter().any(|s|s.as_slice()==name.to_bytes());
        if self.entry_present(name)?!=listed{return Err(invalid("native lookup and exact enumerated spelling disagree"));}
        Ok(listed)
    }
    pub(crate) fn exact_directory(&self,name:&CStr)->io::Result<Option<HeldDir>>{
        let observed=self.names()?.iter().any(|r|r.as_slice()==name.to_bytes());
        self.directory_if_exact(name,observed)
    }
    /// A bulk reader supplies its same-held-parent name snapshot and must
    /// compare that parent again before reporting the enumeration. Identity,
    /// lookup, mount and the actual named object are still checked per entry.
    pub(crate) fn directory_in_sample(&self,name:&CStr,sample:&std::collections::BTreeSet<Vec<u8>>)->io::Result<Option<HeldDir>>{
        self.directory_if_exact(name,sample.contains(name.to_bytes()))
    }
    fn directory_if_exact(&self,name:&CStr,present:bool)->io::Result<Option<HeldDir>>{
        if self.entry_present(name)?!=present{return Err(invalid("native lookup and original directory spelling disagree"));}
        let d=self.child_directory(name)?;
        if d.is_some()!=present{return Err(invalid("original directory spelling changed during held lookup"));}
        if let Some(h)=&d{h.require_named()?;}self.require_named()?;Ok(d)
    }
    pub(crate) fn checked_child_path(&self,name:&CStr)->io::Result<PathBuf>{
        let path=self.path.join(leaf(name)?);check_path(&path)?;Ok(path)
    }
    pub(crate) fn child_directory(&self,name:&CStr)->io::Result<Option<HeldDir>>{
        let path=self.checked_child_path(name)?;
        let file=match child(&self.file,name) {
            Ok(file)=>file,
            Err(e) if e.kind()==io::ErrorKind::NotFound=>return Ok(None),
            Err(e)=>return Err(e),
        };
        if mounts::id(&file)?!=mounts::id(&self.file)?{return Err(invalid("FILE directory crosses its original mount"));}
        let meta=file.metadata()?;
        if meta.uid()!=local_uid()||meta.mode()&0o7777!=0o700{
            return Err(invalid("FILE container directory is not private to the local owner"));
        }
        let held=HeldDir{entity:entity(&file)?,file,path};
        self.require_named()?;held.require_named()?;
        Ok(Some(held))
    }
    pub(crate) fn new_child_directory(&self,name:&CStr)->io::Result<HeldDir>{
        check_path(&self.path.join(leaf(name)?))?;
        self.require_named()?;
        // SAFETY: name is a bounded single component and the parent FD is held.
        os_result(unsafe{zxs_mkdir_private(self.file.as_raw_fd(),name.as_ptr())})?;
        self.child_directory(name)?.ok_or_else(||invalid("new FILE directory vanished"))
    }
    pub(crate) fn names(&self)->io::Result<Vec<Vec<u8>>>{
        let mut buf=vec![0u8;DIRECTORY_BYTES];
        // SAFETY: the live output buffer is cap bytes and the directory is held.
        let n=unsafe{zxs_root_names(self.file.as_raw_fd(),buf.as_mut_ptr(),buf.len(),DIRECTORY_NAMES)};
        if n<0{return Err(io::Error::last_os_error());}
        let mut input=buf.get(..n as usize).ok_or_else(||invalid("invalid directory listing length"))?;
        let mut result=Vec::new();
        while let Some((&len,tail))=input.split_first(){
            let (name,rest)=tail.split_at_checked(usize::from(len)).ok_or_else(||invalid("incomplete name enumeration"))?;
            if name.is_empty(){return Err(invalid("invalid directory name"));}
            result.push(name.to_vec());input=rest;
        }
        Ok(result)
    }
    pub(crate) fn install_child(&self,old:&CStr,new:&CStr,birth:&Birth)->io::Result<()>{
        leaf(old)?;leaf(new)?;self.require_named()?;
        let original=self.child_directory(old)?.ok_or_else(||invalid("original prepared FILE directory absent"))?;
        birth.require(&Birth::read(&original.file)?)?;
        // SAFETY: the exact original directory is still held; both components
        // belong to this parent. renameat2 refuses to replace any destination.
        os_result(unsafe{zxs_rename_fresh(self.file.as_raw_fd(),old.as_ptr(),self.file.as_raw_fd(),new.as_ptr())})?;
        self.file.sync_all()
    }
    /// True same-mount directory rename between two held, already registered
    /// parents. Never copy on EXDEV, follow a new path or overwrite any entry.
    pub(crate) fn handoff_directory(&self,old:&CStr,target:&HeldDir,new:&CStr,entity:Entity,birth:&Birth,
        after:impl FnOnce()->io::Result<()>)->io::Result<()>{
        leaf(old)?;leaf(new)?;self.require_named()?;target.require_named()?;
        if !self.same_mount(target)?{return Err(invalid("original rename no longer selects one held mount"));}
        let held=self.child_directory(old)?.ok_or_else(||invalid("original MOVE directory is absent"))?;
        if held.entity!=entity||Birth::read(&held.file)?!=*birth||target.pin_record(new)?.is_some(){return Err(invalid("wrong source or occupied target for original MOVE"));}
        // SAFETY: both directory descriptors are held; checked single-component
        // names; renameat2(RENAME_NOREPLACE). Never chase an alternative target.
        os_result(unsafe{zxs_rename_fresh(self.file.as_raw_fd(),old.as_ptr(),target.file.as_raw_fd(),new.as_ptr())})?;
        after()?;target.file.sync_all()?;self.file.sync_all()?;
        self.require_named()?;target.require_named()?;
        self.confirm_absence(old)?;
        let there=target.child_directory(new)?.ok_or_else(||invalid("renamed FILE absent at exact target"))?;
        if there.entity!=entity||Birth::read(&there.file)?!=*birth{return Err(invalid("different FILE after native rename"));}Ok(())
    }
    /// Never recursive. The caller holds the original empty source plus all
    /// applicable original retirement seals. The kernel also refuses nonempty.
    pub(crate) fn retire_empty_original(&self,name:&CStr,held:&HeldDir,birth:&Birth,after:impl FnOnce()->io::Result<()>)->io::Result<()>{
        let check=self.child_directory(name)?.ok_or_else(||invalid("source directory already missing at retirement"))?;
        if check.entity!=held.entity||Birth::read(&check.file)?!=*birth||Birth::read(&held.file)?!=*birth||!check.same_mount(held)?{
            return Err(invalid("refuse to retire a different FILE directory"));
        }
        held.require_named()?;held.only(None)?;held.file.sync_all()?;self.require_named()?;
        // SAFETY: only this checked empty original directory, AT_REMOVEDIR.
        os_result(unsafe{zxs_remove_empty_dir(self.file.as_raw_fd(),name.as_ptr())})?;after()?;self.confirm_absence(name)
    }
    pub(crate) fn remove_original_record(&self,name:&CStr,stamp:&OriginalStamp,bytes:&[u8])->io::Result<()>{
        leaf(name)?;self.require_named()?;
        let (actual,record)=self.record(name,bytes.len())?.ok_or_else(||invalid("original intent absent"))?;
        if &actual!=stamp||record!=bytes{return Err(invalid("cannot release a different intent"));}
        // SAFETY: only the exact, target-checked regular intent is being
        // released, under the original exclusive SYSTEM lease.
        os_result(unsafe{zxs_remove_name(self.file.as_raw_fd(),name.as_ptr())})?;
        self.file.sync_all()
    }
    pub(crate) fn require_preparation_token(&self,name:&CStr,expected:&OriginalStamp,token:&[u8;32],max:usize)->io::Result<()>{
        let pin=self.pin_record(name)?.ok_or_else(||invalid("original initial material disappeared"))?;
        if record_stamp(&pin,max as u64)?!=expected.stat||mounts::id(&pin)?!=mounts::id(&self.file)?{
            return Err(invalid("initial material changed before original token check"));
        }
        // SAFETY: only the pinned private regular object is opened; its native
        // birth and exact token are rechecked before any acceptance.
        let fd=unsafe{zxs_open_pinned_record(pin.as_raw_fd(),0)};
        if fd<0{return Err(io::Error::last_os_error());}
        let f=unsafe{File::from_raw_fd(fd)};
        if record_stamp(&f,max as u64)?!=expected.stat{return Err(invalid("different initial material record"));}
        expected.birth.require(&Birth::read(&f)?)?;
        let mut actual=[0u8;32];
        let n=unsafe{zxs_get_material_token(f.as_raw_fd(),actual.as_mut_ptr(),actual.len())};
        if n<0{return Err(io::Error::last_os_error());}
        if n!=32||&actual!=token{return Err(invalid("not the original prepared FILE record"));}
        Ok(())
    }
    pub(crate) fn confirm_original_name(&self,name:&CStr,expected:&OriginalStamp,max:usize)->io::Result<()>{
        let named=self.pin_record(name)?.ok_or_else(||invalid("named original SYSTEM object disappeared"))?;
        if record_stamp(&named,max as u64)?!=expected.stat || mounts::id(&named)?!=mounts::id(&self.file)?{
            return Err(invalid("named original SYSTEM object changed"));
        }
        self.require_named()
    }
    pub(crate) fn confirm_record_sync(&self,name:&CStr,expected:&OriginalStamp,max:usize)->io::Result<()>{
        let pin=self.pin_record(name)?.ok_or_else(||invalid("original SYSTEM inode disappeared"))?;
        if record_stamp(&pin,max as u64)?!=expected.stat || mounts::id(&pin)?!=mounts::id(&self.file)?{
            return Err(invalid("SYSTEM inode changed before durability check"));
        }
        // SAFETY: open only the just-pinned original private regular object.
        let fd=unsafe{zxs_open_pinned_record(pin.as_raw_fd(),0)};
        if fd<0{return Err(io::Error::last_os_error());}
        let f=unsafe{File::from_raw_fd(fd)};
        if record_stamp(&f,max as u64)?!=expected.stat||mounts::id(&f)?!=mounts::id(&pin)?{
            return Err(invalid("SYSTEM durability check lost its original file"));
        }
        expected.birth.require(&Birth::read(&f)?)?;
        f.sync_all()?;
        // Keep f live while validating the name, so its inode cannot be recycled.
        self.confirm_original_name(name,expected,max)
    }
    pub(crate) fn append_exact(&self,name:&CStr,expected:&OriginalStamp,offset:u64,suffix:&[u8],max:usize)->io::Result<()>{
        leaf(name)?;self.require_named()?;
        let pin=self.pin_record(name)?.ok_or_else(||invalid("SYSTEM journal disappeared"))?;
        if record_stamp(&pin,max as u64)?!=expected.stat||mounts::id(&pin)?!=mounts::id(&self.file)?{
            return Err(invalid("SYSTEM append no longer names the inspected private inode"));
        }
        if expected.stat.len!=offset || offset.checked_add(suffix.len() as u64).filter(|n|*n<=max as u64).is_none(){
            return Err(invalid("SYSTEM append offset or bound changed"));
        }
        // SAFETY: O_PATH pin already identifies a private regular file. Open
        // the original, not its path; never request append-at-arbitrary-offset.
        let fd=unsafe{zxs_open_pinned_record(pin.as_raw_fd(),1)};
        if fd<0{return Err(io::Error::last_os_error());}
        let out=unsafe{File::from_raw_fd(fd)};
        if record_stamp(&out,max as u64)?!=expected.stat||mounts::id(&out)?!=mounts::id(&pin)?{
            return Err(invalid("held SYSTEM journal no longer matches its original fence"));
        }
        expected.birth.require(&Birth::read(&out)?)?;
        let mut bytes=suffix;let mut at=offset;
        while !bytes.is_empty(){
            match out.write_at(bytes,at){
                Ok(0)=>return Err(io::Error::new(io::ErrorKind::WriteZero,"short SYSTEM frame append")),
                Ok(n)=>{at+=n as u64;bytes=&bytes[n..];}
                Err(e) if e.kind()==io::ErrorKind::Interrupted=>{},
                Err(e)=>return Err(e),
            }
        }
        // This is required even for an already-visible complete original frame.
        out.sync_all()?;
        self.require_named()
    }
}
