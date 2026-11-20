//! Fixed C ABI to the offline private native role. Application code has no
//! generic dlsym, password/NT accessor, or arbitrary command/filename here.
#![allow(unsafe_code)]
use core::{ffi::{c_int,c_void,CStr},ptr::NonNull};
use std::{ffi::CString,fs::File,io,os::fd::{AsRawFd,FromRawFd}};

type Domain=unsafe extern "C" fn(c_int,c_int,c_int,u32,u32,*const u8)->bool;
type Check=unsafe extern "C" fn(c_int,c_int,c_int,*const Expected)->c_int;
type Confirm=unsafe extern "C" fn(c_int,c_int,c_int,*const Expected)->bool;
#[repr(C)]
struct LinkMap { addr:usize, name:*const libc::c_char, dynamic:*mut c_void, next:*mut LinkMap, previous:*mut LinkMap }
type Change=unsafe extern "C" fn(c_int,c_int,c_int,*const Expected,*const u8,*mut c_int)->c_int;
pub const PATH:&str="/usr/lib/local-ops-helper/account-native-v7/helper-account-native.so";

#[repr(C)]
pub struct Expected {
    login_len:usize, login:[u8;64], target:[u8;154], has_previous:bool, previous:[u8;154],
}
impl Expected {
    pub fn for_marker(name:&[u8],previous:Option<[u8;154]>,target:[u8;154])->io::Result<Self>{
        if !(1..=64).contains(&name.len())||name.contains(&0){return Err(refused());}
        let mut login=[0;64];login[..name.len()].copy_from_slice(name);
        Ok(Self{login_len:name.len(),login,target,has_previous:previous.is_some(),previous:previous.unwrap_or([0;154])})
    }
}
#[cfg(target_arch="x86_64")]
const _:()=assert!(std::mem::size_of::<Expected>()==384
    &&std::mem::offset_of!(Expected, target)==72&&std::mem::offset_of!(Expected, previous)==227);
fn refused()->io::Error{io::Error::other("protected native Samba account operation is unavailable or unconfirmed")}

