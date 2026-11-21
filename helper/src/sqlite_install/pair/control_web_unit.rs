//! Explicitly control only the separately installed native HTTP fragment.
//! Start may share the original S lease with the online Samba pair; stop
//! must remain usable with a busy Web database and never attempts a SQL write.
use super::{Id,bad,command::{key,sha,value},domain::require_lent_sh,
    inactive_web_unit::{Fragment,Target,UNIT,require_amd64_elf},native_run::verify_web_unit,
    terminal::PublicConfirmation,web_systemd::Manager,web_systemd_state::State,web_unit_cgroup,web_unit_process::Process};
use crate::{fs_tools::{RuntimeFile,hash_upto},pinned::{Kind,Pinned},smb_profile,sqlite_install::{approval,markers::LiveSystemMark}};
use rustix::fs::{FlockOperation,flock};
use std::{ffi::OsString,io,os::unix::{ffi::OsStrExt,fs::MetadataExt},path::PathBuf,time::{Duration,Instant}};

const PROGRAM:&[&str]=&["usr","bin","zxs"];
const LIMIT:u64=128<<20;
fn no()->io::Error{bad("the loaded Web unit, original host or selected pool changed")}
fn hex(x:&[u8])->String{x.iter().map(|b|format!("{b:02x}")).collect()}
#[derive(Debug,Clone,Copy)]
enum Action{Start{init:Id,setup:Id,reference:Id},Stop{reference:Id},Status}
#[derive(Debug)]
pub(crate) struct Control{action:Action,system:PathBuf,pool:Id,elf:[u8;32]}
impl Control{
    pub(crate) fn parse(mut a:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        let verb=a.next().and_then(|v|v.into_string().ok()).ok_or("choose Web unit start, stop or status")?;
        if !matches!(verb.as_str(),"start"|"stop"|"status"){return Err("choose Web unit start, stop or status");}
        let system=PathBuf::from(value(&mut a,"--system")?);
        Kind::StorageRoot.check_path(system.as_os_str().as_bytes())?;
        let pool=key(&mut a,"--pool")?;
        let action=match verb.as_str(){
            "start"=>Action::Start{init:key(&mut a,"--init")?,setup:key(&mut a,"--setup-op")?,reference:key(&mut a,"--approval")?},
            "stop"=>Action::Stop{reference:key(&mut a,"--approval")?},
            _=>Action::Status,
        };
        let elf=sha(&mut a,"--zxs-sha256")?;
        if a.next().is_some(){return Err("no alternate unit, automatic reload, enable, restart or network options");}
        Ok(Self{action,system,pool,elf})
    }
    pub(crate) fn run(self)->io::Result<()>{
        approval::no_inherited_descriptors()?;
        let host=approval::host_admin()?;let subject=approval::profile()?;let term=PublicConfirmation::select()?;
        let pin=Pinned::open(&self.system,Kind::StorageRoot)?;let share=pin.storage_directory()?;
        flock(&share,FlockOperation::NonBlockingLockShared)?;
        let qualified=||{
            term.check()?;approval::host_admin()?;
            let m=share.metadata()?;
            if approval::profile()?!=subject||m.uid()!=subject.1||m.mode()!=0o040700||m.nlink()==0{return Err(no());}
            require_lent_sh(&share,(pin.entity(),pin.require_same_named_mount()?.id))
        };
        qualified()?;
        let mark=LiveSystemMark::select(&share,subject.1,&self.pool)?;
        let check=||{qualified()?;mark.verify(&share)};
        check()?;
        if let Action::Start{init,setup,..}=self.action{
            verify_web_unit(&pin,&share,subject,[self.pool,init,setup],true,&check)?;
        }
        let mut program=RuntimeFile::open(PROGRAM,true,LIMIT)?;let n=program.file.metadata()?.len();
        require_amd64_elf(&program.file)?;
        if hash_upto(&mut program.file,n,LIMIT)?!=self.elf{return Err(no());}
        program.recheck()?;
        let target=Target::select(&host)?;
        let fragment=Fragment::render(pin.path(),&self.pool,subject.1,subject.2)?;
        let installed=target.require_installed(&fragment,None)?;
        let full=|| -> io::Result<()>{
            check()?;program.recheck()?;
            target.require_installed(&fragment,Some(&installed))?;Ok(())
        };
        let manager=Manager::select()?;
        let read=||{full()?;manager.read(pin.path(),&self.pool,subject.1,subject.2,&check)};
        let current=read()?;
        let actual=|pid|->io::Result<Process>{
            let p=Process::select(pid)?;p.require(&program.file,&pin,mark.entity(),subject.1,subject.2)?;Ok(p)
        };
        let running=match current{
            State::Inactive=>None,State::Running{pid,..}=>Some(actual(pid)?),
            State::Draining=>return Err(bad("the fixed native Web unit is draining; no new action was requested")),
        };
        let group=match (self.action,current){
            (Action::Stop{..},State::Running{pid,..})=>Some((pid,web_unit_cgroup::Original::select(pid)?)),
            _=>None,
        };
        if let Action::Status=self.action{
            if read()?!=current{return Err(no());}
            if let Some(p)=&running{p.require(&program.file,&pin,mark.entity(),subject.1,subject.2)?;}
            return term.report(match current{
                State::Inactive=>"web-unit=inactive；已核此固定单元未运行；未核池是否还有其它在线参与者。\n",
                State::Running{..}=>"web-unit=running；已核此固定单元的本次主进程；未验 HTTP 监听和登录。\n",
                State::Draining=>return Err(no()),
            });
        }
        let (start,purpose,reference)=match (self.action,current){
            (Action::Start{reference,..},State::Inactive)=>(true,"只通过真实 PID 1 启动该单元。原生管理 HTTP 如成功绑定，将使用所有 IPv4 接口的 TCP 8443；不配置防火墙、DNS 或 TLS。",reference),
            (Action::Stop{reference},State::Running{..})=>(false,"只向下面已核准的本次 Web 主进程发送一次正常终止信号；不按单元名字停止替代进程，不停 SMB 或其它在线参与者。",reference),
            _=>return Err(bad("choose the action matching the current stable state; no automatic reload, restart or takeover")),
        };
        let mut pretty=String::new();smb_profile::escaped(&mut pretty,pin.path().as_os_str().as_bytes());
        let active=match current{State::Inactive=>"当前由 PID 1 报告未运行。".to_owned(),
            State::Running{pid,invocation}=>format!("正在运行的原实例 PID {}，本次 InvocationID {}。",pid.as_raw_pid(),hex(&invocation)),
            State::Draining=>return Err(no()),
        };
        let preview=format!("{purpose}\n固定单元 {UNIT}。{active}\n原 SYSTEM \"{pretty}\"（dev {} / inode {} / mount {}），存储 UID/GID {}/{}，原池 {}。\n本机维护主体 {}，审批参考 {}；主片段 SHA-256 {}；已核准的 ZXS ELF SHA-256 {}。\n",
            pin.entity().dev,pin.entity().ino,pin.require_same_named_mount()?.id,subject.1,subject.2,hex(&self.pool),
            hex(&subject.0),hex(&reference),hex(&fragment.sha()),hex(&self.elf));
        term.confirm_web_control(start,preview)?;
        if let Action::Start{init,setup,..}=self.action{
            verify_web_unit(&pin,&share,subject,[self.pool,init,setup],true,&check)?;
        }
        if read()?!=current{return Err(no());}
        let cut=|| -> io::Result<()>{full()?;if let Some(p)=&running{p.require(&program.file,&pin,mark.entity(),subject.1,subject.2)?;}Ok(())};
        cut()?;
        let mut stop_deadline=None;
        if start{
            manager.start(&check)?;
        }else{
            let original=running.as_ref().ok_or_else(no)?;
            let (pid,cgroup)=group.as_ref().ok_or_else(no)?;
            cgroup.sole_process(*pid)?;original.require(&program.file,&pin,mark.entity(),subject.1,subject.2)?;
            // StopUnit accepts a name, not an InvocationID. Do not let a
            // concurrent stop/start replace the process the operator approved.
            original.terminate()?;
            let until=Instant::now()+Duration::from_secs(60);stop_deadline=Some(until);
            while !original.exited()?||!cgroup.drained()?{
                check()?;
                if Instant::now()>=until{
                    return Err(io::Error::new(io::ErrorKind::TimedOut,"the original Web process or its group did not finish; no forced stop was sent"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        // A manager job or a signal is not evidence of the final state.
        // There is no automatic retry or compensation after lost response.
        let after=loop{
            let state=read().map_err(|_|bad("the actual service result is unconfirmed; inspect status before retrying"))?;
            if state!=State::Draining||stop_deadline.is_none_or(|end|Instant::now()>=end){break state;}
            std::thread::sleep(Duration::from_millis(80));
        };
        match (start,after,running){
            (true,State::Running{pid,..},None)=>{
                let p=Process::select(pid)?;
                let until=Instant::now()+Duration::from_secs(10);
                loop{
                    full()?;
                    match p.require(&program.file,&pin,mark.entity(),subject.1,subject.2){
                        Ok(())=>break,
                        Err(e) if e.kind()==io::ErrorKind::WouldBlock&&Instant::now()<until=>std::thread::sleep(Duration::from_millis(40)),
                        Err(e)=>return Err(e),
                    }
                }
                if read()?!=after{return Err(no());}
                p.require(&program.file,&pin,mark.entity(),subject.1,subject.2)?;
                let _=term.report("web-unit=started；同一单元的实际主进程已确认，未验 HTTP 监听和登录。\n");
            }
            (false,State::Inactive,Some(p)) if p.exited()?=>{
                if read()?!=State::Inactive||!group.as_ref().ok_or_else(no)?.1.drained()?{return Err(no());}
                let _=term.report("web-unit=stopped；原主进程已退出且原进程组无残留，这不证明池已离线或 SMB 已停止。\n");
            }
            _=>return Err(bad("the action returned but its actual native result was not confirmed; do not repeat automatically")),
        }
        Ok(())
    }
}
