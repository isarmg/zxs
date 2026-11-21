//! Only the pair's signed closure may supply executable/loadable originals.
//! A root-managed expected manifest hash is a selection after external
//! verification, never a signature, substitute for signature, or launch grant.
use super::{no,origins::{ident,mid}};
use crate::{fs_tools::{RuntimeFile,hash_upto},pinned};
use helper_linux_evidence::smb_views::{Access,Public,Source};
use rustix::{fs::{Mode,OFlags,ResolveFlags,fgetxattr,open,openat,openat2},io::Errno};
use sha2::{Digest,Sha256};
use std::{fs::File,io::{self,Read,Seek},os::unix::fs::{FileExt,MetadataExt},path::Path};

const POLICY:&[&str]=&["etc","local-ops-helper","online-smb.policy"];
const MANIFEST:&[&str]=&["usr","lib","zxs-smb-runtime","debian13-amd64","ELF-ROLES-1"];
const ROOT:&[&str]=&["usr","lib","zxs-smb-runtime","debian13-amd64","elf-roles"];
const LOCK:&str=include_str!("../../packaging/debian13-amd64/online-sources.json");
const SCHEMA:&[u8]=include_bytes!("../sqlite_install/schema.sql");
const DIR:OFlags=OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
const LEAF:OFlags=OFlags::PATH.union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
const SAFE:ResolveFlags=ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS).union(ResolveFlags::NO_XDEV);
const MAX:u64=512<<10;
pub(super) const ELF:u64=128<<20;
const NAMES:[&str;2]=["smbd","storage"];
const PROGRAMS:[&str;2]=["usr/lib/zxs-smb-runtime/debian13-amd64/sbin/smbd",
    "usr/lib/zxs-smb-runtime/debian13-amd64/bin/zxs-smb-storage"];
const LIB:&str="usr/lib/x86_64-linux-gnu/";
const LOADER:&str="usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2";
const ALIAS:&str="lib64/ld-linux-x86-64.so.2";
const NSS:[&str;4]=["etc/nsswitch.conf","etc/passwd","etc/group","etc/zxs-smb-identities"];
const CONVERSION:[&str;3]=["usr/lib/x86_64-linux-gnu/gconv/IBM850.so",
    "usr/lib/x86_64-linux-gnu/gconv/UTF-16.so","usr/lib/x86_64-linux-gnu/gconv/gconv-modules"];
type Hash=[u8;32];

fn lower(s:&str,n:usize)->bool{
    s.len()==n&&s.bytes().all(|b|b.is_ascii_digit()||matches!(b,b'a'..=b'f'))
}
fn hash(s:&str)->io::Result<Hash>{
    if !lower(s,64){return Err(no());}
    crate::fs_tools::hex(s).filter(|d|d!=&[0;32]).ok_or_else(no)
}
fn frozen_tree()->io::Result<&'static str>{
    let mut found=LOCK.lines().map(str::trim).filter_map(|l|l.strip_prefix("\"project_tree\": \""))
        .filter_map(|v|v.strip_suffix("\","));
    let exact=found.next().filter(|s|lower(s,40)).ok_or_else(no)?;
    if found.next().is_some(){return Err(no());}
    Ok(exact)
}
fn field<'a>(l:Option<&'a str>,key:&str)->io::Result<&'a str>{
    l.and_then(|v|v.strip_prefix(key)).ok_or_else(no)
}
fn protected(f:&File)->io::Result<()>{
    let m=f.metadata()?;
    if !m.is_dir()||m.uid()!=0||m.gid()!=0||m.mode()&0o7022!=0||m.nlink()==0{return Err(no());}
    Ok(())
}
fn parents(role:usize)->io::Result<File>{
    let mut p=File::from(open(c"/",DIR,Mode::empty())?);
    protected(&p)?;
    for name in ROOT.iter().copied().chain([NAMES[role]]){
        p=File::from(openat(&p,name,DIR,Mode::empty())?);protected(&p)?;
    }
    Ok(p)
}

