//! One read-only observation of the original ZXS-owned deployment. The
//! service and its media are not entered; only public credential metadata
//! can leave the lowered original SYSTEM reader.
use super::{Channel,Kind,PublicConfirmation,bad,observed,place,query_with,random,sealed,sealed_owner};
use crate::{smb_profile::rust_first::{MAX_WIRE,RustPlan},sqlite_install::{markers::SystemObservation,originals::LockedSystem,pair::{
    domain::DeployedSite,fresh_issuer::{ExplicitSubject,InitialDatabase,MAX_SITE,SitePayload,compiled_v18,image}
}}};
use rustix::process::{Pid,getppid};
use sha2::{Digest,Sha256};
use std::{io,path::Path};

pub(super) const DOMAIN:&[u8]=b"helper original ZXS deployment observation 1\0";
const CAP:usize=DOMAIN.len()+32+24+2+MAX_SITE+2+MAX_WIRE;
const _:()=assert!(CAP<=super::BUDGET);
fn unavailable()->io::Error{bad("the exact original offline ZXS deployment is not confirmed")}
type Id=[u8;16];
fn hex(id:&Id)->String{id.iter().map(|v|format!("{v:02x}")).collect()}

pub(super) struct Question{
    nonce:[u8;32],where_system:[u64;3],site:SitePayload,plan:RustPlan,
}
impl Question{
    fn encode(&self)->io::Result<Vec<u8>>{
        compiled_v18()?;
        let site=self.site.encode()?;
        let plan=self.plan.wire();
        if self.nonce==[0;32]||self.where_system[1..].contains(&0)||self.site.origin!=self.plan.origin||
            self.site.plan_sha!=self.plan.sha()||image::check_path(&self.site.image).is_err()||
            !(1..=MAX_SITE).contains(&site.len())||!(1..=MAX_WIRE).contains(&plan.len()){
            return Err(unavailable());
        }
        let mut out=DOMAIN.to_vec();out.extend(self.nonce);
        for word in self.where_system{out.extend(word.to_be_bytes());}
        for b in [&site[..],plan]{
            out.extend(u16::try_from(b.len()).map_err(|_|unavailable())?.to_be_bytes());out.extend(b);
        }
        if out.len()>CAP{return Err(unavailable());}
        Ok(out)
    }
    pub(super) fn decode(raw:&[u8])->io::Result<Self>{
        let mut b=raw.strip_prefix(DOMAIN).filter(|_|raw.len()<=CAP).ok_or_else(unavailable)?;
        fn take<'a>(b:&mut &'a[u8],len:usize)->io::Result<&'a[u8]>{
            let (first,rest)=b.split_at_checked(len).ok_or_else(unavailable)?;*b=rest;Ok(first)
        }
        fn field<const N:usize>(b:&mut &[u8])->io::Result<[u8;N]>{
            Ok(take(b,N)?.try_into().expect("fixed field"))
        }
        fn counted<'a>(b:&mut &'a[u8],bound:usize)->io::Result<&'a[u8]>{
            let len=u16::from_be_bytes(field(b)?) as usize;
            if !(1..=bound).contains(&len){return Err(unavailable());}
            take(b,len)
        }
        let nonce=field(&mut b)?;
        let where_system=[u64::from_be_bytes(field(&mut b)?),u64::from_be_bytes(field(&mut b)?),u64::from_be_bytes(field(&mut b)?)];
        let site=SitePayload::decode(counted(&mut b,MAX_SITE)?)?;
        let plan=RustPlan::decode(counted(&mut b,MAX_WIRE)?).map_err(bad)?;
        let q=Self{nonce,where_system,site,plan};
        if !b.is_empty()||q.encode()?.as_slice()!=raw{return Err(unavailable());}
        Ok(q)
    }
    pub(super) fn original_system(&self)->(ExplicitSubject,[u64;3],&Path){
        (self.site.subject,self.where_system,&self.site.system)
    }
    pub(super) fn inspect(&self,original:&LockedSystem,channel:&Channel,scope:&[u8;32],parent:Pid)->io::Result<()>{
        let (db,witness,marker,prefix)=observed::deployed_active(original,&self.site,&self.plan)?;
        let public=sealed(&prefix)?;
        let mut frame=[0;65];frame[0]=1;frame[1..].copy_from_slice(&witness);
        original.require()?;
        if getppid()!=Some(parent){return Err(unavailable());}
        channel.send_original(Kind::DeployedDatabase,scope,&frame,&db)?;
        let mut proof=[0;65];proof[0]=1;
        proof[1..].copy_from_slice(&marker.require(original.source().dir())?);
        original.require()?;
        if getppid()!=Some(parent){return Err(unavailable());}
        channel.send_original(Kind::DeployedMarker,scope,&proof,marker.pin())?;
        let meta=make_reply(&self.site.sha()?,&prefix);
        original.require()?;
        if getppid()!=Some(parent){return Err(unavailable());}
        channel.send_original(Kind::DeployedActive,scope,&meta,&public)
    }
}

