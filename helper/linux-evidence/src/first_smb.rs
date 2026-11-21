//! Debian 13 amd64, fixed Libgcrypt 1.11 ABI. Not a hash or login service.
//! Password-equivalent material is consumed in this process only: the exact
//! native account operation, or one private fixed Rust vault staging inode.
//! No bytes go to SQL, IPC, diagnostics or the console.
#![allow(unsafe_code)]
use core::{ffi::{c_char,c_int,c_uint,c_void,CStr},ptr::{self,NonNull}};
use std::{io,slice,sync::atomic::{AtomicBool,Ordering}};
use zeroize::Zeroizing;

// No DT_NEEDED for either optional provider on the helper executable. A
// receipt query or disk inventory never loads these DSOs, even if they are absent.
// The caller first checks both root-owned fixed originals and the per-site grant.
struct Dso(NonNull<c_void>);
impl Dso{
    // A prior object with either provider SONAME can pre-empt a DT_NEEDED
    // lookup even if a later explicit absolute dlopen returns the right
    // file. One newly exec'd, single-threaded process; no racing plugin loads.
    fn require_absent(soname:&'static CStr)->io::Result<()>{
        // SAFETY: only fixed SONAMEs, NOLOAD cannot introduce a mapping.
        // Do not promote a previous handle or request eager relocations.
        let p=unsafe{libc::dlopen(soname.as_ptr(),libc::RTLD_LOCAL|libc::RTLD_LAZY|libc::RTLD_NOLOAD)};
        if let Some(p)=NonNull::new(p){drop(Self(p));return Err(no());}Ok(())
    }
    fn open(path:&'static CStr)->io::Result<Self>{
        // SAFETY: only the two literal absolute slots in Libraries::load can reach
        // this private operation. Local eager binding; no alternate path, lazy
        // plugin, caller-controlled name, or new global symbol scope.
        let p=unsafe{libc::dlopen(path.as_ptr(),libc::RTLD_LOCAL|libc::RTLD_NOW|libc::RTLD_NODELETE)};
        NonNull::new(p).map(Self).ok_or_else(no)
    }
    fn symbol(&self,name:&'static CStr)->io::Result<*mut c_void>{
        // SAFETY: live dlopen handle and one constant NUL-terminated ABI symbol.
        NonNull::new(unsafe{libc::dlsym(self.0.as_ptr(),name.as_ptr())}).map(NonNull::as_ptr).ok_or_else(no)
    }
}
impl Drop for Dso{fn drop(&mut self){
    // SAFETY: balance exactly one successful dlopen. NODELETE retains upstream
    // process-global initialization until the dedicated single-use process exits.
    unsafe{libc::dlclose(self.0.as_ptr());}
}}
struct Abi{
    _gcrypt:Dso,_support:Dso,
    version:unsafe extern "C" fn(req:*const c_char)->*const c_char,
    control:unsafe extern "C" fn(cmd:c_int,...)->c_uint,
    algo_info:unsafe extern "C" fn(algo:c_int,what:c_int,buffer:*mut c_void,n:*mut usize)->c_uint,
    algo_len:unsafe extern "C" fn(algo:c_int)->c_uint,
    open:unsafe extern "C" fn(out:*mut *mut c_void,algo:c_int,flags:c_uint)->c_uint,
    enabled:unsafe extern "C" fn(handle:*mut c_void,algo:c_int)->c_int,
    secure:unsafe extern "C" fn(handle:*mut c_void)->c_int,
    setkey:unsafe extern "C" fn(handle:*mut c_void,key:*const c_void,n:usize)->c_uint,
    write:unsafe extern "C" fn(handle:*mut c_void,data:*const c_void,n:usize)->(),
    ctl:unsafe extern "C" fn(handle:*mut c_void,cmd:c_int,buffer:*mut c_void,n:usize)->c_uint,
    read:unsafe extern "C" fn(handle:*mut c_void,algo:c_int)->*mut u8,
    ptr_secure:unsafe extern "C" fn(p:*const c_void)->c_int,
    close:unsafe extern "C" fn(handle:*mut c_void)->(),
    support_version:unsafe extern "C" fn(req:*const c_char)->*const c_char,
}
/// Two fixed libraries only, never a general loader. No exported function is
/// called before the parent binds every address to its approved original ELF.
pub struct Libraries(std::rc::Rc<Abi>);
impl Libraries{
    pub fn load()->io::Result<Self>{
        Dso::require_absent(c"libgpg-error.so.0")?;Dso::require_absent(c"libgcrypt.so.20")?;
        let support=Dso::open(c"/usr/lib/local-ops-helper/disk-tools-v1/lib/libgpg-error.so.0")?;
        let gcrypt=Dso::open(c"/usr/lib/local-ops-helper/disk-tools-v1/lib/libgcrypt.so.20")?;
        // The dependency scope of the actual Gcrypt object must resolve the
        // same support entry as the explicit original held support handle.
        if gcrypt.symbol(c"gpgrt_check_version")?!=support.symbol(c"gpgrt_check_version")?{return Err(no());}
        macro_rules! sym{($library:ident,$name:expr,$ty:ty)=>{{
            let p=$library.symbol($name)?;
            // SAFETY: Linux x86_64 POSIX dynamic ABI. Each fixed symbol below
            // has the exact function-pointer signature of the locked header;
            // both DSOs outlive all calls through this Abi (and every Handle).
            unsafe{std::mem::transmute::<*mut c_void,$ty>(p)}
        }}}
        Ok(Self(std::rc::Rc::new(Abi{
            version:sym!(gcrypt,c"gcry_check_version",unsafe extern "C" fn(req:*const c_char)->*const c_char),
            control:sym!(gcrypt,c"gcry_control",unsafe extern "C" fn(cmd:c_int,...)->c_uint),
            algo_info:sym!(gcrypt,c"gcry_md_algo_info",unsafe extern "C" fn(algo:c_int,what:c_int,buffer:*mut c_void,n:*mut usize)->c_uint),
            algo_len:sym!(gcrypt,c"gcry_md_get_algo_dlen",unsafe extern "C" fn(algo:c_int)->c_uint),
            open:sym!(gcrypt,c"gcry_md_open",unsafe extern "C" fn(out:*mut *mut c_void,algo:c_int,flags:c_uint)->c_uint),
            enabled:sym!(gcrypt,c"gcry_md_is_enabled",unsafe extern "C" fn(handle:*mut c_void,algo:c_int)->c_int),
            secure:sym!(gcrypt,c"gcry_md_is_secure",unsafe extern "C" fn(handle:*mut c_void)->c_int),
            setkey:sym!(gcrypt,c"gcry_md_setkey",unsafe extern "C" fn(handle:*mut c_void,key:*const c_void,n:usize)->c_uint),
            write:sym!(gcrypt,c"gcry_md_write",unsafe extern "C" fn(handle:*mut c_void,data:*const c_void,n:usize)->()),
            ctl:sym!(gcrypt,c"gcry_md_ctl",unsafe extern "C" fn(handle:*mut c_void,cmd:c_int,buffer:*mut c_void,n:usize)->c_uint),
            read:sym!(gcrypt,c"gcry_md_read",unsafe extern "C" fn(handle:*mut c_void,algo:c_int)->*mut u8),
            ptr_secure:sym!(gcrypt,c"gcry_is_secure",unsafe extern "C" fn(p:*const c_void)->c_int),
            close:sym!(gcrypt,c"gcry_md_close",unsafe extern "C" fn(handle:*mut c_void)->()),
            support_version:sym!(support,c"gpgrt_check_version",unsafe extern "C" fn(req:*const c_char)->*const c_char),
            _gcrypt:gcrypt,_support:support,
        })))
    }
    pub fn entry_sites(&self)->([usize;13],usize){
        let a=&self.0;
        ([a.version as *const () as usize,a.control as *const () as usize,a.algo_info as *const () as usize,a.algo_len as *const () as usize,a.open as *const () as usize,a.enabled as *const () as usize,a.secure as *const () as usize,a.setkey as *const () as usize,a.write as *const () as usize,a.ctl as *const () as usize,a.read as *const () as usize,a.ptr_secure as *const () as usize,a.close as *const () as usize],a.support_version as *const () as usize)
    }
}
const MD4:c_int=301;
const MD5:c_int=1;
const SECURE:c_uint=1;
const HMAC:c_uint=2;
const FINAL:c_int=5;
const TEST_ALGO:c_int=8;
const INIT_SECMEM:c_int=24;
const FINISHED:c_int=38;
const ANY_INIT:c_int=40;
const FIPS_MODE:c_int=55;
static CLAIMED:AtomicBool=AtomicBool::new(false);
fn no()->io::Error{io::Error::new(io::ErrorKind::Unsupported,"the selected SMB enrollment provider is unavailable or disallowed")}

