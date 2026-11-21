//! Web-only credential rotation on the already-deployed original v17 S.
//! No Web verifier leaves the original lowered SQL worker.
use super::{Id,bad,blob,collision,login,one,sql,transaction::{AccountCut,expired}};
use super::super::{fresh_issuer::SitePayload,web_secret::PreparedWeb};
use crate::{smb_profile::rust_first::RustPlan,sqlite_install::originals::LockedSystem};
use rusqlite::{OptionalExtension,Transaction,params};
use sha2::{Digest,Sha256};
use std::{fs::File,io,time::Instant};

fn wrong()->io::Error{bad("the exact original Web administrator revision was not proved")}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::sqlite_install::pair) struct Order{pub op:Id,pub approval:Id}
impl Order{
    pub(in crate::sqlite_install::pair) fn check(self,site:&SitePayload)->io::Result<()>{
        let o=site.origin;
        if [o.pool,o.init,o.instance,o.bind,o.setup,[0;16]].contains(&self.op)||
            self.approval==[0;16]||self.approval==self.op{return Err(wrong());}
        Ok(())
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::sqlite_install::pair) struct Current{pub account:Id,pub login:Vec<u8>,pub op:Id,pub generation:i64}
impl Current{
    pub(in crate::sqlite_install::pair) fn check(&self,site:&SitePayload)->io::Result<()>{
        if self.account==[0;16]||self.op==[0;16]||self.op==site.origin.init||
            self.generation<1||(self.generation==1)!=(self.op==site.origin.setup)||!login(&self.login){
            return Err(wrong());
        }
        Ok(())
    }
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::sqlite_install::pair) struct Receipt{
    pub previous:Id,pub generation:i64,pub current:bool,pub digest:[u8;32],
}
pub(in crate::sqlite_install::pair) fn fingerprint(site:&SitePayload,request:Order,web:&Current)->io::Result<[u8;32]>{
    web.check(site)?;request.check(site)?;
    if web.op==request.op||web.generation==i64::MAX{return Err(wrong());}
    // Exact existing ZXS credential.rs::reset_request(Role::Web) bytes.
    // The stronger physical site/plan and Helper binding lives in the sealed
    // per-invocation envelope, not in the persistent cross-project digest.
    let o=site.origin;
    let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/RESET-WEB\0");
    for id in [o.pool,o.init,o.setup,request.op,site.subject.tuple().0,request.approval,web.account,web.op]{h.update(id);}
    h.update(web.generation.to_be_bytes());Ok(h.finalize().into())
}
fn web_current(tx:&Transaction<'_>,site:&SitePayload)->io::Result<Current>{
    let o=site.origin;let (actor,uid,_)=site.subject.tuple();
    let mut q=tx.prepare("SELECT a.web_id,a.web_login,w.credential_op,w.generation
        FROM main.pool_birth b
        JOIN main.operation i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1 AND i.actor_id=?1
        JOIN main.account_pair a ON a.singleton=b.singleton AND a.smb_family=1
        JOIN main.operation s ON s.op_id=a.birth_op AND s.kind=2 AND s.state=1
        JOIN main.web_password w ON w.slot=a.singleton
        JOIN main.active_credential p ON p.role=0 AND p.slot=w.slot
          AND p.credential_op=w.credential_op AND p.generation=w.generation
        WHERE b.singleton=1 AND b.pool_id=?2 AND b.init_op=?3 AND b.storage_uid=?4
          AND a.birth_op=?5 AND a.instance_id=?6 AND a.setup_bound_sha=?7 AND a.smb_rust_bind_id=?8
        LIMIT 2").map_err(sql)?;
    let mut rows=q.query(params![&actor[..],&o.pool[..],&o.init[..],i64::from(uid),&o.setup[..],&o.instance[..],
        &site.plan_sha[..],&o.bind[..]]).map_err(sql)?;
    let row=rows.next().map_err(sql)?.ok_or_else(wrong)?;
    let name=row.get_ref(1).map_err(sql)?.as_blob().map_err(|_|wrong())?;
    if !login(name){return Err(wrong());}
    let current=Current{account:blob(row,0)?,login:name.to_vec(),op:blob(row,2)?,generation:row.get(3).map_err(sql)?};
    current.check(site)?;
    if rows.next().map_err(sql)?.is_some(){return Err(wrong());}
    if current.generation!=1&&recorded(tx,site,&current,&current.op,None)?.is_none(){return Err(wrong());}
    Ok(current)
}
// Reconstruct exactly the public request from the immutable rows; no verifier,
// salt or old password is selected. A collision with any other kind is unknown.
fn recorded(tx:&Transaction<'_>,site:&SitePayload,web:&Current,op:&Id,approval:Option<Id>)->io::Result<Option<Receipt>>{
    let mut q=tx.prepare("SELECT o.actor_id,o.request_sha,r.previous_op,r.generation,r.approval_ref
        FROM main.operation o JOIN main.credential_reset r ON r.op_id=o.op_id
        WHERE o.op_id=?1 AND o.kind=8 AND o.state=1 AND r.role=0 AND r.bound_sha IS NULL").map_err(sql)?;
    let mut rows=q.query([&op[..]]).map_err(sql)?;
    let Some(row)=rows.next().map_err(sql)? else{
        if collision(tx,op)?{return Err(wrong());}
        return Ok(None);
    };
    let actor=blob::<16>(row,0)?;let digest=blob::<32>(row,1)?;
    let previous=blob::<16>(row,2)?;let generation:i64=row.get(3).map_err(sql)?;
    let reference=blob::<16>(row,4)?;
    if actor!=site.subject.tuple().0||approval.is_some_and(|v|v!=reference)||
        generation<2||generation>web.generation||(generation==web.generation)!=(web.op==*op)||
        rows.next().map_err(sql)?.is_some(){return Err(wrong());}
    let old=Current{op:previous,generation:generation-1,..web.clone()};
    if fingerprint(site,Order{op:*op,approval:reference},&old)?!=digest{return Err(wrong());}
    let predecessor:Option<i64>=tx.query_row("SELECT e.generation FROM main.credential_epoch e
        JOIN main.operation p ON p.op_id=e.credential_op AND p.state=1
          AND p.kind=CASE WHEN e.generation=1 THEN 2 ELSE 8 END
        WHERE e.role=0 AND e.slot=1 AND e.credential_op=?1 AND e.generation=?2",
        params![&previous[..],old.generation],|r|r.get(0)).optional().map_err(sql)?;
    if predecessor!=Some(old.generation){return Err(wrong());}
    Ok(Some(Receipt{previous,generation,current:*op==web.op,digest}))
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::sqlite_install::pair) struct Witness{pub web:Current,pub smb:[u8;224]}
fn original(tx:&Transaction<'_>,site:&SitePayload,plan:&RustPlan)->io::Result<Witness>{
    if plan.origin!=site.origin||plan.sha()!=site.plan_sha{return Err(wrong());}
    Ok(Witness{web:web_current(tx,site)?,smb:super::rust_vault::active_at(tx,plan)?.record_prefix()?})
}
fn commit_approved(tx:&Transaction<'_>,site:&SitePayload,plan:&RustPlan,order:Order,before:&Witness,
    prepared:&PreparedWeb)->io::Result<Receipt>{
    let observed=original(tx,site,plan)?;
    if &observed!=before||collision(tx,&order.op)?{return Err(wrong());}
    let digest=fingerprint(site,order,&before.web)?;
    let generation=before.web.generation+1;
    // v17's kind8 is a single commit, not a kind7 vault-style PENDING.
    // The existing role-0 trigger insists on the exact active predecessor.
    one(tx.execute("INSERT INTO main.credential_reset(op_id,role,previous_op,generation,approval_ref,bound_sha)
                    VALUES(?1,0,?2,?3,?4,NULL)",
        params![&order.op[..],&before.web.op[..],generation,&order.approval[..]]))?;
    one(prepared.reset(tx,&order.op,&before.web.op,generation,before.web.generation))?;
    one(tx.execute("INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,8,?2,?3,1)",
        params![&order.op[..],&site.subject.tuple().0[..],&digest[..]]))?;
    let after=original(tx,site,plan)?;
    if after.smb!=before.smb||after.web.generation!=generation||after.web.op!=order.op||
        (after.web.account,&after.web.login)!=(before.web.account,&before.web.login){return Err(wrong());}
    recorded(tx,site,&after.web,&order.op,Some(order.approval))?.ok_or_else(wrong)
}

pub(in crate::sqlite_install::pair) struct WebReset<'s,'p>{cut:AccountCut<'s>,site:&'p SitePayload,plan:&'p RustPlan,order:Order}
impl<'s,'p> WebReset<'s,'p>{
    pub(in crate::sqlite_install::pair) fn open(locked:&'s LockedSystem,site:&'p SitePayload,plan:&'p RustPlan,order:Order)->io::Result<Self>{
        super::super::fresh_issuer::compiled_v17()?;
        order.check(site)?;
        let root=locked.source();
        if root.path()!=site.system||root.uid()!=site.subject.tuple().1||site.origin!=plan.origin||
            site.plan_sha!=plan.sha(){return Err(wrong());}
        Ok(Self{cut:AccountCut::at(locked,site.origin.pool,site.origin.init)?,site,plan,order})
    }
    pub(in crate::sqlite_install::pair) fn observe(&self,new:bool)->io::Result<(Witness,Option<Receipt>)>{
        self.cut.read(|tx|{
            let seen=original(tx,self.site,self.plan)?;
            let receipt=recorded(tx,self.site,&seen.web,&self.order.op,Some(self.order.approval))?;
            if new{
                if receipt.is_some(){return Err(wrong());}
                fingerprint(self.site,self.order,&seen.web)?;
            }
            Ok((seen,receipt))
        })
    }
    pub(in crate::sqlite_install::pair) fn objects(&self)
        ->io::Result<(File,[u8;64],crate::sqlite_install::markers::SystemObservation)>{
        self.cut.original_objects()
    }
    pub(in crate::sqlite_install::pair) fn apply(&self,before:&Witness,prepared:PreparedWeb,until:Instant,
        live:impl Fn()->io::Result<()>)->io::Result<Receipt>{
        expired(until)?;live()?;
        let expect=self.cut.write(until,&live,|tx|commit_approved(tx,self.site,self.plan,self.order,before,&prepared))?;
        drop(prepared);
        self.cut.sync()?;
        let (after,receipt)=self.observe(false)?;
        if after.smb!=before.smb||receipt!=Some(expect)||!expect.current{return Err(wrong());}
        expired(until)?;live()?;Ok(expect)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::sqlite_install::pair::fresh_issuer::ExplicitSubject;
    use rusqlite::Connection;
    use std::path::Path;

    fn seed()->(Connection,SitePayload,RustPlan){
        let data=include_str!("../../../../tests/smb-host/rust-install-v1.txt");
        let hex=data.lines().find_map(|s|s.strip_prefix("wire_hex=")).unwrap();
        let raw=hex.as_bytes().chunks_exact(2).map(|b|u8::from_str_radix(std::str::from_utf8(b).unwrap(),16).unwrap()).collect::<Vec<_>>();
        let plan=RustPlan::decode(&raw).unwrap();
        let o=plan.origin;
        let actor=[87u8;16];
        let subject=ExplicitSubject::checked(actor,1001,1002).unwrap();
        let site=SitePayload::new(&plan,subject,Path::new("/synthetic/S"),Path::new("/usr/bin/zxs"),[91;32]).unwrap();
        let mut c=Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;").unwrap();
        c.execute_batch(crate::sqlite_install::stage::SCHEMA).unwrap();
        let tx=c.transaction().unwrap();
        tx.execute("INSERT INTO main.operation VALUES(?1,1,?2,?3,0)",params![&o.init[..],&actor[..],&[92u8;32][..]]).unwrap();
        tx.execute("INSERT INTO main.pool_birth VALUES(1,?1,?2,?3,1001,?4)",
            params![&o.pool[..],&o.init[..],&[93u8;16][..],(plan.folders().len()+2) as i64]).unwrap();
        let roots=[([94u8;16],"/synthetic/S".to_string()),([95u8;16],"/synthetic/R".to_string())]
            .into_iter().chain(plan.folders().iter().enumerate().map(|(i,(f,_))|(f.volume,format!("/synthetic/F{i}"))));
        for (i,(v,name)) in roots.enumerate(){
            let role=i.min(2) as i64;
            tx.execute("INSERT INTO main.volume_claim VALUES(?1,?2,?3,?4,?5,255,?6)",
                params![&o.init[..],i as i64,role,&v[..],name.as_bytes(),&[101u8+i as u8;32][..]]).unwrap();
        }
        tx.execute("UPDATE main.operation SET state=1 WHERE op_id=?1",[&o.init[..]]).unwrap();
        super::super::rust_vault::web_reset_fixture_pair(&tx,&plan,actor);
        tx.commit().unwrap();(c,site,plan)
    }
    fn order(n:u8)->Order{Order{op:[n;16],approval:[n+1;16]}}
    fn secret()->PreparedWeb{PreparedWeb::synthetic([115;16],[116;32])}
    #[test]
    fn web_admin_reset_persistent_digest_matches_the_existing_zxs_reader_vector(){
        const V:&str=include_str!("../../../../tests/sqlite/first-account-v1.txt");
        fn bytes<const N:usize>(field:&str)->[u8;N]{
            let v=V.lines().find_map(|line|line.split_once('=').filter(|(k,_)|*k==field)).unwrap().1;
            assert_eq!(v.len(),2*N);std::array::from_fn(|i|u8::from_str_radix(&v[2*i..2*i+2],16).unwrap())
        }
        let (_,mut site,_)=seed();
        site.origin.pool=bytes("pool");site.origin.init=bytes("init_op");site.origin.setup=bytes("setup_op");
        site.subject=ExplicitSubject::checked(bytes("offline_actor"),1001,1002).unwrap();
        let order=Order{op:bytes("web_reset_op"),approval:bytes("reset_approval")};
        let now=Current{account:bytes("result_web_id"),login:b"first.admin".to_vec(),op:site.origin.setup,generation:1};
        assert_eq!(fingerprint(&site,order,&now).unwrap(),bytes("web_reset_sha"));
    }
    #[test]
    fn web_admin_reset_two_atomic_successors_keep_the_exact_smb_epoch_and_only_the_current_web_login(){
        let (mut c,site,plan)=seed();
        let tx=c.transaction().unwrap();let before=original(&tx,&site,&plan).unwrap();
        let a=order(121);let b=order(123);
        let r=commit_approved(&tx,&site,&plan,a,&before,&secret()).unwrap();
        assert_eq!(r.generation,2);assert!(r.current);
        assert_eq!(r.digest,fingerprint(&site,a,&before.web).unwrap());
        tx.commit().unwrap();
        let tx=c.transaction().unwrap();let mid=original(&tx,&site,&plan).unwrap();
        assert_eq!(mid.web.op,a.op);assert_eq!(mid.web.account,before.web.account);
        assert_eq!(mid.web.login,before.web.login);assert_eq!(mid.smb,before.smb);
        assert!(commit_approved(&tx,&site,&plan,b,&before,&secret()).is_err());
        assert!(commit_approved(&tx,&site,&plan,a,&mid,&secret()).is_err());
        let second=commit_approved(&tx,&site,&plan,b,&mid,&secret()).unwrap();
        assert_eq!(second.generation,3);tx.commit().unwrap();
        let tx=c.transaction().unwrap();let end=original(&tx,&site,&plan).unwrap();
        assert_eq!(end.smb,before.smb);
        assert_eq!(recorded(&tx,&site,&end.web,&a.op,Some(a.approval)).unwrap(),Some(Receipt{current:false,..r}));
        assert_eq!(recorded(&tx,&site,&end.web,&b.op,Some(b.approval)).unwrap(),Some(second));
        assert!(recorded(&tx,&site,&end.web,&a.op,Some(b.approval)).is_err());
        let active:Vec<(i64,Vec<u8>,i64)>=tx.prepare("SELECT role,credential_op,generation FROM main.active_credential ORDER BY role")
            .unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap().collect::<Result<_,_>>().unwrap();
        assert_eq!(active,[(0,b.op.to_vec(),3),(1,site.origin.setup.to_vec(),1)]);
    }
    #[test]
    fn web_admin_reset_rollback_keeps_both_active_roles_and_creates_no_fake_history(){
        let (mut c,site,plan)=seed();
        let action=order(125);
        let tx=c.transaction().unwrap();let original_state=original(&tx,&site,&plan).unwrap();
        let different=SitePayload{origin:crate::smb_profile::rust_first::Origin{pool:[111;16],..site.origin},..site.clone()};
        assert_ne!(fingerprint(&site,action,&original_state.web).unwrap(),fingerprint(&different,action,&original_state.web).unwrap());
        let bad_predecessor=Current{generation:3,..original_state.web.clone()};
        assert!(fingerprint(&site,action,&bad_predecessor).is_err());
        let mut mismatch=original_state.clone();mismatch.smb[32]^=1;
        assert!(commit_approved(&tx,&site,&plan,action,&mismatch,&secret()).is_err());
        assert!(recorded(&tx,&site,&original_state.web,&action.op,Some(action.approval)).unwrap().is_none());
        commit_approved(&tx,&site,&plan,action,&original_state,&secret()).unwrap();
        tx.rollback().unwrap();
        let tx=c.transaction().unwrap();assert_eq!(original(&tx,&site,&plan).unwrap(),original_state);
        assert!(recorded(&tx,&site,&original_state.web,&action.op,Some(action.approval)).unwrap().is_none());
        let remaining:i64=tx.query_row("SELECT count(*) FROM main.credential_reset",[],|r|r.get(0)).unwrap();
        assert_eq!(remaining,0);
    }
}