#[derive(Clone,Copy,Eq,PartialEq)]
enum Part{Exec,Code,Public,Nss}
#[derive(Clone,Eq,PartialEq)]
struct Row{ path:String,mode:u32,size:u64,sha:Hash,links:u64,part:Part }
fn program_path(side:usize,p:&str)->bool{p==PROGRAMS[side]}
fn lib_name(p:&str)->bool{
    p.len()<=150&&p.split_once(".so").is_some_and(|(first,tail)|!first.is_empty()&&
        (tail.is_empty()||tail.starts_with('.')&&tail.len()>1))&&
        p.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'.'|b'-'|b'_'|b'+'))
}
fn part(side:usize,path:&str)->Option<(Part,u32,u64,u64)>{
    if program_path(side,path){return Some((Part::Exec,0o755,1,ELF));}
    if [LOADER,ALIAS].contains(&path){return Some((Part::Code,0o755,2,ELF));}
    if side==0 {
        if NSS.contains(&path){return Some((Part::Nss,0o644,1,512));}
        if CONVERSION.contains(&path) {
            return Some(if path==CONVERSION[2]{(Part::Public,0o644,1,1024)}
                else{(Part::Code,0o644,1,ELF)});
        }
    }
    let leaf=path.strip_prefix(LIB)?;
    let s=leaf.strip_prefix("samba/").unwrap_or(leaf);
    if lib_name(s){Some((Part::Code,0o644,1,ELF))}else{None}
}
fn rows(raw:&[u8],source:&str,expected_schema:Hash)->io::Result<(Hash,[Vec<Row>;2])>{
    if raw.is_empty()||raw.len() as u64>MAX||!raw.ends_with(b"\n"){return Err(no());}
    let mut lines=std::str::from_utf8(raw).map_err(|_|no())?.split_terminator('\n');
    if lines.next()!=Some("ZXS-SMB/ELF-ROLES-1")||field(lines.next(),"source ")?!=source{
        return Err(no());
    }
    if hash(field(lines.next(),"schema ")?)?!=expected_schema{return Err(no());}
    let nss=hash(field(lines.next(),"nss ")?)?;
    let mut out:[Vec<Row>;2]=[Vec::new(),Vec::new()];let mut last=0;let mut totals=[0u64;2];
    for line in lines{
        let words=line.split(' ').collect::<Vec<_>>();
        let [side,mode,size,sum,path]=words.as_slice()else{return Err(no());};
        let role=match *side{"smbd"=>0,"storage"=>1,_=>return Err(no())};
        if role<last||out[role].len()>=768||out[role].last().is_some_and(|r|r.path.as_str()>=*path){return Err(no());}
        last=role;
        let (part,bits,links,cap)=part(role,path).ok_or_else(no)?;
        if (*mode!="0755"||bits!=0o755)&&(*mode!="0644"||bits!=0o644){return Err(no());}
        let n=size.parse::<u64>().ok().filter(|n|*n>0&&*n<=cap&&n.to_string()==*size).ok_or_else(no)?;
        totals[role]+=n;if totals[role]>1<<30{return Err(no());}
        out[role].push(Row{path:(*path).to_owned(),mode:bits,size:n,sha:hash(sum)?,links,part});
    }
    for (i,side) in out.iter().enumerate(){
        if [PROGRAMS[i],LOADER,ALIAS].iter().any(|want|!side.iter().any(|r|r.path==*want)){return Err(no());}
    }
    if NSS.iter().chain(&CONVERSION).any(|want|!out[0].iter().any(|r|r.path==*want))
        ||out[0].iter().find(|r|r.path==NSS[3]).is_none_or(|r|r.sha!=nss){return Err(no());}
    Ok((nss,out))
}

