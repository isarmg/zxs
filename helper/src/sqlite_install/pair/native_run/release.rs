//! The site's protected local policy binds the exact approved artifact digests
//! and original site before any native closure is used. This is separate from
//! fresh console approval. One fixed private native closure and crypto provider.
use super::{Id,bad};
use crate::{fs_tools::{RuntimeFile,hash_upto},maintenance::thread_file,pinned::{Entity,Kind,LocalDataFile,Mount,Pinned},
    sqlite_install::pair::smb_policy as helpers};
use helper_linux_evidence::{first_smb,native_sam};
use rustix::fs::{Mode,OFlags,RawDir,openat};
use std::{collections::BTreeSet,fs::File,io::{self,Read},mem::MaybeUninit,os::unix::fs::MetadataExt,path::{Path,PathBuf}};

const POLICY:&[&str]=&["etc","local-ops-helper","native-account.policy"];
const HELPER:&[&str]=&["usr","bin","helper"];
const INDEX:&[&str]=&["usr","lib","local-ops-helper","account-native-v7","ELF-INDEX-1"];
const GCRYPT:&[&str]=&["usr","lib","local-ops-helper","disk-tools-v1","lib","libgcrypt.so.20"];
const GPGERR:&[&str]=&["usr","lib","local-ops-helper","disk-tools-v1","lib","libgpg-error.so.0"];
const ROOT:&str="/usr/lib/local-ops-helper/account-native-v7";
const NATIVE:&str="helper-account-native.so";
const LIBC:&str="/usr/lib/local-ops-helper/disk-tools-v1/lib/libc.so.6";
const LOADER:&str="/usr/lib/local-ops-helper/disk-tools-v1/lib/ld-linux-x86-64.so.2";
const MAX_INDEX:u64=48<<10;
const MAX_MAPS:u64=256<<10;
struct Rule{pool:Id,instance:Id,provision:Id,live:[u8;32],helper:[u8;32],index:[u8;32],gcrypt:[u8;32],gpg:[u8;32]}
impl Rule{
    fn parse(raw:&str)->io::Result<Self>{
        let mut w=raw.strip_suffix('\n').ok_or_else(fail)?.split(' ');
        if w.next()!=Some("SITE-NATIVE-NTLMV2-1"){return Err(fail());}
        let mut take=||w.next().ok_or_else(fail);
        let r=Self{pool:helpers::lower(take()?)?,instance:helpers::lower(take()?)?,provision:helpers::lower(take()?)?,
            live:helpers::lower(take()?)?,helper:helpers::lower(take()?)?,index:helpers::lower(take()?)?,
            gcrypt:helpers::lower(take()?)?,gpg:helpers::lower(take()?)?};
        if w.next().is_some()||[r.pool,r.instance,r.provision].contains(&[0;16]){return Err(fail());}Ok(r)
    }
    fn require_site(&self,pool:Id,instance:Id,provision:Id,live:[u8;32])->io::Result<()>{
        if (self.pool,self.instance,self.provision,self.live)!=(pool,instance,provision,live){return Err(fail());}
        Ok(())
    }
}
fn fail()->io::Error{bad("no verified fixed native-account release and site permission for this original instance")}
fn plain_name(n:&str)->bool{
    !n.is_empty()&&n.len()<=150&&n.ends_with(".so")&&n.bytes().all(|c|c.is_ascii_alphanumeric()||b"._-".contains(&c))
        &&!n.starts_with('.')&&n!="libc.so"&&n!="ld-linux-x86-64.so"
}
fn parse_index(b:&str)->io::Result<Vec<(PathBuf,[u8;32])>>{
    if b.len()>MAX_INDEX as usize||!b.ends_with('\n'){return Err(fail());}
    let mut iter=b.lines();
    if iter.next()!=Some("NATIVE-ELF-1"){return Err(fail());}
    let mut v=Vec::new();let mut prev="";
    for l in iter{
        let (name,value)=l.split_once(' ').ok_or_else(fail)?;
        if name<=prev||v.len()>=120{return Err(fail());}prev=name;
        let path=match name{
            "@libc"=>PathBuf::from(LIBC),"@loader"=>PathBuf::from(LOADER),
            n if plain_name(n)=>Path::new(ROOT).join(n),_=>return Err(fail()),
        };
        v.push((path,helpers::lower(value)?));
    }
    if v.len()<4||!v.iter().any(|(p,_)|p==Path::new(native_sam::PATH))
        ||!v.iter().any(|(p,_)|p==Path::new(LIBC))||!v.iter().any(|(p,_)|p==Path::new(LOADER)){
        return Err(fail());
    }Ok(v)
}
fn pos(f:&File)->io::Result<(Entity,u64)>{
    Ok((Entity::of(&f.metadata()?)?,Mount::on(f)?.ok_or_else(fail)?.id))
}
struct Member{path:PathBuf,pin:LocalDataFile,file:File,original:(Entity,u64),length:u64,mode:u32,times:(i64,i64,i64,i64)}
impl Member{
    fn open(path:PathBuf,expected:[u8;32],dir:Option<(Entity,u64)>)->io::Result<Self>{
        let pin=LocalDataFile::select(&path)?;
        let m=pin.metadata()?;
        let mode=m.mode();let length=m.len();
        if ![0o100644,0o100755].contains(&mode)||dir.is_some()&&mode!=0o100644
            ||m.uid()!=0||m.gid()!=0||m.nlink()!=1||!(1..=64<<20).contains(&length){
            return Err(fail());
        }
        let file=pin.reader()?;let original=pos(&file)?;
        if dir.is_some_and(|d|original.1!=d.1||original.0.dev!=d.0.dev)||original.0!=Entity::of(&m)?{
            return Err(fail());
        }
        let mut read=&file;
        let times=(m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec());
        let sha=hash_upto(&mut read,length,64<<20)?;
        if sha!=expected{return Err(fail());}
        let s=Self{path,pin,file,original,length,mode,times};s.require()?;Ok(s)
    }
    fn require(&self)->io::Result<()>{
        self.pin.require_same_path()?;
        let m=self.file.metadata()?;
        if pos(&self.file)?!=self.original||m.len()!=self.length||(m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec())!=self.times||m.mode()!=self.mode||m.nlink()!=1||m.uid()!=0||m.gid()!=0{
            return Err(fail());
        }Ok(())
    }
}
pub(super) struct Release{
    policy:RuntimeFile,helper:RuntimeFile,index:RuntimeFile,gcrypt:RuntimeFile,gpg:RuntimeFile,
    dir:Pinned,files:Vec<Member>,
}
impl Release {
    pub(super) fn authorize(pool:Id,instance:Id,provision:Id,live:[u8;32])->io::Result<Self>{
        if !helpers::clean_environment()||std::env::var_os("TDB_NO_FSYNC").is_some()
            ||std::env::var_os("TDB_TRACE").is_some(){
            return Err(fail());
        }
        match std::fs::symlink_metadata("/etc/ld.so.preload"){
            Err(e) if e.kind()==io::ErrorKind::NotFound=>(),_=>return Err(fail()),
        }
        let policy=RuntimeFile::open(POLICY,false,512)?;
        helpers::mode(&policy.file,0o600)?;
        let mut text=String::new();(&policy.file).take(513).read_to_string(&mut text)?;
        let rule=Rule::parse(&text)?;policy.recheck()?;
        rule.require_site(pool,instance,provision,live)?;
        let helper=helpers::release(HELPER,0o755,&rule.helper)?;helper.require_running_helper()?;
        let gcrypt=helpers::release(GCRYPT,0o644,&rule.gcrypt)?;
        let gpg=helpers::release(GPGERR,0o644,&rule.gpg)?;
        let mut index=RuntimeFile::open(INDEX,false,MAX_INDEX)?;
        helpers::mode(&index.file,0o444)?;
        if index.digest()?!=rule.index{return Err(fail());}
        let mut raw=String::new();(&index.file).take(MAX_INDEX+1).read_to_string(&mut raw)?;
        let directory=Pinned::open(Path::new(ROOT),Kind::StorageRoot)?;
        let d=directory.storage_directory()?;
        let loc=pos(&d)?;
        if pos(&index.file)?.1!=loc.1||pos(&index.file)?.0.dev!=loc.0.dev{return Err(fail());}
        let rows=parse_index(&raw)?;
        let total=rows.iter().map(|(p,h)|{
            let parent=(p.parent()==Some(Path::new(ROOT))).then_some(loc);
            Member::open(p.to_owned(),*h,parent)
        }).collect::<io::Result<Vec<_>>>()?;
        if total.iter().map(|f|f.length).sum::<u64>()>512<<20{return Err(fail());}
        let s=Self{policy,helper,index,gcrypt,gpg,dir:directory,files:total};
        s.require()?;Ok(s)
    }
    pub(super) fn helper(&self)->&File{&self.helper.file}
    pub(super) fn reads(&self)->Vec<&File>{
        let mut v=vec![&self.policy.file,&self.helper.file,&self.index.file,&self.gcrypt.file,&self.gpg.file];
        v.extend(self.files.iter().map(|f|&f.file));v
    }
    pub(super) fn require(&self)->io::Result<()>{
        helpers::system_policy()?;
        self.helper.require_running_helper()?;
        for f in [&self.policy,&self.index,&self.gcrypt,&self.gpg]{f.recheck()?;}
        let dir=self.dir.storage_directory()?;let m=dir.metadata()?;
        if m.uid()!=0||m.gid()!=0||m.mode()!=0o040755{return Err(fail());}
        let mut expected=BTreeSet::from([b"ELF-INDEX-1".to_vec()]);
        for f in &self.files{
            f.require()?;
            if f.path.parent()==Some(Path::new(ROOT)){
                expected.insert(f.path.file_name().ok_or_else(fail)?.as_encoded_bytes().to_vec());
            }
        }
        let mut buf=[MaybeUninit::uninit();4096];let mut rows=RawDir::new(&dir,&mut buf);
        for _ in 0..=120+2 {
            let Some(e)=rows.next()else{
                if expected.is_empty(){return self.dir.require_same_path();}return Err(fail());
            };
            let e=e?;let n=e.file_name().to_bytes();
            if n==b"."||n==b".."{continue;}
            if !expected.remove(n){return Err(fail());}
        }Err(fail())
    }
    pub(super) fn load(&self)->io::Result<(native_sam::Library,first_smb::Libraries)>{
        self.require()?;
        let native=native_sam::Library::load()?;
        let crypto=first_smb::Libraries::load()?;
        self.loaded(&native,&crypto)?;
        Ok((native,crypto))
    }
    pub(super) fn loaded(&self,native:&native_sam::Library,crypto:&first_smb::Libraries)->io::Result<()>{
        self.require()?;
        let actual=native.linked_paths()?.into_iter().map(|(b,ld)|std::str::from_utf8(&b)
            .map(|p|(PathBuf::from(p),ld)).map_err(|_|fail())).collect::<io::Result<Vec<_>>>()?;
        let expected=self.files.iter().map(|f|f.path.clone()).collect::<BTreeSet<_>>();
        if actual.len()!=expected.len()||actual.iter().map(|(p,_)|p.clone()).collect::<BTreeSet<_>>()!=expected{
            return Err(fail());
        }
        let mapping=self.files.iter().find(|f|f.path==Path::new(native_sam::PATH)).ok_or_else(fail)?;
        let mut raw=String::new();thread_file(c"thread-self/maps")?.take(MAX_MAPS+1).read_to_string(&mut raw)?;
        for (p,address) in actual{
            let member=self.files.iter().find(|f|f.path==p).ok_or_else(fail)?;
            helpers::segments(&raw,&[address],&helpers::actual(p.to_str().ok_or_else(fail)?,&member.file)?,false)?;
        }
        helpers::matched(&raw,&native.entry_sites(),&helpers::actual(native_sam::PATH,&mapping.file)?)?;
        // The support of the separate mature crypto provider has its own
        // positive mapping checks (not counted as native Samba libraries).
        helpers::mappings(&self.gcrypt.file,&self.gpg.file,crypto)?;
        self.require()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]fn local_approval_closure_can_only_supply_the_fixed_private_names_and_base_glibc(){
        let h="11".repeat(32);
        let valid=format!("NATIVE-ELF-1\n@libc {h}\n@loader {h}\nhelper-account-native.so {h}\nnative-lib0001.so {h}\n");
        assert_eq!(parse_index(&valid).unwrap().len(),4);
        for x in [valid.replace("native-lib0001.so","../escape.so"),valid.replace("@loader","@other"),valid.replace("@loader","@libc"),valid.trim_end().to_owned()]{
            assert!(parse_index(&x).is_err());
        }
    }
    #[test]fn local_approval_native_account_requires_exact_site_and_all_four_digests(){
        let fields=["01".repeat(16),"02".repeat(16),"03".repeat(16),"04".repeat(32),
            "05".repeat(32),"06".repeat(32),"07".repeat(32),"08".repeat(32)];
        let text=format!("SITE-NATIVE-NTLMV2-1 {}\n",fields.join(" "));
        let rule=Rule::parse(&text).unwrap();
        assert_eq!([rule.helper,rule.index,rule.gcrypt,rule.gpg],[[5;32],[6;32],[7;32],[8;32]]);
        assert!(rule.require_site([1;16],[2;16],[3;16],[4;32]).is_ok());
        for (p,i,d,l) in [([9;16],[2;16],[3;16],[4;32]),([1;16],[9;16],[3;16],[4;32]),
            ([1;16],[2;16],[9;16],[4;32]),([1;16],[2;16],[3;16],[9;32])]{
            assert!(rule.require_site(p,i,d,l).is_err());
        }
        for wrong in [String::new(),text.trim_end().to_owned(),text.replace(&format!(" {}",fields[7]),""),
            format!("{text}{text}")]{assert!(Rule::parse(&wrong).is_err());}
    }
}
