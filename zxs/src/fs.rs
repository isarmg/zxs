//! Bounded directory operations through retained Linux descriptors. The C
//! shim only translates Linux ABI flags, xattrs and the directory-entry ABI.
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::raw::{c_char, c_int, c_uint};
use std::os::unix::{ffi::OsStrExt, fs::{MetadataExt, PermissionsExt}};
use std::path::{Component, Path, PathBuf};

mod mounts;
mod relative;
mod frontier;
pub(crate) mod stream;
pub(crate) use frontier::{frontier,initial_frontier,initial_attempt_binding,Frontier};
mod birth;
pub(crate) use birth::{Birth,MAX_BIRTH};

unsafe extern "C" {
    fn zxs_open_directory(parent: c_int, name: *const c_char) -> c_int;
    fn zxs_directory_has_only(fd: c_int, allowed: *const c_char) -> c_int;
    fn zxs_read_label(fd: c_int, bytes: *mut u8, len: usize) -> isize;
    fn zxs_claim(fd: c_int, bytes: *const u8, len: usize) -> c_int;
    fn zxs_pin_record(fd: c_int, name: *const c_char) -> c_int;
    fn zxs_open_pinned_record(pin:c_int,write:c_int)->c_int;
    fn zxs_mkdir_private(fd:c_int,name:*const c_char)->c_int;
    fn zxs_remove_name(fd:c_int,name:*const c_char)->c_int;
    fn zxs_put_material_token(fd:c_int,value:*const u8,n:usize)->c_int;
    fn zxs_get_material_token(fd:c_int,value:*mut u8,n:usize)->isize;
    fn zxs_root_names(fd:c_int,out:*mut u8,cap:usize)->isize;
    fn zxs_anonymous_file(fd: c_int) -> c_int;
    fn zxs_link_anonymous(file: c_int, dir: c_int, name: *const c_char) -> c_int;
    fn zxs_rename_fresh(from: c_int, old: *const c_char, to: c_int, next: *const c_char) -> c_int;
    fn zxs_volume_label_present(fd: c_int) -> c_int;
    fn zxs_effective_uid() -> c_uint;
}

pub(crate) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Entity { pub device: u64, pub inode: u64 }

fn entity(file: &File) -> io::Result<Entity> {
    let meta = file.metadata()?;
    if !meta.is_dir() || meta.ino() == 0 {
        return Err(invalid("storage identity is not a bound directory"));
    }
    Ok(Entity { device: meta.dev(), inode: meta.ino() })
}

fn child(parent: &File, component: &CStr) -> io::Result<File> {
    // SAFETY: the borrowed descriptor and NUL-terminated component live for
    // the call. On success openat transferred a fresh descriptor to us.
    let fd = unsafe { zxs_open_directory(parent.as_raw_fd(), component.as_ptr()) };
    if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { File::from_raw_fd(fd) }) }
}

pub(crate) fn local_uid()->u32 {
    // SAFETY: read the real process-effective credential; never accept a UID from a client.
    unsafe { zxs_effective_uid() }
}

pub(crate) fn check_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute() || path.as_os_str().as_bytes().len() > 4096 {
        return Err(invalid("supply a bounded absolute directory path"));
    }
    let mut normal = PathBuf::from("/");
    let mut depth = 0;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                depth += 1;
                if depth > 256 || name.as_bytes().contains(&0) { return Err(invalid("invalid directory component or depth")); }
                normal.push(name);
            }
            _ => return Err(invalid("relative or parent components are not allowed")),
        }
    }
    if depth == 0 || normal.as_os_str().as_bytes() != path.as_os_str().as_bytes() {
        return Err(invalid("use an exact absolute directory path, not / or an alias spelling"));
    }
    Ok(())
}

fn open_absolute(path: &Path) -> io::Result<File> {
    check_path(path)?;
    let mut current=File::open("/")?;
    for component in path.components() {
        if let Component::Normal(name)=component {
            let c=CString::new(name.as_bytes()).map_err(|_|invalid("NUL in directory path"))?;
            current=child(&current,&c)?;
        }
    }
    Ok(current)
}

