//! The approved exact boot plan and its original committed setup receipts.
//! Decoding a plan never creates an SMB session or a directory handle.
use super::{Connection,InitialReader,denied};
use super::super::{Catalog,check,credential::Pair,share_mode::{self,Mode,Setting},sqlite3_step};
use crate::{OperationId,Role,VolumeId,display,fs::invalid,hash::{Sha256,digest},identity::ShareId};
use std::{collections::BTreeSet,io,net::{IpAddr,Ipv4Addr,Ipv6Addr}};

const MAGIC:&[u8;16]=b"ZXS-SMB-FIRST-1\0";
pub(super) const PLAN_SLOT:usize=17631;
const MAX_WIRE:usize=PLAN_SLOT;
type Id=[u8;16];

#[derive(Clone,Copy,Eq,Ord,PartialEq,PartialOrd)]
pub(super) struct Endpoint{pub(super) ip:IpAddr,pub(super) prefix:u8}
impl Endpoint{
    fn new(ip:IpAddr,prefix:u8)->io::Result<Self>{
        match ip{
            IpAddr::V4(a) if a.octets()[0]!=0&&!a.is_loopback()&&!a.is_multicast()&&!a.is_broadcast()&&
                (1..=32).contains(&prefix)=>{
                    if prefix<31{
                        let mask=u32::MAX>>prefix;
                        if matches!(u32::from(a)&mask,0) || u32::from(a)&mask==mask{return Err(denied());}
                    }
                },
            IpAddr::V6(a) if !a.is_unspecified()&&!a.is_loopback()&&!a.is_multicast()&&!a.is_unicast_link_local()&&
                a.to_ipv4().is_none()&&(1..=128).contains(&prefix)=>{},
            _=>return Err(denied()),
        }
        Ok(Self{ip,prefix})
    }
}
pub(super) struct Folder{
    name:String,volume:VolumeId,share:ShareId,publish:OperationId,first:OperationId,mode:Mode,
}
impl Folder{
    pub(super) fn name(&self)->&str{&self.name}
    pub(super) fn share(&self)->ShareId{self.share}
    pub(super) fn volume(&self)->VolumeId{self.volume}
}

