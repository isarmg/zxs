//! Offline setup only. Discover the original receipt first; for a fresh pair,
//! require a separate local confirmation, two protocol-specific results and a
//! short SQL transaction. No generic credential setter or update/reset path.
use super::{domain::{InstalledSetup,SelectedSetup},observed,terminal::{InputBudget,PublicConfirmation,SelectedInput},smb_policy::Permit,
    web_secret::PreparedWeb, History,Id,PairRequest};
use crate::sqlite_install::{approval,bad,catalog::Canonical,engine,markers::SystemMark,
    reservation::Existing,stage::random,DATABASE,MARKER};
use rusqlite::{params,Transaction,TransactionBehavior};
use rustix::fs::{StatVfsMountFlags,fstatvfs};
use std::{io,path::Path,time::{Duration,Instant}};
use helper_linux_evidence::first_smb::FirstSlot;

pub(super) struct Session {
    installed:InstalledSetup,terminal:PublicConfirmation,input:Option<io::Result<SelectedInput>>,
}
pub(super) enum Start<'a>{Original(History),New(ReadyPair<'a>)}
impl Session {
    pub(super) fn select(system:&Path,pool:Id,init:Id,instance:Id,provision:Id)->io::Result<Self>{
        // Root pins only origins/tty and samples whether a future secret input
        // is permitted. No password, SQL, data-read or account write as root.
        let selection=SelectedSetup::select(system,pool,init,instance,provision)?;
        let (uid,gid)=selection.storage_identity();let terminal=PublicConfirmation::select()?;
        let input=SelectedInput::before_drop(); // keep failure; it need not prevent lookup of a previous receipt
        approval::drop_to_storage(uid,gid)?;
        let installed=selection.after_drop()?;terminal.check()?;
        Ok(Self{installed,terminal,input:Some(input)})
    }
    pub(super) fn original_or_confirm(&mut self,op:Id,reference:Id,web:&[u8],smb:&[u8])->io::Result<Start<'_>>{
        self.terminal.check()?;
        let installed=&self.installed;let r=installed.request(op,reference,web,smb)?;
        let locked=installed.system();let t=Canonical::new()?;
        let db=Existing::observed_system(locked)?;let mark=SystemMark::before_sql(locked.source())?;
        // Standard SQLite VFS only. A stat tuple alone permits inode recycling
        // during a prompt or between a RW close and an independent RO reopen.
        let mut anchor=engine::existing(&db,engine::Access::Read)?;
        let (wanted,result)=observed::snapshot(&mut anchor,&r.pool,&r.init,locked,&db,&t,&mark,|tx|r.original_after_init(tx))?;
        installed.require()?;self.terminal.check()?;
        if let History::Committed{..}=result{
            engine::original_name(&anchor)?;db.sidecars_absent()?;
            anchor.close().map_err(|_|bad("original account observation did not close"))?;return Ok(Start::Original(result));
        }
        let input=self.input.take().ok_or_else(||bad("a new account selection is single-use"))??;
        let root=locked.source();
        if fstatvfs(root.dir())?.f_flag.contains(StatVfsMountFlags::RDONLY){return Err(bad("the selected SYSTEM is read-only"));}
        let (instance,provision)=installed.installation();
        self.terminal.confirm_pair(&r,root.path(),&instance,&provision)?;
        let order=ReadyPair{installed,terminal:&self.terminal,input,db,t,mark,wanted,r,anchor,
            until:Instant::now()+Duration::from_secs(9*60)};
        order.writable_database()?;Ok(Start::New(order))
    }
}