#[derive(PartialEq,Eq)]
struct Stamp{object:crate::pinned::Entity,mount:u64,mtime:(i64,i64),ctime:(i64,i64)}
fn stamp(row:&Row,root:&File,pin:&File)->io::Result<Stamp>{
    let m=pin.metadata()?;
    let mount=mid(pin)?;
    if m.mode()!=(0o100000|row.mode)||m.uid()!=0||m.gid()!=0||m.nlink()!=row.links||m.len()!=row.size||mount!=mid(root)?{
        return Err(no());
    }
    Ok(Stamp{object:ident(pin)?,mount,mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec())})
}
fn named(root:&File,name:&str)->io::Result<File>{
    let (base,_)=name.rsplit_once('/').ok_or_else(no)?;
    // Check every intermediate parent's ownership; openat2 itself is not
    // proof that a mode-admitted leaf is below a protected directory.
    let mut here=root.try_clone()?;
    for component in base.split('/'){
        here=File::from(openat2(&here,component,DIR,Mode::empty(),SAFE)?);protected(&here)?;
    }
    Ok(File::from(openat2(root,name,LEAF,Mode::empty(),SAFE)?))
}
pub(super) struct Member{row:Row,original:Stamp,pub(super) file:File}
impl Member{
    fn select(root:&File,row:Row)->io::Result<Self>{
        let pin=named(root,&row.path)?;let original=stamp(&row,root,&pin)?;
        let mut file=pinned::installed_regular_reader(&pin)?;
        if stamp(&row,root,&file)?!=original
            ||!matches!(fgetxattr(&file,c"security.capability",&mut [0u8;0][..]),Err(Errno::NODATA))
            ||hash_upto(&mut file,row.size,ELF)?!=row.sha{return Err(no());}
        if matches!(row.part,Part::Exec|Part::Code){
            let mut h=[0;20];file.read_exact_at(&mut h,0)?;
            if &h[..7]!=b"\x7fELF\x02\x01\x01"||!matches!(h[16..20],[2,0,62,0]|[3,0,62,0]){return Err(no());}
        }
        file.rewind()?;
        let m=Self{row,original,file};m.require(root)?;Ok(m)
    }
    fn require(&self,root:&File)->io::Result<()>{
        if stamp(&self.row,root,&self.file)?!=self.original||stamp(&self.row,root,&named(root,&self.row.path)?)?!=self.original{return Err(no());}
        Ok(())
    }
}
struct Role{root:File,members:Vec<Member>}
impl Role{
    fn check(&self,which:usize)->io::Result<()>{
        if !super::same(&parents(which)?,&self.root)?{return Err(no());}
        for x in &self.members{x.require(&self.root)?;}
        Ok(())
    }
    fn get(&self,name:&str)->io::Result<&Member>{
        self.members.iter().find(|m|m.row.path==name).ok_or_else(no)
    }
}
pub(super) struct Release{
    policy:RuntimeFile,manifest:RuntimeFile,roles:[Role;2],manifest_sha:Hash,nss:Hash,nss_bytes:[Vec<u8>;4],
}
fn read(f:&RuntimeFile,mode:u32,cap:u64)->io::Result<Vec<u8>>{
    let m=f.file.metadata()?;
    if m.mode()!=(0o100000|mode)||m.gid()!=0||m.uid()!=0||m.nlink()!=1||!(1..=cap).contains(&m.len()){return Err(no());}
    let mut b=Vec::with_capacity(m.len() as usize);(&f.file).take(m.len()+1).read_to_end(&mut b)?;
    if b.len() as u64!=m.len(){return Err(no());}f.recheck()?;Ok(b)
}
impl Release{
    pub(super) fn select()->io::Result<Self>{
        let policy=RuntimeFile::open(POLICY,false,128)?;
        let p=read(&policy,0o600,128)?;
        let limit=std::str::from_utf8(&p).ok().and_then(|p|p.strip_prefix("ZXS-SMB/ONLINE-1 "))
            .and_then(|p|p.strip_suffix('\n')).ok_or_else(no)?;
        let expected=hash(limit)?;
        let manifest=RuntimeFile::open(MANIFEST,false,MAX)?;
        let raw=read(&manifest,0o444,MAX)?;
        if <Hash>::from(Sha256::digest(&raw))!=expected{return Err(no());}
        let (nss,listed)=rows(&raw,frozen_tree()?,Sha256::digest(SCHEMA).into())?;
        let mut chosen=Vec::new();
        for (i,r) in listed.into_iter().enumerate(){
            let root=parents(i)?;
            let members=r.into_iter().map(|v|Member::select(&root,v)).collect::<io::Result<Vec<_>>>()?;
            let role=Role{root,members};
            let raw=role.get(LOADER)?;let alias=role.get(ALIAS)?;
            if raw.row.sha!=alias.row.sha||!super::same(&raw.file,&alias.file)?{return Err(no());}
            role.check(i)?;chosen.push(role);
        }
        let roles:[Role;2]=chosen.try_into().map_err(|_|no())?;
        let mut nss_bytes:[Vec<u8>;4]=std::array::from_fn(|_|Vec::new());
        for (i,n) in NSS.iter().enumerate(){
            let m=roles[0].get(n)?;
            let mut b=vec![0;m.row.size as usize];m.file.read_exact_at(&mut b,0)?;
            let first=Sha256::digest(&b);
            if <Hash>::from(first)!=m.row.sha||m.file.read_at(&mut [0],m.row.size)?!=0{return Err(no());}
            nss_bytes[i]=b;
        }
        let result=Self{policy,manifest,roles,manifest_sha:expected,nss,nss_bytes};result.require()?;Ok(result)
    }
    pub(super) fn require(&self)->io::Result<()>{
        self.policy.recheck()?;self.manifest.recheck()?;
        for (which,r) in self.roles.iter().enumerate(){r.check(which)?;}
        self.policy.recheck()?;self.manifest.recheck()
    }
    pub(super) fn serial(&self)->Hash{self.nss}
    pub(super) fn manifest_sha(&self)->Hash{self.manifest_sha}
    pub(super) fn public(&self)->[Public<'_>;4]{
        std::array::from_fn(|i|Public{path:Path::new(NSS[i]),bytes:&self.nss_bytes[i]})
    }
    pub(super) fn executable(&self,daemon:bool)->io::Result<&File>{
        let i=if daemon{0}else{1};self.require()?;
        Ok(&self.roles[i].get(PROGRAMS[i])?.file)
    }
    pub(super) fn sources(&self,daemon:bool)->Vec<Source<'_>>{
        self.roles[if daemon{0}else{1}].members.iter().filter_map(|m|{
            let access=match m.row.part{Part::Nss=>return None,Part::Public=>Access::Public,_=>Access::Code};
            Some(Source{path:Path::new(&m.row.path),file:&m.file,access})
        }).collect()
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn fixture()->String{
        let sha="03".repeat(32);
        let mut out=format!("ZXS-SMB/ELF-ROLES-1\nsource {}\nschema {}\nnss {}\n","04".repeat(20),"05".repeat(32),sha);
        for (i,name) in NAMES.iter().enumerate(){
            let mut fields=vec![PROGRAMS[i],LOADER,ALIAS];if i==0{fields.extend(NSS);fields.extend(CONVERSION);}
            fields.sort_unstable();
            for p in fields{
                let (_,mode,_,_)=part(i,p).unwrap();
                out+=&format!("{name} {mode:04o} 88 {sha} {p}\n");
            }
        }
        out
    }
    #[test]fn exact_text_matches_only_one_expected_role_closure_and_source(){
        let full=fixture();let tree="04".repeat(20);
        assert!(rows(full.as_bytes(),&tree,[5;32]).is_ok());
        assert!(rows(full.as_bytes(),&"06".repeat(20),[5;32]).is_err());
        assert!(rows(full.as_bytes(),&tree,[6;32]).is_err());
        for (a,b) in [(ALIAS,"etc/ld.so.preload"),(LOADER,"usr/lib/x86_64-linux-gnu/../liba.so"),(NSS[3],"etc/shadow"),
            (PROGRAMS[0],"usr/sbin/smbd"),(PROGRAMS[1],PROGRAMS[0]),(" 88 "," 088 ")]{
            assert!(rows(full.replace(a,b).as_bytes(),&tree,[5;32]).is_err());
        }
    }
}