pub(super) struct Plan{
    scope:[Id;5], sha:[u8;32], endpoints:Vec<Endpoint>, folders:Vec<Folder>, recycle:String,
}
pub(super) struct InstalledPlan(Plan);
impl Plan{
    pub(super) fn parse(raw:&[u8])->io::Result<Self>{
        fn take<'a>(p:&mut &'a[u8],len:usize)->io::Result<&'a[u8]>{
            let (out,rest)=p.split_at_checked(len).ok_or_else(denied)?;*p=rest;Ok(out)
        }
        fn fixed<const N:usize>(p:&mut &[u8])->io::Result<[u8;N]>{
            Ok(take(p,N)?.try_into().expect("exact requested length"))
        }
        let mut p=raw.strip_prefix(MAGIC).filter(|_|raw.len()<=MAX_WIRE).ok_or_else(denied)?;
        let sha=digest(raw)?;
        let mut seen=BTreeSet::new();
        let mut scope=[[0u8;16];5];
        for id in &mut scope{
            *id=fixed(&mut p)?;
            if *id==[0;16]||!seen.insert(*id){return Err(denied());}
        }
        let [n]=fixed(&mut p)?;
        if !(1..=16).contains(&n){return Err(denied());}
        let mut endpoints=Vec::<Endpoint>::with_capacity(n as usize);
        for _ in 0..n{
            let [family,prefix]=fixed(&mut p)?;
            let ip=match family{
                4=>IpAddr::V4(Ipv4Addr::from(fixed::<4>(&mut p)?)),
                6=>IpAddr::V6(Ipv6Addr::from(fixed::<16>(&mut p)?)),
                _=>return Err(denied()),
            };
            let e=Endpoint::new(ip,prefix)?;
            if endpoints.last().is_some_and(|old|old.ip>=e.ip){return Err(denied());}
            endpoints.push(e);
        }
        let [n]=fixed(&mut p)?;
        if !(1..=62).contains(&n){return Err(denied());}
        let mut folders=Vec::<Folder>::with_capacity(n as usize);
        let mut names=BTreeSet::new();
        for _ in 0..n {
            let id=fixed(&mut p)?;
            if id==[0;16]||folders.last().is_some_and(|f|f.volume.0>=id)||!seen.insert(id){return Err(denied());}
            let mode=match fixed(&mut p)?{[1]=>Mode::ReadOnly,[2]=>Mode::ReadWrite,_=>return Err(denied())};
            let k=u16::from_be_bytes(fixed(&mut p)?) as usize;
            let name=display::component(take(&mut p,k)?,display::MAX)?;
            if !names.insert(name){return Err(denied());}
            let next=|kind|{
                let mut h=Sha256::new();h.update(b"ZXS-SMB-FIRST-1/ID\0")?;h.update(&sha)?;
                h.update(&[kind])?;h.update(&id)?;
                Ok::<Id,io::Error>(h.finish()[..16].try_into().expect("derive 16B"))
            };
            folders.push(Folder{name:name.to_owned(),volume:VolumeId(id),mode,share:ShareId(next(1)?),
                publish:OperationId(next(2)?),first:OperationId(next(3)?)});
        }
        let k=u16::from_be_bytes(fixed(&mut p)?) as usize;
        let recycle=display::component(take(&mut p,k)?,display::MAX)?;
        if names.contains(recycle)||!p.is_empty()||sha==[0;32]{return Err(denied());}
        for f in &folders{
            for id in [f.share.0,f.publish.0,f.first.0]{
                if id==[0;16]||!seen.insert(id){return Err(denied());}
            }
        }
        Ok(Self{scope,sha,endpoints,folders,recycle:recycle.to_owned()})
    }
    pub(super) fn sha(&self)->[u8;32]{self.sha}
    pub(super) fn endpoints(&self)->&[Endpoint]{&self.endpoints}
    pub(super) fn folders(&self)->&[Folder]{&self.folders}
    pub(super) fn install(self,src:&InitialReader,bound:&[u8;128])->io::Result<InstalledPlan>{
        if self.scope[0]!=bound[16..32]||self.scope[2]!=bound[32..48]||self.scope[3]!=bound[48..64]{
            return Err(denied());
        }
        let bound_sha=digest(bound)?;
        src.at(|db,c|self.qualify_native_at(db,&c,bound_sha).map(drop))?;
        Ok(InstalledPlan(self))
    }
    /// Qualify only the native installation baseline inside the caller's
    /// original SYSTEM read cut. The BOUND digest comes from the already
    /// checked public BOUND above. This is not a verifier or an SMB grant.
    /// A reader needing the active epoch can use this same Pair before leaving
    /// the cut; native plan installation still requires only the original rows.
    fn qualify_native_at(&self,db:&Connection,c:&Catalog,bound_sha:[u8;32])->io::Result<Pair>{
        let pair=Pair::confirmed(db,c)?.ok_or_else(denied)?;
        if self.scope!=[c.birth.pool.0,c.birth.op.0,pair.instance,self.scope[3],pair.setup.0]||
            pair.setup_bound!=bound_sha{return Err(denied());}
        if !c.rows().get(1).is_some_and(|r|r.ordinal==1&&r.role==Role::Recycle){return Err(denied());}
        for f in self.folders(){self.original(db,c,&pair,f)?;}
        Ok(pair)
    }
    fn original(&self,db:&Connection,c:&Catalog,pair:&Pair,f:&Folder)->io::Result<()>{
        if !c.rows().iter().any(|v|v.role==Role::File&&v.volume==f.volume){return Err(denied());}
        let prefix=|domain:&[u8],op:OperationId|->io::Result<Sha256>{
            let mut h=Sha256::new();h.update(domain)?;
            for v in [c.birth.pool.0,op.0,pair.web.0,pair.setup.0]{h.update(&v)?;}
            h.update(&1i64.to_be_bytes())?;Ok(h)
        };
        let mut pub_sha=prefix(b"ZXS-SQLITE-1/PUBLISH-ROOT\0",f.publish)?;
        pub_sha.update(&f.volume.0)?;pub_sha.update(&(f.name.len() as u16).to_be_bytes())?;pub_sha.update(f.name.as_bytes())?;
        let mut first_sha=prefix(b"ZXS-SQLITE-1/SHARE-MODE\0",f.first)?;
        first_sha.update(&f.share.0)?;first_sha.update(&f.publish.0)?;first_sha.update(&[f.mode as u8])?;
        let s=db.prepare(c"SELECT p.kind,p.state,p.actor_id,p.request_sha,r.share_id,r.author_auth_op,r.volume_id,r.display_label,o.kind,o.state,o.actor_id,o.request_sha,m.share_id,m.previous_op,m.author_auth_op,m.revision,m.mode FROM main.root_share r JOIN main.operation p ON p.op_id=r.publish_op JOIN main.share_mode m ON m.share_id=r.share_id AND m.revision=1 JOIN main.operation o ON o.op_id=m.change_op WHERE r.publish_op=?1 AND m.change_op=?2")?;
        s.bind_bytes(1,&f.publish.0)?;s.bind_bytes(2,&f.first.0)?;
        match unsafe{sqlite3_step(s.0)}{100=>{},101=>return Err(denied()),rc=>{check(rc)?;return Err(invalid("invalid original planned share receipt"));}}
        if s.integer(0)?!=3||s.integer(1)?!=1||s.array::<16>(2)?!=pair.web.0||s.array::<32>(3)?!=pub_sha.finish()||
            s.array::<16>(4)?!=f.share.0||s.array::<16>(5)?!=pair.setup.0||s.array::<16>(6)?!=f.volume.0||
            s.blob(7,display::MAX)?!=f.name.as_bytes()||s.integer(8)?!=4||s.integer(9)?!=1||
            s.array::<16>(10)?!=pair.web.0||s.array::<32>(11)?!=first_sha.finish()||s.array::<16>(12)?!=f.share.0||
            s.array::<16>(13)?!=f.publish.0||s.array::<16>(14)?!=pair.setup.0||s.integer(15)?!=1||s.byte(16)?!=f.mode as u8{
            return Err(denied());
        }
        s.done()
    }
}