struct Handle{raw:NonNull<c_void>,algo:c_int,lib:std::rc::Rc<Abi>}
impl Drop for Handle{fn drop(&mut self){
    // SAFETY: the sole owning handle came from successful md_open, is not
    // shared, and the upstream close wipes/deallocates its secure storage.
    unsafe{(self.lib.close)(self.raw.as_ptr());}
}}
impl Handle{
    fn open(lib:&std::rc::Rc<Abi>,algo:c_int,flags:c_uint)->io::Result<Self>{
        let mut p=ptr::null_mut();
        // SAFETY: fixed IDs/flags from the reviewed header, initialized out pointer,
        // no data or requested size for the two standard informational operations.
        if unsafe{(lib.algo_info)(algo,TEST_ALGO,ptr::null_mut(),ptr::null_mut())}!=0
            ||unsafe{(lib.algo_len)(algo)}!=16
            ||unsafe{(lib.open)(&mut p,algo,flags)}!=0{return Err(no());}
        let h=Self{raw:NonNull::new(p).ok_or_else(no)?,algo,lib:lib.clone()};
        // SAFETY: the live unique handle was created above. Do not fall back to pageable crypto memory.
        if unsafe{(lib.enabled)(p,algo)}!=1||unsafe{(lib.secure)(p)}!=1{return Err(no());}Ok(h)
    }
    fn complete(&mut self,bytes:&[u8])->io::Result<&[u8;16]>{
        // SAFETY: byte slice stays valid during the synchronous update. The fixed
        // FINALIZE operation takes no output buffer and reports any finalization error.
        unsafe{(self.lib.write)(self.raw.as_ptr(),bytes.as_ptr().cast(),bytes.len());}
        if unsafe{(self.lib.ctl)(self.raw.as_ptr(),FINAL,ptr::null_mut(),0)}!=0{return Err(no());}
        self.value()
    }
    fn value(&mut self)->io::Result<&[u8;16]>{
        // SAFETY: only finalized single-algorithm handles with checked 16-byte
        // output are read. The pointer belongs to this live handle and is used
        // before the exclusive borrow ends. No return if the pool is not secure.
        let p=unsafe{(self.lib.read)(self.raw.as_ptr(),self.algo)};
        if p.is_null()||unsafe{(self.lib.ptr_secure)(p.cast())}==0{return Err(no());}
        unsafe{slice::from_raw_parts(p,16)}.try_into().map_err(|_|no())
    }
}
fn versions(lib:&Abi)->bool{
    // SAFETY: NULL requests the library-owned static runtime version C string.
    let(a,b)=unsafe{((lib.version)(ptr::null()),(lib.support_version)(ptr::null()))};
    !a.is_null()&&!b.is_null()&&unsafe{CStr::from_ptr(a)}==c"1.11.0"&&unsafe{CStr::from_ptr(b)}==c"1.51"
}
fn permitted(lib:&Abi)->io::Result<()>{
    // SAFETY: fixed read-only query, no pointers. Never force or disable FIPS or enable a legacy mode.
    if unsafe{(lib.control)(FIPS_MODE,0i32)}==0{Ok(())}else{Err(no())}
}
/// One fresh non-root credential process only; the caller must first enforce
/// host enrollment policy, single-thread identity, mlockall and the approved exact provider.
/// A failure consumes this single attempt; do not switch libraries or restart it.
pub struct Enrollment(Handle);
impl Libraries{
    pub fn qualify(&self)->io::Result<Enrollment>{
        let lib=&self.0;
        if CLAIMED.swap(true,Ordering::AcqRel){return Err(no());}
        // SAFETY: ANY_INIT is the upstream non-initializing predicate (global.c).
        // Ask it before version_check initializes the selected library; no
        // input yet. We decline somebody else's prior global mutable state.
        if unsafe{(lib.control)(ANY_INIT,0i32)}!=0||!versions(lib){return Err(no());}
        permitted(lib)?;
        // SAFETY: INIT_SECMEM takes one unsigned int size; fail if locking was
        // not verified by the provider. No warning suppression or auto-expansion.
        if unsafe{(lib.control)(INIT_SECMEM,32768u32,0i32)}!=0||unsafe{(lib.control)(FINISHED,0i32)}!=0{return Err(no());}
        permitted(lib)?;
        // Two *public*, fixed known-answer samples before a person enters any
        // secret. The HMAC is a capability check, not a challenge/response API.
        let mut test=Handle::open(lib,MD4,SECURE)?;
        if *test.complete(b"a")?!=[0xbd,0xe5,0x2c,0xb3,0x1d,0xe3,0x3e,0x46,0x24,0x5e,0x05,0xfb,0xdb,0xd6,0xfb,0x24]{return Err(no());}
        drop(test);
        let mut mac=Handle::open(lib,MD5,SECURE|HMAC)?;let key=[0x0bu8;16];
        // SAFETY: exactly this public RFC 2202 fixture; the key lives through setkey.
        if unsafe{(lib.setkey)(mac.raw.as_ptr(),key.as_ptr().cast(),key.len())}!=0||
            *mac.complete(b"Hi There")?!=[0x92,0x94,0x72,0x7a,0x36,0x38,0xbb,0x1c,0x13,0xf4,0x8e,0xf8,0x15,0x8b,0xfc,0x9d]{return Err(no());}
        drop(mac);Ok(Enrollment(Handle::open(lib,MD4,SECURE)?))
    }
}
const RUST_PREFIX:usize=224;
const RUST_LEN:u64=240;
const RUST_AUTH_DOMAIN:&[u8]=b"ZXSLOCAL";
const RUST_V2:&[u8;16]=b"ZXS-RUST-SMB-V2\0";

