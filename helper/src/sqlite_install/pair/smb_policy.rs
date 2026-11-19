//! A *site* decision for this one pool / dedicated instance, not a declaration
//! by the package itself. This gate is consulted only for a genuinely absent
//! first SETUP_PAIR. It grants neither an online session nor a reset.
use super::{Id,domain::InstalledSetup};
use crate::{fs_tools::RuntimeFile,maintenance::thread_file};
use helper_linux_evidence::first_smb;
use rustix::fs::{major,minor};
use std::{fs::File,io::{self,Read},os::unix::fs::MetadataExt};

const POLICY:&[&str]=&["etc","local-ops-helper","ntlmv2.policy"];
const HELPER:&[&str]=&["usr","bin","helper"];
const GCRYPT:&[&str]=&["usr","lib","local-ops-helper","disk-tools-v1","lib","libgcrypt.so.20"];
const GPGERR:&[&str]=&["usr","lib","local-ops-helper","disk-tools-v1","lib","libgpg-error.so.0"];
const LIB_ROOT:&str="/usr/lib/local-ops-helper/disk-tools-v1/lib/";
const MAX_MAPS:usize=256<<10;
fn no()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,
    "no current site permission for the exact installed NTLMv2 enrollment provider and instance")}

#[derive(Clone,Copy,Eq,PartialEq)]
struct Rule{pool:Id,instance:Id,provision:Id,live:[u8;32],helper:[u8;32],provider:[u8;32],support:[u8;32]}
fn lower<const N:usize>(s:&str)->io::Result<[u8;N]>{
    if s.len()!=N*2||!s.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)){return Err(no());}
    let mut out=[0;N];for(i,p)in out.iter_mut().enumerate(){*p=u8::from_str_radix(&s[i*2..i*2+2],16).map_err(|_|no())?;}
    Ok(out)
}
impl Rule{
    fn parse(raw:&[u8])->io::Result<Self>{
        let line=std::str::from_utf8(raw).map_err(|_|no())?.strip_suffix('\n').ok_or_else(no)?;
        let mut w=line.split(' ');if w.next()!=Some("SITE-ALLOW-SETUP-NTLMV2-1"){return Err(no());}
        let mut one=||w.next().ok_or_else(no);
        let r=Self{pool:lower(one()?)?,instance:lower(one()?)?,provision:lower(one()?)?,
            live:lower(one()?)?,helper:lower(one()?)?,provider:lower(one()?)?,support:lower(one()?)?};
        if w.next().is_some()||[r.pool,r.instance,r.provision].contains(&[0;16]){return Err(no());}
        Ok(r)
    }
    fn scope(&self,pool:Id,instance:Id,provision:Id,live:[u8;32])->io::Result<()>{
        if(self.pool,self.instance,self.provision,self.live)==(pool,instance,provision,live){Ok(())}else{Err(no())}
    }
}
fn mode(f:&File,want:u32)->io::Result<()>{
    let m=f.metadata()?;if m.mode()!=(0o100000|want)||m.uid()!=0||m.gid()!=0||m.nlink()!=1{Err(no())}else{Ok(())}
}
fn release(slot:&'static [&'static str],perm:u32,pin:&[u8;32])->io::Result<RuntimeFile>{
    let mut f=RuntimeFile::open(slot,perm==0o755,64<<20)?;mode(&f.file,perm)?;
    if f.digest()?!=*pin{return Err(no());}Ok(f)
}
fn clean_environment()->bool{
    use std::os::unix::ffi::OsStrExt;
    std::env::vars_os().all(|(k,_)|{let k=k.as_bytes();
        !(k==b"GLIBC_TUNABLES"||[b"LD_".as_slice(),b"LIBGCRYPT_",b"GCRYPT_",b"MALLOC_"].iter().any(|p|k.starts_with(p)))})
}
// Do not interpret an unreadable kernel control as 0. Kernels not exposing
// this optional flag still need both explicit site authorization and a genuinely
// non-FIPS provider. Respect the upstream administrator force-file as well.
fn system_policy()->io::Result<()>{
    match std::fs::symlink_metadata("/etc/gcrypt/fips_enabled"){
        Ok(_)=>return Err(no()),Err(e) if e.kind()==io::ErrorKind::NotFound=>{},Err(_)=>return Err(no()),
    }
    match thread_file(c"sys/crypto/fips_enabled"){
        Ok(f)=>{let mut v=Vec::new();f.take(4).read_to_end(&mut v)?;if v!=b"0\n"{return Err(no());}}
        Err(e) if e.kind()==io::ErrorKind::NotFound=>{},Err(_)=>return Err(no()),
    }Ok(())
}
#[derive(Eq,PartialEq)]
struct Mapping<'a>{path:&'a str,major:u32,minor:u32,ino:u64}
fn actual<'a>(path:&'a str,f:&File)->io::Result<Mapping<'a>>{
    let m=f.metadata()?;Ok(Mapping{path,major:major(m.dev()),minor:minor(m.dev()),ino:m.ino()})
}
/// /proc maps reports the mapped file's device/inode and kernel path. Numeric
/// identity is combined with the independently held exact public release
/// file and approved bytes. No memory reads, logging of paths, or ELF execution.
fn matched(raw:&str,sites:&[usize],expected:&Mapping<'_>)->io::Result<()>{
    if raw.len()>MAX_MAPS||!raw.ends_with('\n')||sites.is_empty()||sites.len()>16{return Err(no());}
    let mut hits=[false;16];
    for (n,line) in raw.lines().enumerate(){
        if n>=4096{return Err(no());}
        let mut f=line.split_ascii_whitespace();let range=f.next().and_then(|s|s.split_once('-')).ok_or_else(no)?;
        let a=usize::from_str_radix(range.0,16).map_err(|_|no())?;
        let b=usize::from_str_radix(range.1,16).map_err(|_|no())?;
        if a>=b{return Err(no());}if !sites.iter().any(|p|(a..b).contains(p)){continue;}
        if f.next()!=Some("r-xp"){return Err(no());}let _offset=f.next().ok_or_else(no)?;
        let (ma,mi)=f.next().and_then(|s|s.split_once(':')).ok_or_else(no)?;
        let ma=u32::from_str_radix(ma,16).map_err(|_|no())?;let mi=u32::from_str_radix(mi,16).map_err(|_|no())?;
        let ino=f.next().ok_or_else(no)?.parse::<u64>().map_err(|_|no())?;
        if (ma,mi,ino)!=(expected.major,expected.minor,expected.ino)||f.next()!=Some(expected.path)||f.next().is_some(){return Err(no());}
        for(i,p)in sites.iter().enumerate(){if(a..b).contains(p)&&std::mem::replace(&mut hits[i],true){return Err(no());}}
    }
    if hits[..sites.len()].iter().all(|v|*v){Ok(())}else{Err(no())}
}
fn mappings(a:&File,b:&File,libs:&first_smb::Libraries)->io::Result<()>{
    let mut bytes=Vec::new();thread_file(c"thread-self/maps")?.take((MAX_MAPS+1)as u64).read_to_end(&mut bytes)?;
    let raw=std::str::from_utf8(&bytes).map_err(|_|no())?;
    let (s,t)=libs.entry_sites();
    matched(raw,&s,&actual(&format!("{LIB_ROOT}libgcrypt.so.20"),a)?)?;
    matched(raw,&[t],&actual(&format!("{LIB_ROOT}libgpg-error.so.0"),b)?)
}
pub(super) struct Permit{libs:first_smb::Libraries,policy:RuntimeFile,helper:RuntimeFile,provider:RuntimeFile,support:RuntimeFile}
impl Permit{
    pub(super) fn open(installed:&InstalledSetup,pool:Id)->io::Result<Self>{
        if !clean_environment(){return Err(no());}
        installed.require()?;let(instance,provision)=installed.installation();
        let policy=RuntimeFile::open(POLICY,false,512)?;mode(&policy.file,0o444)?;
        let mut text=Vec::new();(&policy.file).take(513).read_to_end(&mut text)?;
        if text.len()as u64!=policy.file.metadata()?.len(){return Err(no());}
        let r=Rule::parse(&text)?;r.scope(pool,instance,provision,installed.live_digest())?;policy.recheck()?;
        let helper=release(HELPER,0o755,&r.helper)?;helper.require_running_helper()?;
        let provider=release(GCRYPT,0o644,&r.provider)?;let support=release(GPGERR,0o644,&r.support)?;
        policy.recheck()?;provider.recheck()?;support.recheck()?;system_policy()?;
        let libs=first_smb::Libraries::load()?;
        let p=Self{libs,policy,helper,provider,support};p.require()?;Ok(p)
    }
    pub(super) fn qualify(&self)->io::Result<first_smb::Enrollment>{self.require()?;self.libs.qualify()}
    pub(super) fn require(&self)->io::Result<()>{
        system_policy()?;self.policy.recheck()?;self.helper.require_running_helper()?;self.provider.recheck()?;self.support.recheck()?;
        mappings(&self.provider.file,&self.support.file,&self.libs)?;self.policy.recheck()
    }
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn no_package_self_approval_no_wildcards_and_one_exact_scope(){
        let line=format!("SITE-ALLOW-SETUP-NTLMV2-1 {} {} {} {} {} {} {}\n","11".repeat(16),"22".repeat(16),
            "33".repeat(16),"44".repeat(32),"55".repeat(32),"66".repeat(32),"77".repeat(32));
        let r=Rule::parse(line.as_bytes()).unwrap();r.scope([0x11;16],[0x22;16],[0x33;16],[0x44;32]).unwrap();
        for args in [([0x12;16],r.instance,r.provision,r.live),(r.pool,[0x23;16],r.provision,r.live),
            (r.pool,r.instance,[0x34;16],r.live),(r.pool,r.instance,r.provision,[0x45;32])]{assert!(r.scope(args.0,args.1,args.2,args.3).is_err());}
        for v in [line.replace("SITE-ALLOW-SETUP-NTLMV2-1","PAIR-RELEASE-1"),line.replace(&"11".repeat(16),"*"),
            line.replace(&"66".repeat(32),"aA"),line.repeat(2),line.trim_end().to_owned()]{assert!(Rule::parse(v.as_bytes()).is_err());}
    }
    #[test]fn merely_installing_the_right_file_does_not_prove_the_active_provider(){
        let s="1000-2000 r-xp 00004000 08:ab 125 /private/lib.so\n2000-3000 rw-p 00005000 08:ab 125 /private/lib.so\n";
        let g=Mapping{path:"/private/lib.so",major:8,minor:171,ino:125};
        matched(s,&[0x1100,0x1fff],&g).unwrap();
        assert!(matched(s,&[0x2000],&g).is_err());assert!(matched(s,&[0x3000],&g).is_err());
        for v in [s.replace("08:ab","08:ac"),s.replace(" 125 "," 126 "),s.replace("/private","/host"),s.replace(".so",".so (deleted)"),s.replace("r-xp","rwxp"),s.repeat(2)]{
            assert!(matched(&v,&[0x1100],&g).is_err());}
    }
}
