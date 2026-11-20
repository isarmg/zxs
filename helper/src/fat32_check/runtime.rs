//! Verified installation for one fixed Linux/x86-64 profile, not a tool resolver.
use crate::{fs_tools::{RuntimeFile,hex},pinned::invalid};
use std::{io::{self,Read},os::unix::fs::FileExt};

use super::Format;
pub(super) const PROFILE:&str="debian13-amd64/private-pack-1/bubblewrap-0.12/dosfstools-4.2";
const KEYS:[&str;8]=["bwrap","fsck.fat","ld-linux-x86-64","libc","libcap","libselinux","libpcre2","libgcc"];
macro_rules! pack {($($name:literal),+) => {&["usr","lib","local-ops-helper","disk-tools-v1",$($name),+]};}
const SLOTS:[&[&str];8]=[
    pack!("bin","bwrap"),pack!("bin","fsck.fat"),pack!("lib","ld-linux-x86-64.so.2"),
    pack!("lib","libc.so.6"),pack!("lib","libcap.so.2"),pack!("lib","libselinux.so.1"),
    pack!("lib","libpcre2-8.so.0"),pack!("lib","libgcc_s.so.1"),
];
const POLICY:&[&str]=pack!("fat32-check-v1");
const EX_PROFILE:&str="debian13-amd64/private-pack-1/bubblewrap-0.12/exfatprogs-1.2.9";
const EX_POLICY:&[&str]=pack!("exfat-check-v1");
const EX_CHECKER:&[&str]=pack!("bin","fsck.exfat");
const EXT_PROFILE:&str="debian13-amd64/private-pack-1/bubblewrap-0.12/e2fsprogs-1.47.2";
const EXT_POLICY:&[&str]=pack!("ext4-check-v1");
const EXT_POLICY_BYTES:u64=1046;
const EXT_CHECKER:&[&str]=pack!("bin","e2fsck");
// Only these five extra DSOs are part of the exact e2fsck and guest libc
// closure. They are not replacement host loaders or Samba dependencies.
const EXT_LIBS:[(&str,&[&str],&str);5]=[
    ("libext2fs.so.2",pack!("lib","libext2fs.so.2"),"/lib/x86_64-linux-gnu/libext2fs.so.2"),
    ("libcom_err.so.2",pack!("lib","libcom_err.so.2"),"/lib/x86_64-linux-gnu/libcom_err.so.2"),
    ("libblkid.so.1",pack!("lib","libblkid.so.1"),"/lib/x86_64-linux-gnu/libblkid.so.1"),
    ("libuuid.so.1",pack!("lib","libuuid.so.1"),"/lib/x86_64-linux-gnu/libuuid.so.1"),
    ("libe2p.so.2",pack!("lib","libe2p.so.2"),"/lib/x86_64-linux-gnu/libe2p.so.2"),
];
const MAX_ELF:u64=32<<20;

