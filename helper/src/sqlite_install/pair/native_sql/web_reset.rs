//! Independent Web reset. No instance/BOUND read, native library, SMB
//! secrets, independently allocated protocol role, or multi-stage state.
use super::{Core,Id,Original,bad,blob,collision,one,require_init,sql,RESET_LOOKUP};
use super::super::{terminal::{PublicConfirmation,SelectedInput},web_secret::PreparedWeb};
use crate::sqlite_install::{approval,originals::SelectedSystem};
use rusqlite::{params,Transaction};
use sha2::{Digest,Sha256};
use std::{io,os::unix::ffi::OsStrExt,path::Path,time::{Duration,Instant}};

pub(in crate::sqlite_install::pair) struct Reset {
    scope:Core, setup:Id, account:Id, previous:Id, previous_generation:i64,
}
impl Reset {
    pub(in crate::sqlite_install::pair) fn new(pool:Id,init:Id,setup:Id,op:Id,actor:Id,approval:Id,account:Id,
        previous:Id,previous_generation:i64)->io::Result<Self>{
        let scope=Core::new(pool,init,op,actor,approval)?;
        if [setup,account,previous].contains(&[0;16])||[setup,previous].contains(&op)
            ||!(1..i64::MAX).contains(&previous_generation)||(previous_generation==1)!=(previous==setup){
            return Err(bad("wrong original Web account or predecessor"));
        }Ok(Self{scope,setup,account,previous,previous_generation})
    }
    pub(super) fn core(&self)->&Core{&self.scope}
    pub(super) fn sha(&self)->[u8;32]{
        let c=self.scope;let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/RESET-WEB\0");
        for i in [c.pool,c.init,self.setup,c.op,c.actor,c.approval,self.account,self.previous]{h.update(i);}
        h.update(self.previous_generation.to_be_bytes());h.finalize().into()
    }
    pub(super) fn original(&self,tx:&Transaction<'_>)->io::Result<Original<i64>>{
        let c=self.scope;require_init(tx,&c)?;self.account(tx)?;
        let d=self.sha();let g=self.previous_generation+1;let none:Option<&[u8]>=None;
        let mut q=tx.prepare(RESET_LOOKUP).map_err(sql)?;
        let mut rows=q.query(params![&c.pool[..],&c.init[..],&self.setup[..],&c.op[..],&c.actor[..],&d[..],&c.approval[..],
            0i64,&self.account[..],&self.previous[..],g,none,none]).map_err(sql)?;
        if let Some(r)=rows.next().map_err(sql)?{
            if r.get::<_,i64>(0).map_err(sql)?!=1||r.get::<_,i64>(1).map_err(sql)?!=g||rows.next().map_err(sql)?.is_some(){
                return Err(bad("a Web reset must be one exact committed original"));
            }return Ok(Original::Committed(g));
        }
        if collision(tx,&c.op)?{return Err(bad("operation ID belongs to another request"));}
        Ok(Original::Absent)
    }
    fn account(&self,tx:&Transaction<'_>)->io::Result<Vec<u8>>{
        let name:Vec<u8>=tx.query_row("SELECT a.web_login FROM main.account_pair a JOIN main.operation b ON b.op_id=a.birth_op
            WHERE a.singleton=1 AND b.kind=2 AND b.state=1 AND a.birth_op=?1 AND a.web_id=?2",
            params![&self.setup[..],&self.account[..]],|r|r.get(0)).map_err(sql)?;
        if !super::super::login(&name){return Err(bad("invalid original Web login name"));}
        if self.previous_generation==1{return Ok(name);}
        let mut q=tx.prepare("SELECT r.previous_op,r.approval_ref,o.actor_id,o.request_sha FROM main.credential_reset r
            JOIN main.operation o ON o.op_id=r.op_id WHERE r.role=0 AND r.op_id=?1 AND r.generation=?2 AND o.kind=8 AND o.state=1
            AND EXISTS(SELECT 1 FROM main.credential_epoch e JOIN main.operation o ON o.op_id=e.credential_op
                WHERE e.role=0 AND e.credential_op=r.previous_op AND e.generation=r.generation-1
                AND o.kind=CASE WHEN e.generation=1 THEN 2 ELSE 8 END AND o.state=1)").map_err(sql)?;
        let mut rows=q.query(params![&self.previous[..],self.previous_generation]).map_err(sql)?;
        let r=rows.next().map_err(sql)?.ok_or_else(||bad("no exact retained Web predecessor"))?;
        let prev=Self::new(self.scope.pool,self.scope.init,self.setup,self.previous,blob(r,2)?,blob(r,1)?,
            self.account,blob(r,0)?,self.previous_generation-1)?;
        if prev.sha()!=blob::<32>(r,3)?||rows.next().map_err(sql)?.is_some(){
            return Err(bad("corrupt previous original Web password operation"));
        }Ok(name)
    }
    pub(super) fn eligible(&self,tx:&Transaction<'_>)->io::Result<Vec<u8>>{
        if self.original(tx)?!=Original::Absent{return Err(bad("this original has already completed"));}
        let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.web_password WHERE slot=1 AND credential_op=?1 AND generation=?2)
            AND NOT EXISTS(SELECT 1 FROM main.credential_reset WHERE role=0 AND generation>?2)",
            params![&self.previous[..],self.previous_generation],|r|r.get(0)).map_err(sql)?;
        if !active{return Err(bad("the corresponding Web predecessor is no longer active"));}
        self.account(tx)
    }
    pub(super) fn update(&self,tx:&Transaction<'_>,new:&PreparedWeb)->io::Result<i64>{
        self.eligible(tx)?;
        let c=self.scope;let g=self.previous_generation+1;
        one(tx.execute("INSERT INTO main.credential_reset(op_id,role,previous_op,generation,approval_ref,bound_sha)
            VALUES(?1,0,?2,?3,?4,NULL)",params![&c.op[..],&self.previous[..],g,&c.approval[..]]))?;
        one(new.reset(tx,&c.op,&self.previous,g,self.previous_generation))?;
        one(tx.execute("INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,8,?2,?3,1)",
            params![&c.op[..],&c.actor[..],&self.sha()[..]]))?;
        if self.original(tx)?!=Original::Committed(g){return Err(bad("invalid Web-only original reset result"));}
        Ok(g)
    }
    fn preview(&self,path:&Path,login:&[u8])->String{
        let h=|b:&[u8]|b.iter().map(|v|format!("{v:02x}")).collect::<String>();
        let escape=|b:&[u8]|b.iter().copied().flat_map(std::ascii::escape_default).map(char::from).collect::<String>();
        format!("仅重置 Web 管理员 {} / {}，不访问 SMB 账号库或改变 SMB 的密码及权限。\n原 SYSTEM {}，池 {}、INIT {}、首设 {}。\n原旧凭据 {} 第 {} 代；本次操作 {}、主体 {}、审批参考 {}，请求摘要 {}。\n",
            escape(login),h(&self.account),escape(path.as_os_str().as_bytes()),h(&self.scope.pool),h(&self.scope.init),
            h(&self.setup),h(&self.previous),self.previous_generation,h(&self.scope.op),h(&self.scope.actor),h(&self.scope.approval),h(&self.sha()))
    }
}

#[derive(Debug)]
pub(in crate::sqlite_install::pair) struct Selection {
    pub(in crate::sqlite_install::pair) system:std::path::PathBuf,
    pub(in crate::sqlite_install::pair) pool:Id,pub(in crate::sqlite_install::pair) init:Id,pub(in crate::sqlite_install::pair) setup:Id,pub(in crate::sqlite_install::pair) op:Id,pub(in crate::sqlite_install::pair) reference:Id,
    pub(in crate::sqlite_install::pair) account:Id,pub(in crate::sqlite_install::pair) previous:Id,pub(in crate::sqlite_install::pair) generation:i64,pub(in crate::sqlite_install::pair) lookup:bool,
}
impl Selection {
    pub(in crate::sqlite_install::pair) fn run(self)->io::Result<Original<i64>>{
        use super::transaction::{Order,Session,ResultRow};
        approval::no_inherited_descriptors()?;
        let (actor,uid,gid)=approval::profile()?;
        let origin=SelectedSystem::foreground_origin(&self.system)?;
        let r=Reset::new(self.pool,self.init,self.setup,self.op,actor,self.reference,self.account,self.previous,self.generation)?;
        let terminal=PublicConfirmation::select()?;
        let prepared=(!self.lookup).then(SelectedInput::before_drop);
        approval::drop_to_storage(uid,gid)?;
        let original=origin.lock()?;
        let session=Session::open(&original,Order::WebReset(r))?;
        match session.original()?{
            Original::Committed(ResultRow::Reset(g))=>return Ok(Original::Committed(g)),
            Original::Absent if self.lookup=>return Ok(Original::Absent),
            Original::Absent=>(),
            _=>return Err(bad("invalid Web-only maintenance state")),
        }
        let login=session.web_eligible()?;
        let Order::WebReset(r)=session.order() else{return Err(bad("wrong offline role"));};
        terminal.confirm_web_reset(r.preview(&self.system,&login))?;
        let p=prepared.ok_or_else(||bad("missing local Web input"))??;
        let expires=Instant::now()+Duration::from_secs(5*60);
        let material=p.web_only(uid,gid)?;
        let g=session.reset_web(&material,expires,||terminal.check())?;
        Ok(Original::Committed(g))
    }
}
