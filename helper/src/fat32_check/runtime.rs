//! Verified installation for one fixed Linux/x86-64 profile, not a tool resolver.
use crate::{fs_tools::{RuntimeFile,hex},pinned::invalid};
use std::{io::{self,Read},os::unix::fs::FileExt};

pub(super) const PROFILE:&str="debian13-amd64/private-pack-1/bubblewrap-0.12/dosfstools-4.2";
const KEYS:[&str;8]=["bwrap","fsck.fat","ld-linux-x86-64","libc","libcap","libselinux","libpcre2","libgcc"];
macro_rules! pack {($($name:literal),+) => {&["usr","lib","local-ops-helper","disk-tools-v1",$($name),+]};}
const SLOTS:[&[&str];8]=[
    pack!("bin","bwrap"),pack!("bin","fsck.fat"),pack!("lib","ld-linux-x86-64.so.2"),
    pack!("lib","libc.so.6"),pack!("lib","libcap.so.2"),pack!("lib","libselinux.so.1"),
    pack!("lib","libpcre2-8.so.0"),pack!("lib","libgcc_s.so.1"),
];
const POLICY:&[&str]=pack!("fat32-check-v1");
const MAX_ELF:u64=32<<20;

pub(super) struct Runtime { policy:RuntimeFile, files:[RuntimeFile;8] }
fn pins(bytes:&[u8])->io::Result<[[u8;32];8]> {
    let bad=||invalid("missing or unsupported protected FAT32 checker profile");
    let mut lines=std::str::from_utf8(bytes).map_err(|_|bad())?.split('\n');
    if lines.next()!=Some(PROFILE){return Err(bad());}
    let mut digests=[[0;32];8];
    for (i,k) in KEYS.iter().enumerate(){
        let (key,value)=lines.next().and_then(|s|s.split_once(' ')).ok_or_else(bad)?;
        if key!=*k{return Err(bad());}digests[i]=hex(value).ok_or_else(bad)?;
    }
    if lines.next()!=Some("")||lines.next().is_some(){return Err(bad());}
    Ok(digests)
}
impl Runtime {
    pub(super) fn open()->io::Result<Self>{ Ok(Self::open_for(None)?.0) }
    // Normal diagnostics still trust their protected distribution slot. The
    // host SAM issuer additionally pins this profile from its separate release.
    pub(super) fn open_for(release:Option<&[u8;32]>)->io::Result<(Self,[u8;32])>{
        use sha2::{Digest,Sha256};
        let mut policy=RuntimeFile::open(POLICY,false,1024)?;
        let mut bytes=Vec::new();(&mut policy.file).take(1025).read_to_end(&mut bytes)?;
        if release.is_some_and(|approved|<[u8;32]>::from(Sha256::digest(&bytes))!=*approved){
            return Err(invalid("not the independently enrolled host launcher profile"));
        }
        let expected=pins(&bytes)?;policy.recheck()?;
        let mut files=Vec::new();
        for (i,&slot) in SLOTS.iter().enumerate() {
            let mut f=RuntimeFile::open(slot,i<3,MAX_ELF)?;
            if f.digest()?!=expected[i]{return Err(invalid("fixed checker runtime does not match the approved host profile"));}
            let mut elf=[0;20];f.file.read_exact_at(&mut elf,0)?;
            if &elf[..7]!=b"\x7fELF\x02\x01\x01"||elf[18..20]!=[62,0]{
                return Err(invalid("expected approved x86-64 ELF installation bytes"));
            }
            files.push(f);
        }
        let runtime=Self{policy,files:files.try_into().map_err(|_|invalid("incomplete checker runtime"))?};
        runtime.recheck()?;Ok((runtime,expected[0]))
    }
    pub(super) fn launcher(&self)->&std::fs::File { &self.files[0].file }
    pub(super) fn sandbox_files(&self)->impl Iterator<Item=(&std::fs::File,&'static str)>{
        self.files[1..4].iter().map(|f|&f.file).zip(["/checker","/lib64/ld-linux-x86-64.so.2","/lib/x86_64-linux-gnu/libc.so.6"])
    }
    pub(super) fn recheck(&self)->io::Result<()>{
        self.policy.recheck()?; for f in &self.files{f.recheck()?;}Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protected_profile_has_exact_version_eight_named_digests_and_no_paths_or_extra_switches(){
        let lines=KEYS.iter().map(|k|format!("{k} {}\n","12".repeat(32))).collect::<String>();
        let valid=format!("{PROFILE}\n{lines}");assert!(pins(valid.as_bytes()).is_ok());
        for bad in [valid.replace("libc ","../lib "),valid.replace("4.2","4.1"),valid.replace("bwrap ","fsck.fat "),valid.clone()+"--share-net\n",valid.trim_end().to_string()]{
            assert!(pins(bad.as_bytes()).is_err());
        }
    }
}