/// Only constructed after the original committed INIT, exact absent history,
/// authenticated startup and the independent fresh local confirmation above.
pub(in crate::sqlite_install) struct ReadyPair<'a>{
    installed:&'a InstalledSetup,terminal:&'a PublicConfirmation,
    input:SelectedInput,db:Existing<'a>,t:Canonical,mark:SystemMark,wanted:[u8;101],
    r:PairRequest,until:Instant,anchor:rusqlite::Connection,
}
impl ReadyPair<'_>{
    fn require(&self)->io::Result<()>{
        if Instant::now()>=self.until{return Err(bad("original foreground setup authorization expired"));}
        if !self.anchor.is_autocommit(){return Err(bad("must not hold an SQLite transaction across operator work"));}
        engine::original_name(&self.anchor)?;
        self.terminal.check()?;self.installed.require()?;let root=self.installed.system().source();
        if fstatvfs(root.dir())?.f_flag.contains(StatVfsMountFlags::RDONLY){return Err(bad("the original SYSTEM is now read-only"));}
        self.db.require()?;self.mark.verify(root,&self.wanted)
    }
    pub(in crate::sqlite_install) fn writable_database(&self)->io::Result<&Existing<'_>>{
        self.require()?;self.db.sidecars_absent()?;Ok(&self.db)
    }
    /// Only the actually confirmed, absent original request can reach this
    /// path. No caller-supplied key bytes, policy location or backend selection.
    pub(super) fn complete(self)->io::Result<History>{
        self.writable_database()?;
        let permit=Permit::open(self.installed,self.r.pool)?;
        let(uid,gid)=self.installed.storage_identity();
        let budget=InputBudget::reserve(&self.input,uid,gid)?;
        permit.require()?;let provider=permit.qualify()?;
        self.require()?;permit.require()?;let values=budget.read(&provider)?;
        self.require()?;permit.require()?;
        let(web,smb)=values.prepare_web()?;
        let value=provider.prepare(&smb)?;drop(smb);
        let material=Material::fresh(&self.r,web,PreparedSmb::Native(value))?;
        self.publish(material,&permit)
    }
    fn publish(self,mut material:Material,permit:&Permit)->io::Result<History>{
        if material.request!=self.r.sha(){return Err(bad("credential preparation belongs to another original request"));}
        let root=self.installed.system().source();let r=&self.r;
        self.writable_database()?;
        permit.require()?;let mut c=engine::new_pair(&self)?;
        let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_error)?;
        self.t.require(&tx)?;
        if observed::first_marker(&r.pool,&r.init,&tx,root)?!=self.wanted||r.original_after_init(&tx)?!=History::Absent{
            return Err(bad("initial enrollment or original setup history changed; query the original operation"));
        }
        self.writable_database()?;engine::original_name(&tx)?;
        let expected=insert_new(&tx,r,&mut material)?;
        if r.original_after_init(&tx)?!=expected{return Err(bad("incomplete original setup result; transaction not confirmed"));}
        drop(material); // SQLite transient parameters stay within this protected short-lived process.
        self.require()?;engine::original_name(&tx)?;
        let seen=root.controls(&[DATABASE.as_bytes(),MARKER,b"SYSTEM.sqlite3-journal"])?;
        if !seen[0]||!seen[1]{return Err(bad("original SYSTEM controls missing before account commit"));}
        permit.require()?;tx.commit().map_err(|_|uncertain())?;c.close().map_err(|_|uncertain())?;
        // A late failure is not evidence of rollback. Never try a fresh Op,
        // password, ID or salt here. Only the exact existing request is queried.
        let after=(||->io::Result<History>{
            root.dir().sync_all()?;self.require()?;self.db.sidecars_absent()?;
            let (wanted,history)=observed::at_original(&r.pool,&r.init,self.installed.system(),&self.db,
                &self.t,&self.mark,|tx|r.original_after_init(tx))?;
            self.require()?;
            if wanted!=self.wanted||history!=expected{return Err(uncertain());}
            Ok(history)
        })();let history=after.map_err(|_|uncertain())?;
        // Last connection for the original DB, after both the writer close and
        // the independent receipt connection's close. No raw DB fd is used.
        engine::original_name(&self.anchor).map_err(|_|uncertain())?;
        self.anchor.close().map_err(|_|uncertain())?;Ok(history)
    }
}
fn uncertain()->io::Error{bad("setup final outcome not confirmed; retain media and query only the exact original operation")}
fn db_error(_:rusqlite::Error)->io::Error{bad("fixed account transaction did not complete; no automatic retry")}
fn once(n:usize)->io::Result<()>{if n==1{Ok(())}else{Err(bad("fixed account transaction affected an unexpected row count"))}}