impl InstalledPlan{
    pub(super) fn sha(&self)->[u8;32]{self.0.sha()}
    pub(super) fn folders(&self)->&[Folder]{self.0.folders()}
    pub(super) fn retained_label(&self)->&str{&self.0.recycle}
    /// Logical children in exact display-byte order, strictly after a cursor.
    /// Caller must have rechecked the original live SMB subject in this same
    /// SQL cut and validated the cursor; these rows carry no OPEN or grant.
    /// A zero-length cursor selects the complete bounded namespace. No volume
    /// is opened: the installed R label is not evidence that its medium is up.
    pub(super) fn project_visible_after<'p>(&'p self,db:&Connection,c:&Catalog,left:&[u8])
        ->io::Result<Vec<(&'p [u8],u64)>>{
        let mut matches=Vec::new();
        let retained=self.retained_label().as_bytes();
        if retained>left{matches.push((retained,2));}
        for f in self.folders(){
            let name=f.name().as_bytes();
            if name<=left{continue;}
            let now=self.current(db,c,f.name())?.ok_or_else(denied)?;
            if now.mode==Mode::Disabled{continue;}
            let v=c.rows().iter().find(|v|v.role==Role::File&&v.volume==f.volume()).ok_or_else(denied)?;
            matches.push((name,u64::from(v.ordinal)+1));
        }
        matches.sort_unstable_by_key(|r|r.0);
        Ok(matches)
    }
    // The original first mode is not the current permission. A later
    // action still selects the live authenticated session/grant independently.
    pub(super) fn current(&self,db:&Connection,c:&Catalog,label:&str)->io::Result<Option<Setting>>{
        let Some(f)=self.folders().iter().find(|f|f.name()==label)else{return Ok(None);};
        let now=share_mode::current(db,c,f.share)?;
        if now.volume!=f.volume||now.revision<1{return Err(denied());}
        Ok(Some(now))
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use super::super::hex;
    #[test]
    fn complete_first_plan_has_exact_helper_digest_and_separate_case_labels(){
        let entries=include_str!("first-ordinary-plan-v1.txt");
        let read=|key|entries.lines().filter_map(|x|x.split_once('=')).find(|(k,_)|*k==key).unwrap().1;
        let mut raw=hex::<179>(read("wire_hex")).unwrap();
        let p=Plan::parse(&raw).unwrap();
        assert_eq!(p.scope,[[1;16],[2;16],[3;16],[4;16],[5;16]]);
        assert_eq!(p.sha(),hex(read("plan_sha256")).unwrap());
        assert_eq!((p.endpoints[0].ip,p.endpoints[0].prefix),("192.0.2.17".parse().unwrap(),24));
        assert_eq!((p.endpoints[1].ip,p.endpoints[1].prefix),("2001:db8:7::2".parse().unwrap(),64));
        assert_eq!(p.folders[0].name(),"Docs");assert_eq!(p.folders[1].name(),"docs");
        assert_eq!(p.folders[0].share.0,hex(read("f1_share")).unwrap());
        assert_eq!(p.folders[0].publish.0,hex(read("f1_publish")).unwrap());
        assert_eq!(p.folders[0].first.0,hex(read("f1_mode")).unwrap());
        assert_eq!(p.folders[1].share.0,hex(read("f2_share")).unwrap());
        assert_eq!(p.folders[1].publish.0,hex(read("f2_publish")).unwrap());
        assert_eq!(p.folders[1].first.0,hex(read("f2_mode")).unwrap());
        assert_eq!(p.recycle,"回收站");
        assert!(Plan::parse(&raw[..178]).is_err());
        raw[168..170].copy_from_slice(&[0,0]);
        assert!(Plan::parse(&raw[..170]).is_err()); // old ordinary-only ABI is not v10
        raw[168..170].copy_from_slice(&[0,4]);raw[170..174].copy_from_slice(b"Docs");
        assert!(Plan::parse(&raw[..174]).is_err()); // exact name already routes to F
    }
}