pub(crate) struct HeldDir {
    pub(crate) file: File,
    pub(crate) path: PathBuf,
    pub(crate) entity: Entity,
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Location {entity:Entity,mount:u64}
impl Location {
    fn held(file:&File)->io::Result<Self>{Ok(Self{entity:entity(file)?,mount:mounts::id(file)?})}
}
fn lineage_end(here:Location,parent:Location,namespace_root:Location)->io::Result<bool>{
    if here.entity!=parent.entity{return Ok(false);}
    if here!=parent || here!=namespace_root {
        return Err(invalid("repeated directory entity is not the same held namespace root"));
    }
    Ok(true)
}

impl HeldDir {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let file = open_absolute(path)?;
        let entity = entity(&file)?;
        let meta = file.metadata()?;
        // SAFETY: geteuid takes no arguments and returns the process credential.
        if meta.uid() != unsafe { zxs_effective_uid() } || meta.mode() & 0o022 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                "selected directory must belong to this account and not allow group/other writes"));
        }
        Ok(Self { file, path: path.to_owned(), entity })
    }

    pub(crate) fn require_named(&self) -> io::Result<()> {
        let current = Self::open(&self.path)?;
        if current.entity != self.entity || mounts::id(&current.file)? != mounts::id(&self.file)? {
            return Err(invalid("selected directory or its named mount was replaced"));
        }
        Ok(())
    }

    pub(crate) fn share(&self) -> io::Result<()> {
        self.file.try_lock_shared().map_err(io::Error::from)
    }

    pub(crate) fn require_fresh(&self) -> io::Result<()> {
        // SAFETY: both calls receive only our still-held read-only directory.
        let empty = unsafe { zxs_directory_has_only(self.file.as_raw_fd(), std::ptr::null()) };
        if empty < 0 { return Err(io::Error::last_os_error()); }
        if empty != 1 { return Err(invalid("selected directory is not empty")); }
        let label = unsafe { zxs_volume_label_present(self.file.as_raw_fd()) };
        if label < 0 { return Err(io::Error::last_os_error()); }
        if label != 0 { return Err(invalid("selected directory already carries a volume identity")); }
        Ok(())
    }

    pub(crate) fn reject_registered_ancestors(&self, selected: &std::collections::BTreeSet<Entity>) -> io::Result<()> {
        self.check_ancestors(selected,&mounts::Mounts::read()?)
    }

    fn check_ancestors(&self,selected:&std::collections::BTreeSet<Entity>,mounted:&mounts::Mounts)->io::Result<()>{
        let root=File::open("/")?;
        mounted.complete(&root)?;
        let namespace_root=Location::held(&root)?;
        let mut current = self.file.try_clone()?;
        mounted.complete(&current)?;
        let mut prior=Location::held(&current)?;
        for _ in 0..512 {
            let parent = child(&current, c"..")?;
            mounted.complete(&parent)?;
            let id=Location::held(&parent)?;
            if lineage_end(prior,id,namespace_root)?{return Ok(());}
            if selected.contains(&id.entity) { return Err(invalid("selected storage roots overlap")); }
            // A registered but unselected parent also owns its subtree.
            // SAFETY: parent is a still-held directory, and this is a read only call.
            let marked = unsafe { zxs_volume_label_present(parent.as_raw_fd()) };
            if marked < 0 { return Err(io::Error::last_os_error()); }
            if marked != 0 { return Err(invalid("ancestor already belongs to a storage registration")); }
            current = parent;
            prior = id;
        }
        Err(invalid("cannot establish physical root ancestry"))
    }
}

