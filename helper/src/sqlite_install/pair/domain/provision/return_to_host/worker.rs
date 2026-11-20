//! Only the fixed, exec'd child of the confirmed foreground host helper.
//! A sealed nonsensitive description is necessary, never sufficient authority.
use super::{HostOrigins,order};
use super::super::{capsule,FreshApproval,CAPSULE_USR};
use crate::{pinned::{Entity,Kind,Pinned},sqlite_install::{approval,bad,originals::SelectedSystem,request::Id}};
use rustix::{fs::{SealFlags,fcntl_get_seals},io::fcntl_dupfd_cloexec,net::{sockopt,AddressFamily,SocketType},
    process::{getppid,parent_process_death_signal,set_parent_process_death_signal,set_dumpable_behavior,dumpable_behavior,DumpableBehavior,Pid,Signal}};
use std::{fs::File,io,os::unix::{ffi::OsStrExt,fs::{FileExt,MetadataExt}},path::{Path,PathBuf}};
const DOMAIN:&[u8]=b"HELPER/SAM-HOST-ORDER/2\0";
const FIXED:usize=DOMAIN.len()+7*16+2*4+3*32+6*24+2;
const MAX:usize=FIXED+crate::path_rules::MAX_PATH_BYTES;
type Objects=[(Entity,u64);6];

struct Order {
    ids:[Id;6],setup:Id,uid:u32,gid:u32,config:[u8;32],scope:[u8;32],launcher:[u8;32],objects:Objects,path:PathBuf,
}
fn bad_order()->io::Error{bad("not one complete sealed order from the original foreground host")}
impl Order{
    fn encode(&self)->io::Result<Vec<u8>>{
        let p=self.path.as_os_str().as_bytes();Kind::StorageRoot.check_path(p).map_err(bad)?;
        if self.path.to_str().is_none(){return Err(bad_order());}
        let mut out=Vec::with_capacity(FIXED+p.len());out.extend_from_slice(DOMAIN);
        for id in self.ids.iter().chain([&self.setup]){out.extend_from_slice(id);}
        for n in [self.uid,self.gid]{out.extend_from_slice(&n.to_be_bytes());}
        for d in [&self.config,&self.scope,&self.launcher]{out.extend_from_slice(d);}
        for (e,m) in self.objects{for n in [e.dev,e.ino,m]{out.extend_from_slice(&n.to_be_bytes());}}
        out.extend_from_slice(&(p.len() as u16).to_be_bytes());out.extend_from_slice(p);Ok(out)
    }
    fn decode(data:&[u8])->io::Result<Self>{
        if !(FIXED+1..=MAX).contains(&data.len()){return Err(bad_order());}
        let mut b=data.strip_prefix(DOMAIN).ok_or_else(bad_order)?;
        // FIXED includes exactly every following fixed field. No wire length
        // is used for indexing or allocation before proving this bound.
        fn take<const N:usize>(b:&mut &[u8])->[u8;N]{
            let (part,rest)=b.split_at(N);*b=rest;part.try_into().expect("validated fixed envelope")
        }
        let ids:[Id;6]=std::array::from_fn(|_|take(&mut b));let setup=take(&mut b);let uid=u32::from_be_bytes(take(&mut b));let gid=u32::from_be_bytes(take(&mut b));
        let config=take(&mut b);let scope=take(&mut b);let launcher=take(&mut b);
        let objects=std::array::from_fn(|_|{
            let dev=u64::from_be_bytes(take(&mut b));let ino=u64::from_be_bytes(take(&mut b));
            (Entity{dev,ino},u64::from_be_bytes(take(&mut b)))
        });
        let n=usize::from(u16::from_be_bytes(take(&mut b)));
        if ids.contains(&[0;16])||setup==[0;16]||ids[..4].contains(&setup)||![uid,gid].iter().all(|v|(1..u32::MAX).contains(v))
            ||objects.iter().any(|(e,m)|e.ino==0||*m==0)||n!=b.len(){return Err(bad_order());}
        Kind::StorageRoot.check_path(b).map_err(bad)?;
        let path=PathBuf::from(std::str::from_utf8(b).map_err(|_|bad_order())?);
        Ok(Self{ids,setup,uid,gid,config,scope,launcher,objects,path})
    }
    fn select(&self,path:&Path,kind:Kind,index:usize)->io::Result<Pinned>{
        let held=Pinned::open(path,kind)?;
        if (held.entity(),held.mount()?.id)!=self.objects[index]{return Err(bad("requested host original changed; no data open"));}
        Ok(held)
    }
}
pub(super) fn sealed_request(host:&HostOrigins,reference:Id,scope:[u8;32])->io::Result<File>{
    let w=Order{ids:[host.pool,host.init,host.instance,host.op,host.actor,reference],setup:host.setup,uid:host.uid,gid:host.gid,
        config:host.config,scope,launcher:host.release.original_launcher(),objects:host.marks()?,path:host.system.path().into()};
    super::super::sealed(&w.encode()?)
}
fn read_input(input:&File)->io::Result<Order>{
    let st=input.metadata()?;
    let must=SealFlags::WRITE|SealFlags::GROW|SealFlags::SHRINK|SealFlags::SEAL|SealFlags::EXEC;
    if !st.is_file()||st.uid()!=0||st.gid()!=0||st.nlink()!=0||st.mode()&0o111!=0
        ||!(FIXED as u64+1..=MAX as u64).contains(&st.len())||!fcntl_get_seals(input)?.contains(must){return Err(bad_order());}
    let mut buf=vec![0u8;st.len() as usize];input.read_exact_at(&mut buf,0)?;
    loop{match input.read_at(&mut [0],st.len()){
        Ok(0)=>break,Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,_=>return Err(bad_order()),
    }}
    Order::decode(&buf)
}
fn require_parent(out:&impl rustix::fd::AsFd,pid:Pid)->io::Result<()>{
    if sockopt::socket_domain(out)?!=AddressFamily::UNIX||sockopt::socket_type(out)?!=SocketType::SEQPACKET||
        parent_process_death_signal()?!=Some(Signal::TERM)||getppid()!=Some(pid){return Err(bad_order());}
    let who=sockopt::socket_peercred(out)?;
    if who.pid!=pid||!who.uid.is_root()||!who.gid.is_root()||getppid()!=Some(pid){return Err(bad_order());}
    Ok(())
}
/// Zero CLI options. stdin is the non-executable sealed order; stdout is its
/// own parent's private seqpacket endpoint. No target is looked up as root.
pub(crate) fn exec_child()->io::Result<()>{
    approval::no_inherited_descriptors()?;
    let pid=getppid().ok_or_else(bad_order)?;require_parent(&io::stdout(),pid)?;
    let host=approval::host_admin()?;let (actor,uid,gid)=approval::profile()?;
    // Prove original parent AND self map the actually enrolled helper ELF
    // while root can inspect its parent. Return only the public hash values.
    let keys=capsule::worker_keys(pid)?;
    require_parent(&io::stdout(),pid)?;
    approval::drop_to_storage(uid,gid)?;
    // Linux clears PDEATHSIG on credential transitions. No storage access
    // or parsing can happen between this transition and the independent recheck.
    set_parent_process_death_signal(Some(Signal::TERM))?;
    require_parent(&io::stdout(),pid)?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    if dumpable_behavior()?!=DumpableBehavior::NotDumpable{return Err(bad_order());}
    let input=File::from(fcntl_dupfd_cloexec(io::stdin(),3)?);
    let out=File::from(fcntl_dupfd_cloexec(io::stdout(),3)?);let w=read_input(&input)?;drop(input);
    if (w.ids[4],w.uid,w.gid)!=(actor,uid,gid)||
        order(w.ids.each_ref(),&w.setup,uid,gid,&w.path,&w.config,&keys.bind(&w.launcher),w.objects)?!=w.scope{return Err(bad_order());}
    // The sealed declaration and SO_PEERCRED describe originals, not the
    // continued participation of the approving parent. Require its fresh
    // post-drop response BEFORE resolving even the operator-selected paths.
    super::start::child(&out,pid,&w.scope,||require_parent(&out,pid))?;
    let [pool,init,instance_id,op,_,reference]=w.ids;
    let base=Path::new("/var/lib/local-ops-helper/samba-instances").join(super::issue::hex_id(&instance_id));
    let site=w.select(&base,Kind::StorageRoot,1)?;
    // Recognize all six simultaneously held originals with O_PATH before
    // opening any selected root for data/locking or querying the original SQL.
    let s=w.select(&w.path,Kind::StorageRoot,0)?;
    let private=w.select(&base.join("private"),Kind::StorageRoot,2)?;
    let state=w.select(&base.join("state"),Kind::StorageRoot,3)?;
    let live=w.select(&base.join("smb.conf"),Kind::Regular,4)?;
    let usr=w.select(Path::new(CAPSULE_USR),Kind::StorageRoot,5)?;
    require_parent(&out,pid)?;
    let system=SelectedSystem::from_origin(s,host,uid,gid)?.lock()?;
    for p in [&site,&private,&state,&live,&usr]{p.require_same_named_mount()?;}
    require_parent(&out,pid)?;
    FreshApproval{system:&system,pool,init,instance_id,op,setup:w.setup,actor,reference,live_config:w.config,host_order:w.scope,
        site:&site,usr:&usr,expected_bwrap:w.launcher,release:keys,uid,gid}.provision()?.send_to_host(&out)
}