// No production constructor for supplied verifier bytes. The live secure
// native object can only insert the fixed initial slot; it cannot export it.
enum PreparedSmb{Native(FirstSlot),#[cfg(test)] Synthetic([u8;16])}
impl PreparedSmb{
    fn insert(&mut self,tx:&Transaction<'_>,op:&Id)->io::Result<usize>{match self{
        Self::Native(slot)=>slot.insert(tx,op),
        #[cfg(test)] Self::Synthetic(v)=>tx.execute("INSERT INTO main.smb_password(slot,credential_op,generation,nt_owf) VALUES(1,?1,1,?2)",
            params![&op[..],&v[..]]).map_err(db_error),
    }}
}
struct Material{request:[u8;32],web_id:Id,smb_id:Id,web:PreparedWeb,smb:PreparedSmb}
impl Material{
    fn fresh(r:&PairRequest,web:PreparedWeb,smb:PreparedSmb)->io::Result<Self>{
        let web_id=random()?;let smb_id=random()?;
        if web_id==smb_id{return Err(bad("independent random protocol account IDs collided"));}
        Ok(Self{request:r.sha(),web_id,smb_id,web,smb})
    }
}
fn insert_new(tx:&Transaction<'_>,r:&PairRequest,m:&mut Material)->io::Result<History>{
    if m.request!=r.sha()||m.web_id==m.smb_id||m.web_id==[0;16]||m.smb_id==[0;16]{return Err(bad("invalid original pair preparation"));}
    once(tx.execute("INSERT INTO main.account_pair(singleton,birth_op,approval_ref,web_id,web_login,smb_id,smb_login,smb_sid)
        VALUES(1,?1,?2,?3,?4,?5,?6,?7)",params![&r.op[..],&r.approval[..],&m.web_id[..],&r.web[..],&m.smb_id[..],&r.smb[..],&r.sid[..]]).map_err(db_error)?)?;
    once(m.web.insert(tx,&r.op).map_err(db_error)?)?;
    once(m.smb.insert(tx,&r.op)?)?;
    once(tx.execute("INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,2,?2,?3,1)",
        params![&r.op[..],&r.actor[..],&m.request[..]]).map_err(db_error)?)?;
    Ok(History::Committed{web:m.web_id,smb:m.smb_id})
}

#[cfg(test)]
mod tests{
    use super::*;
    use super::super::tests::{database,fixed,hex};
    fn sample(r:&PairRequest,owf:[u8;16])->Material{
        Material{request:r.sha(),web_id:hex("result_web_id").try_into().unwrap(),smb_id:hex("result_smb_id").try_into().unwrap(),
            web:PreparedWeb::synthetic(hex("web_salt").try_into().unwrap(),hex("web_verifier").try_into().unwrap()),
            smb:PreparedSmb::Synthetic(owf)}
    }
    #[test]fn readonly_vfs_anchor_has_no_active_transaction_and_rejects_a_byteidentical_name_replacement(){
        use rusqlite::{Connection,OpenFlags};
        let dir=tempfile::tempdir().unwrap();let p=dir.path().join("test.sqlite3");
        let w=Connection::open(&p).unwrap();w.execute_batch("CREATE TABLE sample(n INTEGER)").unwrap();w.close().unwrap();
        let flags=OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX|OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let mut a=Connection::open_with_flags_and_vfs(&p,flags,c"unix").unwrap();
        let tx=a.transaction().unwrap();assert_eq!(tx.query_row("SELECT count(*) FROM sample",[],|v|v.get::<_,i64>(0)).unwrap(),0);
        tx.commit().unwrap();assert!(a.is_autocommit());engine::original_name(&a).unwrap();
        let kept=dir.path().join("original");std::fs::rename(&p,&kept).unwrap();std::fs::copy(&kept,&p).unwrap();
        let other=Connection::open_with_flags_and_vfs(&p,flags,c"unix").unwrap();engine::original_name(&other).unwrap();
        assert!(engine::original_name(&a).is_err());other.close().unwrap();a.close().unwrap();
    }
    #[test]fn two_protocol_rows_and_original_appear_together_or_not_at_all(){
        let mut db=database();let r=fixed();let shape=Canonical::new().unwrap();
        db.execute("UPDATE operation SET state=1 WHERE op_id=?1",[&r.init[..]]).unwrap();
        {let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
         assert!(r.original(&tx,&shape).unwrap()==History::Absent);
         // Last credential insert (after prepared pair and Web insert) fails.
         assert!(insert_new(&tx,&r,&mut sample(&r,[0;16])).is_err());tx.rollback().unwrap();}
        {let tx=db.transaction().unwrap();assert_eq!(r.original(&tx,&shape).unwrap(),History::Absent);
         for sql in ["SELECT count(*) FROM account_pair","SELECT count(*) FROM web_password","SELECT count(*) FROM smb_password"]{
             assert_eq!(tx.query_row(sql,[],|row|row.get::<_,i64>(0)).unwrap(),0);
         }tx.commit().unwrap();}
        let mut material=sample(&r,hex("synthetic_smb_field").try_into().unwrap());
        let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        let expected=insert_new(&tx,&r,&mut material).unwrap();tx.commit().unwrap();
        db.execute_batch("PRAGMA query_only=ON").unwrap();let tx=db.transaction().unwrap();
        let before=tx.total_changes();assert_eq!(r.original(&tx,&shape).unwrap(),expected);
        let mut other=fixed();other.approval[2]^=1;assert!(other.original(&tx,&shape).is_err());
        assert_eq!(tx.total_changes(),before);
    }
}
