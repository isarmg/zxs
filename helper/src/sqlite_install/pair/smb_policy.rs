//! Exact bundled SMB cryptographic provider and host FIPS checks shared by the native maintenance gate.
use crate::{fs_tools::RuntimeFile,maintenance::thread_file};
use helper_linux_evidence::first_smb;
use rustix::fs::{major,minor};
use std::{fs::File,io::{self,Read},os::unix::fs::MetadataExt};

const LIB_ROOT:&str="/usr/lib/local-ops-helper/disk-tools-v1/lib/";
const MAX_MAPS:usize=256<<10;
fn no()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,
    "no current site permission for the exact installed NTLMv2 enrollment provider and instance")}

pub(super) fn mode(f:&File,want:u32)->io::Result<()>{
    let m=f.metadata()?;if m.mode()!=(0o100_000|want)||m.uid()!=0||m.gid()!=0||m.nlink()!=1{Err(no())}else{Ok(())}
}
pub(super) fn release(slot:&'static [&'static str],perm:u32,pin:&[u8;32])->io::Result<RuntimeFile>{
    let mut f=RuntimeFile::open(slot,perm==0o755,64<<20)?;mode(&f.file,perm)?;
    if f.digest()?!=*pin{return Err(no());}Ok(f)
}
pub(super) fn clean_environment()->bool{
    use std::os::unix::ffi::OsStrExt;
    std::env::vars_os().all(|(k,_)|{let k=k.as_bytes();
        !(k==b"GLIBC_TUNABLES"||[b"LD_".as_slice(),b"LIBGCRYPT_",b"GCRYPT_",b"MALLOC_"].iter().any(|p|k.starts_with(p)))})
}
// Do not interpret an unreadable kernel control as 0. Kernels not exposing
// this optional flag still need both explicit site authorization and a genuinely
// non-FIPS provider. Respect the upstream administrator force-file as well.
pub(super) fn system_policy()->io::Result<()>{
    match std::fs::symlink_metadata("/etc/gcrypt/fips_enabled"){
        Ok(_)=>return Err(no()),Err(e) if e.kind()==io::ErrorKind::NotFound=>{},Err(_)=>return Err(no()),
    }
    match thread_file(c"sys/crypto/fips_enabled"){
        Ok(f)=>{let mut v=Vec::new();f.take(4).read_to_end(&mut v)?;if v!=b"0\n"{return Err(no());}}
        Err(e) if e.kind()==io::ErrorKind::NotFound=>{},Err(_)=>return Err(no()),
    }Ok(())
}
#[derive(Eq,PartialEq)]
pub(super) struct Mapping<'a>{path:&'a str,major:u32,minor:u32,ino:u64}
pub(super) fn actual<'a>(path:&'a str,f:&File)->io::Result<Mapping<'a>>{
    let m=f.metadata()?;Ok(Mapping{path,major:major(m.dev()),minor:minor(m.dev()),ino:m.ino()})
}
/// /proc maps reports the mapped file's device/inode and kernel path. Numeric
/// identity is combined with the independently held exact public release
/// file and approved bytes. No memory reads, logging of paths, or ELF execution.
pub(super) fn matched(raw:&str,sites:&[usize],expected:&Mapping<'_>)->io::Result<()>{
    segments(raw,sites,expected,true)
}
pub(super) fn segments(raw:&str,sites:&[usize],expected:&Mapping<'_>,code:bool)->io::Result<()>{

    if raw.len()>MAX_MAPS||!raw.ends_with('\n')||sites.is_empty()||sites.len()>16{return Err(no());}
    let mut hits=[false;16];
    for (n,line) in raw.lines().enumerate(){
        if n>=4096{return Err(no());}
        let mut f=line.split_ascii_whitespace();let range=f.next().and_then(|s|s.split_once('-')).ok_or_else(no)?;
        let a=usize::from_str_radix(range.0,16).map_err(|_|no())?;
        let b=usize::from_str_radix(range.1,16).map_err(|_|no())?;
        if a>=b{return Err(no());}
        if !sites.iter().any(|p|(a..b).contains(p)){continue;}
        let mode=f.next().ok_or_else(no)?;
        if if code{mode!="r-xp"}else{!["r--p","rw-p","r-xp"].contains(&mode)}{return Err(no());}let _offset=f.next().ok_or_else(no)?;
        let (ma,mi)=f.next().and_then(|s|s.split_once(':')).ok_or_else(no)?;
        let ma=u32::from_str_radix(ma,16).map_err(|_|no())?;let mi=u32::from_str_radix(mi,16).map_err(|_|no())?;
        let ino=f.next().ok_or_else(no)?.parse::<u64>().map_err(|_|no())?;
        if (ma,mi,ino)!=(expected.major,expected.minor,expected.ino)||f.next()!=Some(expected.path)||f.next().is_some(){return Err(no());}
        for(i,p)in sites.iter().enumerate(){if(a..b).contains(p)&&std::mem::replace(&mut hits[i],true){return Err(no());}}
    }
    if hits[..sites.len()].iter().all(|v|*v){Ok(())}else{Err(no())}
}
pub(super) fn mappings(a:&File,b:&File,libs:&first_smb::Libraries)->io::Result<()>{
    let mut bytes=Vec::new();thread_file(c"thread-self/maps")?.take((MAX_MAPS+1)as u64).read_to_end(&mut bytes)?;
    let raw=std::str::from_utf8(&bytes).map_err(|_|no())?;
    let (s,t)=libs.entry_sites();
    matched(raw,&s,&actual(&format!("{LIB_ROOT}libgcrypt.so.20"),a)?)?;
    matched(raw,&[t],&actual(&format!("{LIB_ROOT}libgpg-error.so.0"),b)?)
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn merely_installing_the_right_file_does_not_prove_the_active_provider(){
        let s="1000-2000 r-xp 00004000 08:ab 125 /private/lib.so\n2000-3000 rw-p 00005000 08:ab 125 /private/lib.so\n";
        let g=Mapping{path:"/private/lib.so",major:8,minor:171,ino:125};
        matched(s,&[0x1100,0x1fff],&g).unwrap();
        assert!(matched(s,&[0x2000],&g).is_err());assert!(matched(s,&[0x3000],&g).is_err());
        for v in [s.replace("08:ab","08:ac"),s.replace(" 125 "," 126 "),s.replace("/private","/host"),s.replace(".so",".so (deleted)"),s.replace("r-xp","rwxp"),s.repeat(2)]{
            assert!(matched(&v,&[0x1100],&g).is_err());}
    }
}