fn rust_login(prefix:&[u8;RUST_PREFIX])->io::Result<&[u8]>{
    let mut coords=prefix[24..152].chunks_exact(16);
    let generation=u64::from_be_bytes(prefix[152..160].try_into().expect("fixed generation"));
    let n=usize::from(prefix[17]);
    let init=&prefix[40..56];let setup=&prefix[56..72];let current=&prefix[120..136];
    if &prefix[..16]!=RUST_V2||prefix[16]!=1||prefix[18..24]!=[0;6]||
        coords.any(|id|id==[0;16])||generation==0||generation>i64::MAX as u64||!(1..=64).contains(&n)||
        init==setup||current==init||(generation==1)!=(current==setup){
        return Err(no());
    }
    let name=&prefix[160..160+n];
    let alnum=|b:u8|b.is_ascii_lowercase()||b.is_ascii_digit();
    if !alnum(name[0])||!name.iter().all(|&b|alnum(b)||matches!(b,b'.'|b'_'|b'-'))||
        matches!(name,b"guest"|b"anonymous"|b"root"|b"nobody")||
        prefix[160+n..].iter().any(|&b|b!=0){return Err(no());}
    Ok(name)
}

/// The v2 schema has one configured account, in the same age-bound fixed
/// position. Accept the grammatically exact public prefix, not a grant.
#[doc(hidden)]
pub fn require_rust_v2_public_prefix(prefix:&[u8;RUST_PREFIX])->io::Result<()>{
    rust_login(prefix)?;
    let install=prefix[24..104].chunks_exact(16).collect::<Vec<_>>();
    if install.iter().enumerate().any(|(i,id)|install[..i].contains(id))
        ||prefix[152..160]!=1u64.to_be_bytes()&&install.contains(&&prefix[120..136]){
        return Err(no());
    }
    Ok(())
}
type WriteStamp=(u64,u64,u64);
fn stage_stamp(file:&std::fs::File)->io::Result<WriteStamp>{
    use std::os::unix::fs::MetadataExt;
    let m=file.metadata()?;
    if !m.is_file()||m.uid()!=0||m.gid()!=0||m.nlink()!=1||m.mode()&0o7777!=0o600{return Err(no());}
    Ok((m.dev(),m.ino(),m.len()))
}
#[derive(Eq,PartialEq)]
struct BlankStage{stamp:WriteStamp,changed:[i64;4]}
impl BlankStage{
    fn require(stage:&mut std::fs::File)->io::Result<Self>{
        use std::{io::Seek,os::unix::fs::MetadataExt};
        use rustix::fs::{OFlags,fcntl_getfl};
        let flags=fcntl_getfl(&*stage)?;
        let stamp=stage_stamp(stage)?;
        let m=stage.metadata()?;
        if flags.intersects(OFlags::PATH|OFlags::APPEND)||flags&OFlags::ACCMODE!=OFlags::WRONLY||
            stamp.2!=0||stage.stream_position()?!=0||stage_stamp(stage)?!=stamp{
            return Err(no());
        }
        Ok(Self{stamp,changed:[m.ctime(),m.ctime_nsec(),m.mtime(),m.mtime_nsec()]})
    }
}
/// Own the chosen provider with a borrow of this exact original blank
/// stage. Rechecking after input cannot select a different file or grant.
pub struct RustV2Stage<'a>{enrollment:Enrollment,prefix:&'a [u8;RUST_PREFIX],stage:&'a mut std::fs::File,original:BlankStage}
impl RustV2Stage<'_>{
    #[doc(hidden)]
    pub fn seal(self,input:&[u8])->io::Result<()>{
        if BlankStage::require(self.stage)?!=self.original{return Err(no());}
        self.enrollment.seal_record(input,self.prefix,self.stage)
    }
}
impl Enrollment {
    /// Fixed new-profile stage qualification *before* requesting a secret.
    /// The high caller has already proved the exact name/current grant;
    /// we only receive that actual file, never a vault name or root FD.
    #[doc(hidden)]
    pub fn prepare_rust_v2<'s>(self,prefix:&'s [u8;RUST_PREFIX],stage:&'s mut std::fs::File)->io::Result<RustV2Stage<'s>>{
        require_rust_v2_public_prefix(prefix)?;
        let original=BlankStage::require(stage)?;
        Ok(RustV2Stage{enrollment:self,prefix,stage,original})
    }