fn make_reply(site:&[u8;32],prefix:&[u8;224])->[u8;65]{
    let mut b=[0;65];b[0]=1;b[1..33].copy_from_slice(site);
    b[33..].copy_from_slice(&Sha256::digest(prefix));b
}
fn checked_reply(meta:&[u8;65],site:&SitePayload,raw:&[u8])->io::Result<([u8;224],u64)>{
    let prefix:[u8;224]=raw.try_into().map_err(|_|unavailable())?;
    let o=site.origin;
    let identities=[o.pool,o.init,o.setup,o.instance,o.bind];
    if *meta!=make_reply(&site.sha()?,&prefix)||&prefix[..16]!=b"ZXS-RUST-SMB-V2\0"||prefix[16]!=1||
        prefix[18..24]!=[0;6]||prefix[24..104].as_chunks::<16>().0!=identities{
        return Err(unavailable());
    }
    let n=usize::from(prefix[17]);
    let generation=u64::from_be_bytes(prefix[152..160].try_into().expect("generation"));
    if !(1..=64).contains(&n)||prefix[104..152].as_chunks::<16>().0.contains(&[0;16])||
        !(1..=(i64::MAX as u64)).contains(&generation)||prefix[160+n..].iter().any(|&b|b!=0)||
        !super::super::super::login(&prefix[160..160+n])||
        (generation==1)!=(prefix[120..136]==o.setup){
        return Err(unavailable());
    }
    Ok((prefix,generation))
}