#[derive(Clone,Copy,PartialEq,Eq)]
pub(super) enum Engine{Copy(Format),Ext4}
impl Engine{
    fn profile(self)->&'static str{match self{Self::Copy(Format::Fat32)=>PROFILE,Self::Copy(Format::Exfat)=>EX_PROFILE,Self::Ext4=>EXT_PROFILE}}
    fn policy(self)->&'static [&'static str]{match self{Self::Copy(Format::Fat32)=>POLICY,Self::Copy(Format::Exfat)=>EX_POLICY,Self::Ext4=>EXT_POLICY}}
    fn checker(self)->(&'static str,&'static [&'static str]){
        match self{Self::Copy(Format::Fat32)=>(KEYS[1],SLOTS[1]),Self::Copy(Format::Exfat)=>("fsck.exfat",EX_CHECKER),Self::Ext4=>("e2fsck",EXT_CHECKER)}
    }
    fn slots(self)->impl Iterator<Item=(&'static str,&'static [&'static str])>{
        KEYS.into_iter().zip(SLOTS).enumerate().map(move |(i,(label,slot))|if i==1{self.checker()}else{(label,slot)})
            .chain(EXT_LIBS.into_iter().take(if self==Self::Ext4{EXT_LIBS.len()}else{0}).map(|(key,slot,_)|(key,slot)))
    }
}

pub(super) struct Runtime { kind:Engine,policy:RuntimeFile, files:Vec<RuntimeFile> }
fn pins_for(kind:Engine,bytes:&[u8])->io::Result<Vec<[u8;32]>> {
    let bad=||invalid("missing or unsupported protected filesystem checker profile");
    let mut lines=std::str::from_utf8(bytes).map_err(|_|bad())?.split('\n');
    if lines.next()!=Some(kind.profile()){return Err(bad());}
    let mut digests=Vec::new();
    for (label,_) in kind.slots(){
        let (key,value)=lines.next().and_then(|s|s.split_once(' ')).ok_or_else(bad)?;
        if key!=label{return Err(bad());}digests.push(hex(value).ok_or_else(bad)?);
    }
    if lines.next()!=Some("")||lines.next().is_some(){return Err(bad());}
    Ok(digests)
}
impl Runtime {
    pub(super) fn open()->io::Result<Self>{ Ok(Self::open_for(None)?.0) }
    // Normal diagnostics still trust their protected distribution slot. The
    // host SAM issuer additionally pins this profile from its separate release.
    pub(super) fn open_for(release:Option<&[u8;32]>)->io::Result<(Self,[u8;32])>{
        Self::selected(Engine::Copy(Format::Fat32),release)
    }
    pub(super) fn exfat()->io::Result<Self>{Ok(Self::selected(Engine::Copy(Format::Exfat),None)?.0)}
    pub(super) fn ext4()->io::Result<Self>{Ok(Self::selected(Engine::Ext4,None)?.0)}
    fn selected(kind:Engine,release:Option<&[u8;32]>)->io::Result<(Self,[u8;32])>{
        use sha2::{Digest,Sha256};
        let limit=if kind==Engine::Ext4{EXT_POLICY_BYTES}else{1024};
        let mut policy=RuntimeFile::open(kind.policy(),false,limit)?;
        let mut bytes=Vec::new();(&mut policy.file).take(limit+1).read_to_end(&mut bytes)?;
        if bytes.len() as u64>limit{return Err(invalid("protected checker profile exceeded its size budget"));}
        if release.is_some_and(|approved|<[u8;32]>::from(Sha256::digest(&bytes))!=*approved){
            return Err(invalid("not the independently enrolled host launcher profile"));
        }
        let expected=pins_for(kind,&bytes)?;policy.recheck()?;
        let mut files=Vec::new();
        for (i,(_,slot)) in kind.slots().enumerate() {
            let mut f=RuntimeFile::open(slot,i<3,MAX_ELF)?;
            if f.digest()?!=expected[i]{return Err(invalid("fixed checker runtime does not match the approved host profile"));}
            let mut elf=[0;20];f.file.read_exact_at(&mut elf,0)?;
            if &elf[..7]!=b"\x7fELF\x02\x01\x01"||elf[18..20]!=[62,0]{
                return Err(invalid("expected approved x86-64 ELF installation bytes"));
            }
            files.push(f);
        }
        let runtime=Self{kind,policy,files};
        runtime.recheck()?;Ok((runtime,expected[0]))
    }
    pub(super) fn require_format(&self,kind:Format)->io::Result<()>{
        if self.kind==Engine::Copy(kind){Ok(())}else{Err(invalid("checker profile and selected format differ"))}
    }
    pub(super) fn require_ext4(&self)->io::Result<()> {
        if self.kind!=Engine::Ext4{return Err(invalid("not the approved ext4 checker profile"));}
        Ok(())
    }
    pub(super) fn launcher(&self)->&std::fs::File { &self.files[0].file }
    pub(super) fn sandbox_files(&self)->impl Iterator<Item=(&std::fs::File,&'static str)>{
        self.files[1..4].iter().map(|f|&f.file).zip(["/checker","/lib64/ld-linux-x86-64.so.2","/lib/x86_64-linux-gnu/libc.so.6"])
            .chain(self.files[8..].iter().map(|f|&f.file).zip(EXT_LIBS.map(|(_,_,dest)|dest)))
            .chain((self.kind==Engine::Ext4).then(||(&self.files[2].file,"/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2")))
    }
    pub(super) fn recheck(&self)->io::Result<()>{
        self.policy.recheck()?; for f in &self.files{f.recheck()?;}Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ext4_profile_cannot_substitute_another_version_or_omit_the_actual_guest_closure(){
        let row=|k|format!("{k} {}\n","12".repeat(32));
        let full=format!("{EXT_PROFILE}\n{}",Engine::Ext4.slots().map(|(k,_)|row(k)).collect::<String>());
        assert_eq!(full.len() as u64,EXT_POLICY_BYTES);
        assert_eq!(pins_for(Engine::Ext4,full.as_bytes()).unwrap().len(),13);
        for bad in [full.replace(EXT_PROFILE,EX_PROFILE),full.replace("e2fsck ","fsck.exfat "),
                    full.replace(&row("libblkid.so.1"),""),full.clone()+&row("libss.so.2")]{
            assert!(pins_for(Engine::Ext4,bad.as_bytes()).is_err());
        }
    }
    #[test]
    fn protected_profile_has_exact_version_eight_named_digests_and_no_paths_or_extra_switches(){
        let lines=KEYS.iter().map(|k|format!("{k} {}\n","12".repeat(32))).collect::<String>();
        let valid=format!("{PROFILE}\n{lines}");assert!(pins_for(Engine::Copy(Format::Fat32),valid.as_bytes()).is_ok());
        let ex=valid.replace(PROFILE,EX_PROFILE).replace("fsck.fat ","fsck.exfat ");
        assert!(pins_for(Engine::Copy(Format::Exfat),ex.as_bytes()).is_ok());
        assert!(pins_for(Engine::Copy(Format::Fat32),ex.as_bytes()).is_err());
        assert!(pins_for(Engine::Copy(Format::Exfat),valid.as_bytes()).is_err());
        for bad in [valid.replace("libc ","../lib "),valid.replace("4.2","4.1"),valid.replace("bwrap ","fsck.fat "),valid.clone()+"--share-net\n",valid.trim_end().to_string()]{
            assert!(pins_for(Engine::Copy(Format::Fat32),bad.as_bytes()).is_err());
        }
    }
}