    fn seal_record(mut self,input:&[u8],prefix:&[u8;RUST_PREFIX],stage:&mut std::fs::File)->io::Result<()>{
        use std::io::{Seek,Write};
        use rustix::fs::{OFlags,fcntl_getfl};

        let login=rust_login(prefix)?;
        let flags=fcntl_getfl(&*stage)?;
        let (dev,ino,initial)=stage_stamp(stage)?;
        if initial!=0||stage.stream_position()?!=0||flags.contains(OFlags::APPEND)||
            !matches!(flags&OFlags::ACCMODE,OFlags::WRONLY|OFlags::RDWR){return Err(no());}
        let mut identity=[0u8;144]; // uppercase canonical login + exact enrolled domain, UTF-16LE
        for (n,b) in login.iter().map(u8::to_ascii_uppercase).chain(RUST_AUTH_DOMAIN.iter().copied()).enumerate(){
            identity[n*2]=b;
        }
        permitted(&self.0.lib)?;
        let mut mac=Handle::open(&self.0.lib,MD5,SECURE|HMAC)?;
        {
            let mut pass=Zeroizing::new([0u8;2048]);
            let n=utf16le(input,&mut pass)?;
            let nt=self.0.complete(&pass[..n])?;
            if *nt==[0;16]||*nt==EMPTY{return Err(no());}
            // SAFETY: nt points to the first still-live secure handle's
            // fixed 16 bytes, used synchronously only to key the second one.
            if unsafe{(mac.lib.setkey)(mac.raw.as_ptr(),nt.as_ptr().cast(),nt.len())}!=0{return Err(no());}
        }
        drop(self);
        permitted(&mac.lib)?;
        let key=mac.complete(&identity[..2*(login.len()+RUST_AUTH_DOMAIN.len())])?;
        if *key==[0;16]||stage_stamp(stage)?!=(dev,ino,0)||stage.stream_position()?!=0{return Err(no());}
        stage.write_all(prefix)?;
        stage.write_all(key)?;
        drop(mac);
        if stage_stamp(stage)?!=(dev,ino,RUST_LEN)||stage.stream_position()?!=RUST_LEN{return Err(no());}
        stage.sync_all()?;
        if stage_stamp(stage)?!=(dev,ino,RUST_LEN){return Err(no());}
        Ok(())
    }
}
const EMPTY:[u8;16]=[0x31,0xd6,0xcf,0xe0,0xd1,0x6a,0xe9,0x31,0xb7,0x3c,0x59,0xd7,0xe0,0xc0,0x89,0xc0];
fn utf16le(input:&[u8],out:&mut [u8;2048])->io::Result<usize>{
    if !(1..=1024).contains(&input.len())||input.contains(&0){return Err(no());}
    let s=std::str::from_utf8(input).map_err(|_|no())?;let mut used=0;
    for unit in s.encode_utf16(){out[used..used+2].copy_from_slice(&unit.to_le_bytes());used+=2;}Ok(used)
}
/// No byte accessor, formatter, clone, comparison, serde or generic hash API.
/// Only the selected transaction's initial, non-overwriting SMB slot is bound.
#[cfg(test)]mod tests{
    use super::*;
    // The target glibc is already resident in this executable. Repeated
    // NOLOAD probes must reject and release that reference without promotion.
    #[test]fn a_preexisting_provider_name_cannot_be_adopted_by_the_fresh_loader(){
        assert!(Dso::require_absent(c"libc.so.6").is_err());
        assert!(Dso::require_absent(c"libc.so.6").is_err());
    }
    #[test]fn exact_password_input_is_strict_utf8_and_verbatim_utf16le_without_a_terminator(){
        let mut out=[0xa5;2048];let n=utf16le(" A💠é".as_bytes(),&mut out).unwrap();
        assert_eq!(&out[..n],&[0x20,0,0x41,0,0x3d,0xd8,0xa0,0xdc,0xe9,0]);assert_eq!(out[n],0xa5);
        assert_eq!(utf16le(&[b'a';1024],&mut out).unwrap(),2048);
        for wrong in [b"".as_slice(),&[0xff],&[0xed,0xa0,0x80],&[b'a',0],&[b'a';1025]]{assert!(utf16le(wrong,&mut out).is_err());}
    }
}