fn observe()->io::Result<(Id,Id,Id,Id,u64)>{
    // Site selection requires pristine inherited stdio. Only then open the
    // extra descriptors needed to keep the original terminal pinned.
    let site=DeployedSite::select()?;
    let tty=PublicConfirmation::select()?;
    let (system,locked)=site.original();
    let q=Question{nonce:random()?,where_system:place(system)?,site:site.payload().clone(),plan:site.plan().clone()};
    let subject=q.site.subject.tuple();
    let check=||{tty.check()?;site.require()};
    check()?;
    let (db,witness,marker,marker_witness,prefix,generation)=query_with(&q.encode()?,locked,subject,None,check,|ch,scope,until|{
        let (proof,[fd])=ch.wait_checked::<1>(Kind::DeployedDatabase,scope,until,&check)?;
        let db=InitialDatabase::received(fd,subject.1)?;
        let place=db.named(locked)?;
        let witness=db.observation()?;
        if proof[0]!=1||proof[1..]!=witness||place[0]!=q.where_system[0]||place[2]!=q.where_system[2]{
            return Err(unavailable());
        }
        let (marker_frame,[fd])=ch.wait_checked::<1>(Kind::DeployedMarker,scope,until,&check)?;
        let marker=SystemObservation::received(fd,locked,subject.1)?;
        let marker_witness=marker.require(locked)?;
        if marker_frame[0]!=1||marker_frame[1..]!=marker_witness{return Err(unavailable());}
        let (meta,[fd])=ch.wait_checked::<1>(Kind::DeployedActive,scope,until,&check)?;
        let raw=sealed_owner(&fd,224,(subject.1,subject.2))?;
        if meta[1..33]!=site.site_sha(){return Err(unavailable());}
        let (prefix,generation)=checked_reply(&meta,&q.site,&raw)?;
        Ok((db,witness,marker,marker_witness,prefix,generation))
    })?; // matching lowered PID/UID/GID, normal exit and exact EOF
    db.named(locked)?;
    if db.observation()?!=witness||marker.require(locked)?!=marker_witness{return Err(unavailable());}
    site.require_vault_prefix(&prefix)?;
    check()?;db.named(locked)?;
    if db.observation()?!=witness||marker.require(locked)?!=marker_witness{return Err(unavailable());}
    let o=q.site.origin;
    let op=prefix[120..136].try_into().expect("checked original SMB operation");
    Ok((o.pool,o.init,o.instance,op,generation))
}
pub(crate) fn check_deployment()->(u8,String){
    const SCOPE:&str="仅核对原部署：不启动、不授权维护、不变更配置、密码或介质。未核对数据卷和回收卷的可用性。";
    match observe(){
        Ok((pool,init,instance,op,generation))=>(0,format!("已核对原离线 ZXS 部署：池 {}，历史 INIT {}，实例 {}，当前 SMB 凭据操作 {}／代次 {}。\n{SCOPE}",
            hex(&pool),hex(&init),hex(&instance),hex(&op),generation)),
        Err(_)=>(2,format!("未确认原离线 ZXS 部署：原件缺失、在用、发生变化或证据不足。不会另找实例或自动初始化。\n{SCOPE}")),
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn question()->Question{
        let text=include_str!("../../../../../tests/smb-host/rust-install-v1.txt");
        let hex=text.lines().find_map(|s|s.strip_prefix("wire_hex=")).unwrap();
        let raw=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect::<Vec<_>>();
        let plan=RustPlan::decode(&raw).unwrap();
        let site=SitePayload::new(&plan,ExplicitSubject::checked([31;16],1001,1002).unwrap(),
            Path::new("/fixture/S"),Path::new("/fixture/bin/zxs"),[41;32]).unwrap();
        Question{nonce:[43;32],where_system:[0,44,45],site,plan}
    }
    #[test]
    fn reattach_question_is_one_canonical_site_and_one_plan_with_no_tail(){
        let q=question();let raw=q.encode().unwrap();
        let seen=Question::decode(&raw).unwrap();
        assert!(seen.site==q.site&&seen.plan==q.plan);
        assert_eq!(seen.where_system,q.where_system);
        let mut extra=raw.clone();extra.push(0);assert!(Question::decode(&extra).is_err());
        for end in [0,DOMAIN.len()+31,raw.len()-1]{assert!(Question::decode(&raw[..end]).is_err());}
        let mut wrong=raw;wrong[DOMAIN.len()..DOMAIN.len()+32].fill(0);assert!(Question::decode(&wrong).is_err());
    }
    #[test]
    fn reattach_reply_requires_exact_site_digest_current_origin_and_public_bytes_only(){
        let q=question();let o=q.site.origin;
        let mut b=[0;224];b[..16].copy_from_slice(b"ZXS-RUST-SMB-V2\0");b[16]=1;b[17]=3;
        for (slot,id) in b[24..152].as_chunks_mut::<16>().0.iter_mut().zip([o.pool,o.init,o.setup,o.instance,o.bind,[44;16],o.setup,[45;16]]){
            *slot=id;
        }
        b[159]=1;b[160..163].copy_from_slice(b"smb");
        let sha=q.site.sha().unwrap();let meta=make_reply(&sha,&b);
        assert_eq!(checked_reply(&meta,&q.site,&b).unwrap(),(b,1));
        assert!(checked_reply(&make_reply(&[81;32],&b),&q.site,&b).is_err());
        let mut extra=b.to_vec();extra.extend([99;16]);assert!(checked_reply(&meta,&q.site,&extra).is_err());
        for i in [24,40,56,72,88,120,163]{
            let mut changed=b;changed[i]^=1;
            assert!(checked_reply(&make_reply(&sha,&changed),&q.site,&changed).is_err());
        }
    }
    #[test]
    fn reattach_command_accepts_no_override_or_apply_operand(){
        use std::ffi::OsString;
        let args=|x:&[&str]|x.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(matches!(crate::parse(args(&["check-deployment"])),Ok(crate::Command::CheckDeployment)));
        for forbidden in ["--system","--site","--apply","init","reset"]{
            assert!(crate::parse(args(&["check-deployment",forbidden])).is_err());
        }
    }
}
