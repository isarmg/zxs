//! Actual package material, not a SAM request or offline authorization. The
//! host separately enrolls the signed distribution's exact manifest digest.
mod inventory;
use super::{CAPSULE_USR,live::Stamp};
use crate::{fat32_check::SamLauncher,fs_tools::RuntimeFile,pinned::{Kind,Pinned},sqlite_install::bad};
use sha2::{Digest,Sha256};
use std::{collections::{BTreeMap,BTreeSet},fs::File,io::{self,Read},os::unix::fs::MetadataExt,path::Path};

const TITLE:&str="LOCAL-OPS/SAM-USR/1 debian13-amd64 samba-4.22.11-deb13u1 python3.13";
const MANIFEST:&[&str]=&["usr","lib","local-ops-helper","samba-4.22.11-deb13u1-v1","CAPSULE"];
const POLICY:&[&str]=&["etc","local-ops-helper","samba-provision.release"];
const HELPER:&[&str]=&["usr","bin","helper"];
const LIMIT:usize=2<<20;
fn invalid()->io::Error{bad("unconfirmed protected Samba release; do not provision")}
fn hex(s:&str)->io::Result<[u8;32]>{
    if !s.bytes().all(|c|c.is_ascii_digit()||matches!(c,b'a'..=b'f')){return Err(invalid());}
    crate::fs_tools::hex(s).ok_or_else(invalid)
}
#[derive(Clone,PartialEq,Eq)]
struct Leaf{mode:u32,size:u64,sha:[u8;32]}
struct Layout{files:BTreeMap<String,Leaf>,dirs:BTreeMap<String,BTreeSet<String>>}
fn component(x:&str)->bool{!matches!(x,""|"."|"..")&&x.len()<=150&&x.bytes().all(|b|b.is_ascii_alphanumeric()||b"_+.-".contains(&b))}
fn relative(s:&str)->io::Result<&str>{
    let r=s.strip_prefix("usr/").ok_or_else(invalid)?;
    if s.len()>512||r.split('/').count()>32||!r.split('/').all(component){return Err(invalid());}Ok(r)
}
fn deps(value:&str)->bool{
    if value=="-"{return true;}
    let mut seen=BTreeSet::new();
    value.split(',').all(|d|{
        component(d)&&d.match_indices(".so").any(|(i,_)|i>0&&(i+3==d.len()||
            (d.as_bytes().get(i+3)==Some(&b'.')&&i+4<d.len())))&&seen.insert(d)&&seen.len()<=256
    })
}
fn decimal(v:&str)->io::Result<u64>{
    let n=v.parse::<u64>().ok().filter(|n|n.to_string()==v&&*n<=64<<20).ok_or_else(invalid)?;Ok(n)
}
impl Layout{
    fn parse(bytes:&[u8])->io::Result<Self>{
        if bytes.len()>LIMIT||!bytes.is_ascii(){return Err(invalid());}
        let text=std::str::from_utf8(bytes).map_err(|_|invalid())?.strip_suffix('\n').ok_or_else(invalid)?;
        let mut lines=text.split('\n');if lines.next()!=Some(TITLE){return Err(invalid());}
        let mut out=Self{files:BTreeMap::new(),dirs:BTreeMap::new()};
        let mut last="";let mut total=0u64;
        for line in lines{
            if line.len()+1>8192||out.files.len()>=2048{return Err(invalid());}
            let mut it=line.split('\t');
            let mut next=||it.next().ok_or_else(invalid);
            let path=relative(next()?)?;
            let mode=match next()?{"0644"=>0o644,"0755"=>0o755,_=>return Err(invalid())};
            let size=decimal(next()?)?;let sha=hex(next()?)?;let pkg=next()?;let _member=relative(next()?)?;let needed=next()?;
            if it.next().is_some()||path<=last||!component(pkg)||!pkg.as_bytes()[0].is_ascii_alphanumeric()
                ||!pkg.bytes().all(|c|c.is_ascii_lowercase()||c.is_ascii_digit()||b"+.-".contains(&c))
                ||!deps(needed){return Err(invalid());}
            total=total.checked_add(size).filter(|n|*n<=900<<20).ok_or_else(invalid)?;
            let mut parent=String::new();for part in path.split('/'){
                out.dirs.entry(parent.clone()).or_default().insert(part.to_owned());
                if !parent.is_empty(){parent.push('/');}parent.push_str(part);
            }
            if out.dirs.len()>2048{return Err(invalid());}
            out.files.insert(path.into(),Leaf{mode,size,sha});last=path;
        }
        if out.files.is_empty()||out.files.keys().any(|p|out.dirs.contains_key(p)){return Err(invalid());}
        for p in ["bin/python3.13","lib64/ld-linux-x86-64.so.2"]{
            if out.files.get(p).is_none_or(|v|v.mode!=0o755||v.size==0){return Err(invalid());}
        }Ok(out)
    }
}
#[derive(Clone,Copy,Eq,PartialEq,Debug)]
pub(super) struct Keys{manifest:[u8;32],helper:[u8;32],launcher_profile:[u8;32]}
impl Keys{
    pub(super) fn bind(&self,launcher:&[u8;32])->[u8;32]{
        let mut h=Sha256::new();h.update(b"helper: one authenticated dedicated SAM release v1\0");
        for key in [&self.manifest,&self.helper,&self.launcher_profile,launcher]{h.update(key);}
        h.finalize().into()
    }
}
pub(super) struct Originals{
    pin:Pinned,usr:File,manifest:RuntimeFile,policy:RuntimeFile,worker:RuntimeFile,
    launcher:SamLauncher,keys:Keys,layout:Layout,observed:BTreeMap<String,Stamp>,
}
fn read(f:&mut RuntimeFile,m:u32,max:u64)->io::Result<Vec<u8>>{
    let d=f.file.metadata()?;
    if d.mode()!=(0o100000|m)||d.uid()!=0||d.gid()!=0||d.nlink()!=1||d.len()==0||d.len()>max{return Err(invalid());}
    let mut v=Vec::with_capacity(d.len()as usize);(&f.file).take(max+1).read_to_end(&mut v)?;
    if v.len()as u64!=d.len(){return Err(invalid());}f.recheck()?;Ok(v)
}
fn expected(bytes:&[u8])->io::Result<Keys>{
    let line=std::str::from_utf8(bytes).map_err(|_|invalid())?;
    let parts=line.strip_suffix('\n').ok_or_else(invalid)?.split(' ').collect::<Vec<_>>();
    let ["HOST-SAM-RELEASE-1",m,w,p]=parts.as_slice() else{return Err(invalid());};
    Ok(Keys{manifest:hex(m)?,helper:hex(w)?,launcher_profile:hex(p)?})
}
// Before permanently dropping identity the exec'd worker may read only
// these fixed root-owned public policy inputs. All data FDs die on return.
pub(super) fn worker_keys(parent:rustix::process::Pid)->io::Result<Keys>{
    let mut policy=RuntimeFile::open(POLICY,false,256)?;
    let k=expected(&read(&mut policy,0o600,256)?)?;
    let mut worker=RuntimeFile::open(HELPER,true,64<<20)?;
    worker.require_running_helper()?;worker.require_parent_helper(parent)?;
    let m=worker.file.metadata()?;
    if m.mode()!=0o100755||m.gid()!=0||m.nlink()!=1||worker.digest()?!=k.helper{return Err(invalid());}
    policy.recheck()?;worker.require_parent_helper(parent)?;Ok(k)
}
impl Originals{
    /// For the single approved host installer. The manifest is authenticated
    /// by the separately enrolled root policy BEFORE interpreting any pathname.
    /// The manifest itself must not be used to create/update that policy.
    pub(super) fn open()->io::Result<Self>{
        let _host=crate::sqlite_install::approval::host_admin()?;
        let mut policy=RuntimeFile::open(POLICY,false,256)?;
        let keys=expected(&read(&mut policy,0o600,256)?)?;
        let mut manifest=RuntimeFile::open(MANIFEST,false,LIMIT as u64)?;
        let bytes=read(&mut manifest,0o644,LIMIT as u64)?;
        if <[u8;32]>::from(Sha256::digest(&bytes))!=keys.manifest{return Err(invalid());}
        let layout=Layout::parse(&bytes)?;
        let pin=Pinned::open(Path::new(CAPSULE_USR),Kind::StorageRoot)?;
        let usr=pin.relative_directory()?;
        manifest.require_samba_usr(&usr)?;
        let mut worker=RuntimeFile::open(HELPER,true,64<<20)?;
        worker.require_running_helper()?;let m=worker.file.metadata()?;
        if m.mode()!=0o100755||m.gid()!=0||m.nlink()!=1||worker.digest()?!=keys.helper{return Err(invalid());}
        // This is the installed relocated executable. Never execute to identify it.
        let launcher=SamLauncher::for_release(&keys.launcher_profile)?;
        let observed=inventory::check(&usr,&layout,true)?;
        let result=Self{pin,usr,manifest,policy,worker,launcher,keys,layout,observed};
        result.require()?;Ok(result)
    }
    pub(super) fn key(&self)->[u8;32]{
        self.keys.bind(&self.launcher.sha())
    }
    pub(super) fn bind_worker(&self,cmd:&mut std::process::Command)->io::Result<()>{
        self.slots()?;helper_linux_evidence::diagnostic_process::prepare_sam_worker_exec(cmd,&self.worker.file)
    }
    pub(super) fn original_launcher(&self)->[u8;32]{self.launcher.sha()}
    pub(super) fn original(&self)->io::Result<(crate::pinned::Entity,u64)>{super::mark(&self.usr)}
    pub(super) fn slots(&self)->io::Result<()>{
        self.pin.require_same_path()?;self.manifest.require_samba_usr(&self.usr)?;
        self.policy.recheck()?;self.worker.require_running_helper()?;self.launcher.require()
    }
    pub(super) fn require(&self)->io::Result<()>{
        self.slots()?;
        if inventory::check(&self.usr,&self.layout,false)?!=self.observed{return Err(invalid());}
        self.slots()
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn row(p:&str,mode:&str)->String{format!("usr/{p}\t{mode}\t1\t{}\tpkg\tusr/member\t-\n","01".repeat(32))}
    #[test]fn strict_manifest_cannot_broaden_into_host_paths_or_mix_a_file_and_its_directory(){
        let exe=row("bin/python3.13","0755");let ld=row("lib64/ld-linux-x86-64.so.2","0755");
        let valid=format!("{TITLE}\n{exe}{ld}");let l=Layout::parse(valid.as_bytes()).unwrap();
        assert_eq!(l.files.len(),2);assert_eq!(l.dirs.len(),3);
        for v in [format!("{TITLE}\n{ld}{exe}"),format!("{TITLE}\n{exe}{exe}{ld}"),
            format!("{TITLE}\n{}{exe}{ld}",row("bin","0644")), valid.replace("usr/bin/","usr/../"),
            valid.replace("usr/bin/","/bin/"),valid.replace("\t1\t","\t01\t"),valid.replace("0755","4755"),
            valid.replace("usr/member","../member"),valid.replace("\tpkg\t","\tbad/option\t"),valid.replace("\t-\n","\t../x.so\n"),
            valid.replace("\t-\n","\tliba.so,liba.so\n"),valid.replace("\t-\n","\tlibc.dat\n"),valid.trim_end().to_owned(),valid.replace("SAM-USR/1","SAM-USR/2")]{
            assert!(Layout::parse(v.as_bytes()).is_err());}
    }
    #[test]fn independent_release_selects_three_distinct_purposes_and_rejects_a_missing_one(){
        let(a,b,c)=("12".repeat(32),"ab".repeat(32),"47".repeat(32));
        let line=format!("HOST-SAM-RELEASE-1 {a} {b} {c}\n");
        assert_eq!(expected(line.as_bytes()).unwrap(),Keys{manifest:[0x12;32],helper:[0xab;32],launcher_profile:[0x47;32]});
        for v in [line.trim_end().into(),format!("HOST-SAM-RELEASE-1 {a}\n"),line.replace("ab","AB"),
            format!("{line}{line}"),line.replace("RELEASE-1","USR-1")]{
            assert!(expected(v.as_bytes()).is_err());}
    }
}
