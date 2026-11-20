//! Fixed host destination only. An explicit grant over the pre-existing
//! originals and the returned leases must both survive; no generic root
//! copy/rename/overwrite facility, and no filename/SID selected by a client.
use super::{HostOrigins,OriginChannel,Returned};
use super::super::{FRESH_SITE,Times,live::Stamp};
use crate::sqlite_install::{bad,request::Id};
use rustix::fs::{Mode,OFlags,RenameFlags,fchmod,openat,renameat_with};
use std::{ffi::CStr,fs::File,io::{self,Write},thread,time::{Duration,Instant},os::unix::{ffi::OsStrExt,fs::FileExt},process::{Child,Command,Stdio}};

const MIDDLE:&[&[u8]]=&[b"private",b"state",b"smb.conf",b"accountdb",b".domain.bound.new"];
const END:&[&[u8]]=&[b"private",b"state",b"smb.conf",b"accountdb",b"domain.bound"];
#[derive(Copy,Clone)] enum Name{Stage,Bound}
impl Name{fn name(self)->&'static CStr{match self{Self::Stage=>c".domain.bound.new",Self::Bound=>c"domain.bound"}}}
pub(super) fn hex_id(v:&[u8])->String{v.iter().map(|b|format!("{b:02x}")).collect()}
fn escaped(v:&[u8])->String{v.iter().copied().flat_map(std::ascii::escape_default).map(char::from).collect()}

// Only Grant::start can create this owner from its fixed child and socket.
struct SelectedWorker<'a>{child:Child,channel:Option<OriginChannel>,grant:Option<Grant<'a>>,allowed:bool,reaped:bool}
impl SelectedWorker<'_>{
    /// Unready leaves ownership with the supervisor; abnormal exit or a bad
    /// return is reaped, never published. No re-use of a PID after wait.
    fn finish_when_exited(&mut self)->io::Result<[u8;128]>{
        let Some((pid,normal))=super::waitable(&self.child)? else{
            return Err(io::Error::new(io::ErrorKind::WouldBlock,"dedicated worker is still running"));};
        if !self.allowed{return Err(bad("dedicated worker exited before current foreground permission"));}
        let rx=self.channel.take().ok_or_else(||bad("worker result already consumed"))?;
        let grant=self.grant.take().ok_or_else(||bad("no original foreground grant"))?;
        let r=if normal{Returned::from_waitable(rx,pid,grant.originals)}else{Err(bad("dedicated worker did not finish normally"))};
        let status=loop{match self.child.wait(){
            Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,other=>break other?,
        }};
        self.reaped=true;
        if status.code()!=Some(0){return Err(bad("no clean result from the original dedicated worker"));}
        grant.publish(r?)
    }
}
// Until wait, the actual child (even a zombie) reserves its PID. TERM lets
// the restricted worker clean its own launcher group. KILL or an abnormal
// status is never treated as proof that all guest processes drained.
impl SelectedWorker<'_>{
    fn stop(&mut self)->io::Result<()>{
        if self.reaped{return Ok(());}
        let id=rustix::process::Pid::from_raw(self.child.id() as i32).ok_or_else(||bad("invalid original worker PID"))?;
        let signal=|s|match rustix::process::kill_process(id,s){
            Ok(())|Err(rustix::io::Errno::SRCH)=>Ok(()),Err(e)=>Err(io::Error::from(e)),
        };
        if super::waitable(&self.child)?.is_none(){
            signal(rustix::process::Signal::TERM)?;
            let start=Instant::now();
            while super::waitable(&self.child)?.is_none(){
                if start.elapsed()>=Duration::from_secs(8){signal(rustix::process::Signal::KILL)?;break;}
                thread::sleep(Duration::from_millis(20));
            }
        }
        loop{match self.child.wait(){Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,r=>{r?;self.reaped=true;return Ok(());}}}
    }
    fn complete(mut self)->io::Result<[u8;128]>{
        let begin=Instant::now();
        loop{
            if super::super::cancelled().is_err()||begin.elapsed()>=Duration::from_secs(90){
                self.stop()?;
                return Err(bad("dedicated host operation was interrupted or timed out; keep originals, do not reissue"));
            }
            if super::waitable(&self.child)?.is_some(){return self.finish_when_exited();}
            if !self.allowed {
                let channel=self.channel.as_ref().ok_or_else(||bad("missing original worker channel"))?;
                let g=self.grant.as_ref().ok_or_else(||bad("missing live foreground grant"))?;
                let cred=rustix::net::UCred{pid:rustix::process::Pid::from_raw(self.child.id() as i32)
                    .ok_or_else(||bad("invalid original worker PID"))?,
                    uid:rustix::process::Uid::from_raw(g.originals.uid),gid:rustix::process::Gid::from_raw(g.originals.gid)};
                match super::start::respond(&channel.start_endpoint(),cred,&g.scope,||{
                    g.terminal.check()?;g.originals.fresh()?;g.originals.release.require()?;
                    if g.originals.scope(&g.reference)?!=g.scope||super::waitable(&self.child)?.is_some()
                        ||begin.elapsed()>=Duration::from_secs(90){return Err(bad("original approval, child or time window changed"));}
                    g.terminal.check()?;super::super::cancelled()
                }){
                    Ok(true)=>self.allowed=true,Ok(false)=>(),Err(e)=>return Err(e),
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for SelectedWorker<'_>{fn drop(&mut self){let _=self.stop();}}
use crate::sqlite_install::pair::terminal::PublicConfirmation;
struct Grant<'a>{originals:&'a HostOrigins,reference:Id,scope:[u8;32],terminal:PublicConfirmation}
impl<'a> Grant<'a>{
    /// In the dedicated foreground host CLI, before starting its worker. A
    /// granted reference is consumed once, not stored as a reusable capability.
    fn foreground(originals:&'a HostOrigins,reference:Id)->io::Result<Self>{
        originals.fresh()?;originals.release.require()?;
        let scope=originals.scope(&reference)?;
        let terminal=PublicConfirmation::select()?;let stderr=io::stderr();
        let mut out=stderr.lock();
        writeln!(out,"仅为此空专用实例创建 Samba 本机域与受保护的 domain.bound。不会使用全机 Samba，不建立账户，不开共享。若中断，保留现场，不重新发号。")?;
        writeln!(out,"SYSTEM {}；池 {}；预期原 INIT {}（此时尚未查询；降权后仅当原 SYSTEM 确认同一 INIT 已提交才生成域）。",escaped(originals.system.path().as_os_str().as_bytes()),hex_id(&originals.pool),hex_id(&originals.init))?;
        writeln!(out,"专用实例 {}；原操作 {}；维护主体 {}；审批参考 {}；存储 UID/GID {}/{}。",
            hex_id(&originals.instance),hex_id(&originals.op),hex_id(&originals.actor),hex_id(&reference),originals.uid,originals.gid)?;
        writeln!(out,"正式配置 SHA-256 {}；核准的软件发行材料 {}；本次完整范围 {}。",hex_id(&originals.config),hex_id(&originals.release.key()),hex_id(&scope))?;out.flush()?;drop(out);
        terminal.confirm_domain()?;
        terminal.check()?;originals.fresh()?;originals.release.require()?;
        if originals.scope(&reference)?!=scope{return Err(bad("approved originals changed; do not start provisioning"));}
        Ok(Self{originals,reference,scope,terminal})
    }
    fn start(self)->io::Result<SelectedWorker<'a>>{
        self.terminal.check()?;self.originals.fresh()?;self.originals.release.require()?;
        if self.originals.scope(&self.reference)?!=self.scope{return Err(bad("different confirmed host scope before spawn"));}
        let input=super::worker::sealed_request(self.originals,self.reference,self.scope)?;
        let (channel,tx)=OriginChannel::open()?;
        let mut cmd=Command::new("/no-path-execution-for-dedicated-sam-worker");
        cmd.env_clear().current_dir("/").stdin(Stdio::from(input)).stdout(Stdio::from(tx)).stderr(Stdio::null());
        // execveat the exact held ELF, close all other FDs and pass no path in
        // argv/environment. The two original root policy FDs stay with us.
        self.originals.release.bind_worker(&mut cmd)?;
        self.terminal.check()?;super::super::cancelled()?;
        let child=cmd.spawn()?;drop(cmd);
        Ok(SelectedWorker{child,channel:Some(channel),grant:Some(self),allowed:false,reaped:false})
    }
    fn accept(&self,r:&Returned<'_>)->io::Result<()>{
        if !std::ptr::eq(self.originals,r.originals)||self.scope!=r.scope||self.originals.scope(&self.reference)?!=self.scope{
            return Err(bad("not the foreground-approved original installation and worker"));}
        self.terminal.check()
    }
    /// The selected protected supervisor may call this only with its own
    /// authenticated worker return. There is no public caller or alternate
    /// material source; cold receipts never re-enter the fresh writer.
    fn publish(self,r:Returned<'a>)->io::Result<[u8;128]>{
        let check=|names:&[&[u8]]|->io::Result<()>{
            super::super::cancelled()?;self.accept(&r)?;r.require_names(names)
        };
        self.originals.release.require()?; // do not sign after an observed change of the authorized installation
        check(FRESH_SITE)?;
        let original=fresh(&r.base).map_err(|_|unconfirmed())?;
        // After allocation there is no unlink/repair/retry. That includes a
        // failed chmod/fsync/rename or a lost final readback: an existing file
        // is evidence to inspect, never permission to restart a new domain.
        let write=||->io::Result<()>{
            check(MIDDLE)?;
            let empty=named(&r.base,Name::Stage,&original,0,0o600)?;
            original.write_all_at(&r.record,0)?;
            let private=named(&r.base,Name::Stage,&original,128,0o600)?;
            contents(&original,&r.record)?;
            if named(&r.base,Name::Stage,&original,128,0o600)?!=private{return Err(bad("private staged binding changed"));}
            fchmod(&original,Mode::RUSR|Mode::RGRP|Mode::ROTH)?;
            let fixed=named(&r.base,Name::Stage,&original,128,0o444)?;
            if fixed.entity!=empty.entity||fixed.mount!=empty.mount{return Err(bad("lost first-created stage"));}
            let parent_before=Times::on(&r.base)?;
            original.sync_all()?;contents(&original,&r.record)?;check(MIDDLE)?;
            if named(&r.base,Name::Stage,&original,128,0o444)?!=fixed||Times::on(&r.base)?!=parent_before{
                return Err(bad("changed original stage or parent before no-replace publication"));}
            // NOREPLACE protects the target. The source belongs to this original,
            // protected host parent under its actual EX; not a source-inode CAS.
            install_name(&r.base)?;
            let final_stamp=named(&r.base,Name::Bound,&original,128,0o444)?;
            if (Stamp{ctime:final_stamp.ctime,..fixed})!=final_stamp{return Err(bad("published another binding inode or changed contents"));}
            let parent_before=Times::on(&r.base)?;check(END)?;
            r.base.sync_all()?;
            // The retained first-created FD and a separately opened original
            // public file must agree after the parent sync. No private TDB read.
            let pin=at(&r.base,Name::Bound)?;
            if Stamp::on(&pin)?!=final_stamp{return Err(bad("different final binding after parent sync"));}
            let other=crate::pinned::sam_public_reader(&pin,final_stamp.mount,128)?;
            for f in [&original,&other]{if Stamp::on(f)?!=final_stamp{return Err(bad("binding changed before readback"));}
                contents(f,&r.record)?;if Stamp::on(f)?!=final_stamp{return Err(bad("binding changed during readback"));}}
            check(END)?;
            if named(&r.base,Name::Bound,&original,128,0o444)?!=final_stamp||Times::on(&r.base)?!=parent_before{
                return Err(bad("final binding or its original parent no longer confirmed"));}
            Ok(())
        };
        write().map_err(|_|unconfirmed())?;
        Ok(r.record)
    }
}
/// Internal precursor to the single setup interface. Nothing calls this from
/// a Web handler or a historical receipt; one live confirmation, one new exec.
pub(super) fn foreground(originals:&HostOrigins,reference:Id)->io::Result<[u8;128]>{
    use helper_linux_evidence::diagnostic_process::{require_waitable_children,watch_cancellation};
    require_waitable_children()?;watch_cancellation()?;
    Grant::foreground(originals,reference)?.start()?.complete()
}
fn unconfirmed()->io::Error{bad("SAM binding publication unconfirmed; retain all originals and inspect this operation, never reinitialize")}
fn fresh(dir:&File)->io::Result<File>{Ok(File::from(openat(dir,Name::Stage.name(),OFlags::CREATE|OFlags::EXCL|OFlags::RDWR
    |OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::RUSR|Mode::WUSR)?))}
fn install_name(dir:&File)->io::Result<()>{
    Ok(renameat_with(dir,Name::Stage.name(),dir,Name::Bound.name(),RenameFlags::NOREPLACE)?)
}
fn at(dir:&File,n:Name)->io::Result<File>{
    Ok(File::from(openat(dir,n.name(),OFlags::PATH|OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::empty())?))
}
fn named(dir:&File,n:Name,original:&File,length:u64,mode:u32)->io::Result<Stamp>{
    let s=Stamp::on(original)?;let p=Stamp::on(dir)?;
    if !s.at(&p,length,mode,0)||Stamp::on(&at(dir,n)?)?!=s{return Err(bad("changed first-created root binding or fixed name"));}Ok(s)
}
fn contents(f:&File,expected:&[u8;128])->io::Result<()>{
    let mut got=[0;128];f.read_exact_at(&mut got,0)?;
    if &got!=expected{return Err(bad("incorrect public binding byte content"));}
    loop{match f.read_at(&mut [0],128){
        Ok(0)=>return Ok(()),Ok(_)=>return Err(bad("unexpected public binding tail")),
        Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(e),
    }}
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn fixed_source_and_destination_never_overwrite_a_partial_or_existing_record(){
        let d=tempfile::tempdir().unwrap();let dir=File::open(d.path()).unwrap();
        let src=fresh(&dir).unwrap();src.write_all_at(&[3;128],0).unwrap();
        assert!(fresh(&dir).is_err());contents(&src,&[3;128]).unwrap();
        std::fs::write(d.path().join("domain.bound"),[4;128]).unwrap();
        assert!(install_name(&dir).is_err());
        contents(&src,&[3;128]).unwrap();assert_eq!(std::fs::read(d.path().join("domain.bound")).unwrap(),[4;128]);
        assert_eq!(crate::pinned::Entity::of(&at(&dir,Name::Stage).unwrap().metadata().unwrap()).unwrap(),
                   crate::pinned::Entity::of(&src.metadata().unwrap()).unwrap());
        // A separate fresh fixture, not automatic cleanup of the failed order.
        let fresh_dir=tempfile::tempdir().unwrap();let dir=File::open(fresh_dir.path()).unwrap();
        let new=fresh(&dir).unwrap();new.write_all_at(&[5;128],0).unwrap();new.sync_all().unwrap();
        install_name(&dir).unwrap();dir.sync_all().unwrap();
        assert!(at(&dir,Name::Stage).is_err());
        let published=File::open(fresh_dir.path().join("domain.bound")).unwrap();
        assert_eq!(crate::pinned::Entity::of(&published.metadata().unwrap()).unwrap(),
                   crate::pinned::Entity::of(&new.metadata().unwrap()).unwrap());
        contents(&published,&[5;128]).unwrap();
        assert!(contents(&published,&[6;128]).is_err());
        new.write_all_at(b"x",128).unwrap();assert!(contents(&published,&[5;128]).is_err());
    }
}