fn os_result(rc: c_int) -> io::Result<()> {
    if rc < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileStamp {
    dev: u64, ino: u64, len: u64, ctime: (i64,i64), mtime: (i64,i64),
}
impl FileStamp { pub(crate) fn entity(self)->Entity {Entity{device:self.dev,inode:self.ino}} pub(crate) fn size(self)->u64{self.len} }
#[derive(Clone,Debug,Eq,PartialEq)]
pub(crate) struct OriginalStamp { pub(crate) stat:FileStamp,pub(crate) birth:Birth }
fn record_stamp(file: &File, max: u64) -> io::Result<FileStamp> {
    let m=file.metadata()?;
    // SAFETY: this simply reads our effective UID.
    if !m.is_file() || m.nlink()!=1 || m.ino()==0 || m.uid()!=local_uid()
        || m.mode()&0o7777!=0o600 || m.len()>max {
        return Err(invalid("SYSTEM record is not an owned, bounded private regular file"));
    }
    Ok(FileStamp{dev:m.dev(),ino:m.ino(),len:m.len(),ctime:(m.ctime(),m.ctime_nsec()),mtime:(m.mtime(),m.mtime_nsec())})
}

impl HeldDir {
    /// Same open directory, not a fresh lookup; retains the existing flock.
    pub(crate) fn duplicate(&self)->io::Result<Self>{
        Ok(Self{file:self.file.try_clone()?,path:self.path.clone(),entity:self.entity})
    }
    pub(crate) fn exclusive(&self) -> io::Result<()> {
        self.file.try_lock().map_err(io::Error::from)
    }
    pub(crate) fn only(&self, name: Option<&CStr>) -> io::Result<()> {
        // SAFETY: the optional allowed name is NUL terminated for this call.
        let rc=unsafe { zxs_directory_has_only(self.file.as_raw_fd(),name.map_or(std::ptr::null(),CStr::as_ptr)) };
        if rc<0 {Err(io::Error::last_os_error())} else if rc==0 {Err(invalid("unexpected existing contents at registration boundary"))} else {Ok(())}
    }
    pub(crate) fn label(&self) -> io::Result<Option<Vec<u8>>> {
        let mut out=[0u8;128];
        // SAFETY: the kernel sees exactly our live output buffer.
        let n=unsafe{zxs_read_label(self.file.as_raw_fd(),out.as_mut_ptr(),out.len())};
        match n {
            -2=>Ok(None), -1=>Err(io::Error::last_os_error()),
            v if (0..=out.len() as isize).contains(&v)=>Ok(Some(out[..v as usize].to_vec())),
            _=>Err(invalid("invalid volume label length")),
        }
    }
    pub(crate) fn claim(&self, label:&[u8]) -> io::Result<()> {
        // SAFETY: buffer and FD live for the call. XATTR_CREATE never replaces
        // any prior identity. The original caller also verified every root.
        os_result(unsafe{zxs_claim(self.file.as_raw_fd(),label.as_ptr(),label.len())})?;
        self.file.sync_all()
    }
    fn pin_record(&self,name:&CStr)->io::Result<Option<File>>{
        // SAFETY: O_PATH|O_NOFOLLOW pins an entry without activating a device,
        // opening a FIFO or following a symlink at this untrusted boundary.
        let fd=unsafe{zxs_pin_record(self.file.as_raw_fd(),name.as_ptr())};
        if fd<0{
            let e=io::Error::last_os_error();
            return if e.kind()==io::ErrorKind::NotFound {Ok(None)}else{Err(e)};
        }
        Ok(Some(unsafe{File::from_raw_fd(fd)}))
    }
    pub(crate) fn record(&self,name:&CStr,max:usize)->io::Result<Option<(OriginalStamp,Vec<u8>)>>{
        let Some(pin)=self.pin_record(name)? else{return Ok(None)};
        let stamp=record_stamp(&pin,max as u64)?;
        let mount=mounts::id(&self.file)?;
        if stamp.dev!=self.entity.device||mounts::id(&pin)?!=mount{return Err(invalid("foreign SYSTEM record mount"));}
        // SAFETY: pin is already checked private, regular and on the held mount.
        // Reopen its proc FD, not the name examined earlier.
        let fd=unsafe{zxs_open_pinned_record(pin.as_raw_fd(),0)};
        if fd<0{return Err(io::Error::last_os_error());}
        let mut f=unsafe{File::from_raw_fd(fd)};
        if record_stamp(&f,max as u64)?!=stamp||mounts::id(&f)?!=mount{return Err(invalid("pinned SYSTEM object changed"));}
        let birth=Birth::read(&f)?;
        let mut bytes=Vec::with_capacity(stamp.len as usize);
        (&mut f).take(max as u64+1).read_to_end(&mut bytes)?;
        let named=self.pin_record(name)?.ok_or_else(||invalid("named SYSTEM record disappeared"))?;
        if bytes.len() as u64!=stamp.len || record_stamp(&f,max as u64)?!=stamp || record_stamp(&named,max as u64)?!=stamp || mounts::id(&named)?!=mount {
            return Err(invalid("SYSTEM record changed during read"));
        }
        Ok(Some((OriginalStamp{stat:stamp,birth},bytes)))
    }
    pub(crate) fn anonymous_record(&self)->io::Result<(File,Entity)>{
        // SAFETY: returns a fresh anonymous inode in this writable directory.
        let fd=unsafe{zxs_anonymous_file(self.file.as_raw_fd())};
        if fd<0{return Err(io::Error::last_os_error());}
        let temp=unsafe{File::from_raw_fd(fd)};
        temp.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        let m=temp.metadata()?;
        if !m.is_file()||m.nlink()!=0||m.ino()==0||m.dev()!=self.entity.device||m.uid()!=local_uid()||m.mode()&0o077!=0 {
            return Err(invalid("unnamed SYSTEM reservation is not a private inode on the selected volume"));
        }
        Ok((temp,Entity{device:m.dev(),inode:m.ino()}))
    }
    pub(crate) fn add_record(&self,mut temp:File,name:&CStr,bytes:&[u8])->io::Result<()>{
        temp.write_all(bytes)?;temp.sync_all()?;
        self.require_named()?;
        // SAFETY: both descriptors and the fixed single name remain alive.
        os_result(unsafe{zxs_link_anonymous(temp.as_raw_fd(),self.file.as_raw_fd(),name.as_ptr())})?;
        self.file.sync_all()
    }
    pub(crate) fn rename_registration(&self,old:&CStr,new:&CStr)->io::Result<()>{
        self.require_named()?;
        // SAFETY: source/destination are in the same retained SYSTEM directory;
        // kernel RENAME_NOREPLACE never overwrites an existing registration.
        os_result(unsafe{zxs_rename_fresh(self.file.as_raw_fd(),old.as_ptr(),self.file.as_raw_fd(),new.as_ptr())})
    }
}

pub(crate) fn require_selected_lineages(roots:&[(crate::Role,HeldDir)])->io::Result<()>{
    let selected=roots.iter().map(|(_,d)|d.entity).collect();
    let mounts=mounts::Mounts::read()?;
    for (_,d) in roots {d.require_named()?; d.check_ancestors(&selected,&mounts)?;}
    Ok(())
}

/// The token belongs only to the original physical preparation. It is not
/// portable FILE history or a second custody decision.
pub(crate) fn set_preparation_token(f:&File,token:&[u8;32])->io::Result<()>{
    // SAFETY: called on our still-anonymous new material; key is create-only.
    os_result(unsafe{zxs_put_material_token(f.as_raw_fd(),token.as_ptr(),token.len())})
}

pub(crate) fn require_material_token(file:&File,token:&[u8;32])->io::Result<()>{
    let mut bytes=[0;32];
    // SAFETY: called on the admitted original private regular FD, fixed key.
    let n=unsafe{zxs_get_material_token(file.as_raw_fd(),bytes.as_mut_ptr(),bytes.len())};
    if n<0{return Err(io::Error::last_os_error());}
    if n!=32||&bytes!=token{return Err(invalid("working file does not carry its original preparation token"));}
    Ok(())
}