pub struct Library {
    handle:NonNull<c_void>,domain:Domain,check:Check,confirm:Confirm,change:Change,
}
impl Drop for Library {
    fn drop(&mut self){
        // SAFETY: this owns exactly the successful isolated loader reference.
        unsafe{libc::dlclose(self.handle.as_ptr());}
    }
}
impl Library {
    /// This call is made ONLY after the entire independent issuer-approved
    /// absolute DT_NEEDED closure and exact build slot were positively
    /// checked. An isolated symbol namespace is NOT a sandbox for ELF
    /// constructors. Missing or unverified signed objects forbid loading.
    pub fn load()->io::Result<Self>{
        let path=CString::new(PATH).expect("fixed offline artifact");
        // SAFETY: only this fixed, closed release, a new glibc link-map
        // namespace (no process-global TDB ABI interposition) and no
        // caller-supplied name or flag is used. No Rust allocator state
        // crosses the C ABI. All objects live until our wrapper is dropped.
        let handle=NonNull::new(unsafe{libc::dlmopen(libc::LM_ID_NEWLM,path.as_ptr(),libc::RTLD_LOCAL|libc::RTLD_NOW)})
            .ok_or_else(refused)?;
        let find=|n:&'static CStr|NonNull::new(unsafe{libc::dlsym(handle.as_ptr(),n.as_ptr())}).ok_or_else(refused);
        let loaded=(|| {
            // SAFETY: these four concrete declarations are from the audited
            // private header; the parent verifies their actual mapped origin.
            Ok(unsafe{Self{handle,
                domain:std::mem::transmute::<*mut c_void,Domain>(find(c"helper_native_machine_domain")?.as_ptr()),
                check:std::mem::transmute::<*mut c_void,Check>(find(c"helper_native_predecessor")?.as_ptr()),
                confirm:std::mem::transmute::<*mut c_void,Confirm>(find(c"helper_native_confirm_target")?.as_ptr()),
                change:std::mem::transmute::<*mut c_void,Change>(find(c"helper_native_change")?.as_ptr()),
            }})
        })();
        if loaded.is_err(){
            // SAFETY: no owning Library was returned.
            unsafe{libc::dlclose(handle.as_ptr());}
        }
        loaded
    }
    pub fn linked_paths(&self)->io::Result<Vec<(Vec<u8>,usize)>>{
        let mut head:*mut LinkMap=std::ptr::null_mut();
        // SAFETY: glibc's public link_map prefix only; dlinfo fills the
        // exact pointer for the live private namespace; no concurrent loader.
        let rc=unsafe{libc::dlinfo(self.handle.as_ptr(),libc::RTLD_DI_LINKMAP,(&raw mut head).cast())};
        if rc!=0||head.is_null()||!unsafe{(*head).previous}.is_null(){return Err(refused());}
        let mut paths=Vec::new();let mut current=head;
        while let Some(map)=NonNull::new(current){
            if paths.len()>=128{return Err(refused());}
            // SAFETY: each node belongs to that same live glibc map; it
            // supplies a loader-owned, NUL-terminated pathname.
            let entry=unsafe{map.as_ref()};if entry.name.is_null()||entry.dynamic.is_null(){return Err(refused());}
            let n=unsafe{libc::strnlen(entry.name,512)};
            if n==0||n>=512{return Err(refused());}
            paths.push((unsafe{std::slice::from_raw_parts(entry.name.cast::<u8>(),n)}.to_vec(),entry.dynamic as usize));
            current=entry.next;
        }Ok(paths)
    }
    pub fn entry_sites(&self)->[usize;4]{[self.domain as usize,self.check as usize,self.confirm as usize,self.change as usize]}
    pub fn domain(&self,site:&File,dir:&File,store:&File,uid:u32,gid:u32,public_sid:&[u8;24])->io::Result<()>{
        // SAFETY: borrowed fixed original pins and precisely 24 public bytes.
        if unsafe{(self.domain)(site.as_raw_fd(),dir.as_raw_fd(),store.as_raw_fd(),uid,gid,public_sid.as_ptr())}
        {Ok(())}else{Err(refused())}
    }
    pub fn inspect(&self,site:&File,parent:&File,store:&File,expected:&Expected)->io::Result<bool>{
        // SAFETY: Rust representation matches the C header; this cannot ask
        // the ordinary mounted, already-selected TDB to perform a write.
        match unsafe{(self.check)(site.as_raw_fd(),parent.as_raw_fd(),store.as_raw_fd(),expected)}{
            1=>Ok(false),2=>Ok(true),_=>Err(refused()),
        }
    }
    pub fn sync_target(&self,site:&File,parent:&File,store:&File,expected:&Expected)->io::Result<()>{
        // SAFETY: valid fixed struct, actual pins borrowed during all I/O.
        if unsafe{(self.confirm)(site.as_raw_fd(),parent.as_raw_fd(),store.as_raw_fd(),expected)}
        {Ok(())}else{Err(refused())}
    }
    pub(super) fn change(&self,site:&File,parent:&File,previous:Option<&File>,expected:&Expected,nt:&[u8;16])->io::Result<File>{
        let mut out=-1;
        // SAFETY: Only the same-process private crypto handle calls this,
        // borrowing exactly 16 secret bytes; no result or message copies them.
        let result=unsafe{(self.change)(site.as_raw_fd(),parent.as_raw_fd(),
            previous.map_or(-1,AsRawFd::as_raw_fd),expected,nt.as_ptr(),&mut out)};
        if result!=0||out<3{return Err(refused());}
        // SAFETY: audited C constructs one new CLOEXEC O_PATH, returns its
        // ownership only on success, and closes it itself on every failure.
        Ok(unsafe{File::from_raw_fd(out)})
    }
}