#[cfg(test)]
mod tests{
    use super::*;
    fn example()->Order{Order{ids:[[1;16],[2;16],[3;16],[4;16],[5;16],[6;16]],setup:[7;16],uid:1001,gid:120,
        config:[4;32],scope:[9;32],launcher:[15;32],objects:std::array::from_fn(|i|(Entity{dev:1,ino:i as u64+10},20)),path:"/private/system".into()}}
    #[test]fn bounded_order_refuses_an_alternate_path_encoding_extra_byte_or_truncated_objects(){
        let a=example();let b=a.encode().unwrap();let got=Order::decode(&b).unwrap();
        assert_eq!(got.ids,a.ids);assert_eq!(got.setup,a.setup);assert_eq!(got.objects,a.objects);assert_eq!(got.path,a.path);
        for n in [0,1,FIXED-1,FIXED,b.len()-1]{assert!(Order::decode(&b[..n]).is_err());}
        let mut extended=b.clone();extended.push(0);assert!(Order::decode(&extended).is_err());
        let mut no_id=b.clone();no_id[DOMAIN.len()..DOMAIN.len()+16].fill(0);assert!(Order::decode(&no_id).is_err());
        let mut other=b;other[FIXED]=0xff;assert!(Order::decode(&other).is_err());
    }
    #[test]fn naming_the_same_original_after_replacement_cannot_pass_the_sealed_selection(){
        let d=tempfile::tempdir().unwrap();let target=d.path().join("target");std::fs::create_dir(&target).unwrap();
        let orig=Pinned::open(&target,Kind::StorageRoot).unwrap();let mut w=example();w.objects[0]=(orig.entity(),orig.mount().unwrap().id);
        assert!(w.select(&target,Kind::StorageRoot,0).is_ok());
        std::fs::rename(&target,d.path().join("original")).unwrap();std::fs::create_dir(&target).unwrap();
        assert!(w.select(&target,Kind::StorageRoot,0).is_err());assert!(d.path().join("original").is_dir());
    }
}
