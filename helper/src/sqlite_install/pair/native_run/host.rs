//! Fixed foreground native authority. The root side owns the actual two
//! OFDs and the original approval, no SQLite connection. The low side owns
//! its Unix SQLite connection and sees only public receipts/O_PATH.
use super::{Id,Intent,Native,Work,ResultRow,choose,original_stamp,bad,release::Release,
    manifest::{Manifest,Role},transport::{self,Channel,Kind},worker::{inspect,stage_body}};
use crate::{fs_tools::RuntimeFile,maintenance::thread_file,smb_profile::{self,first::Origin},sqlite_install::{approval,pair::{
    domain::{NativeSeen,NativeSite,give_live_approval,sealed},native_sql::Original,terminal::{PublicConfirmation,SelectedInput}},
    stage::random}};
use helper_linux_evidence::{diagnostic_process::{OfflineSql,prepare_account_sql_exec,require_waitable_children},
    native_confinement,native_sam::Expected};
use rustix::{fs::{Mode,OFlags,openat2,ResolveFlags},io::Errno,net::UCred,
    process::{Gid,Pid,PidfdFlags,Signal,Uid,pidfd_open,pidfd_send_signal,waitid,WaitId,WaitIdOptions,Resource,Rlimit,getrlimit,setrlimit}};
use std::{fs::File,io::{self,Read},os::{fd::AsFd,unix::ffi::OsStrExt},path::PathBuf,
    process::{Child,Command,Stdio},thread,time::{Duration,Instant}};

const HELPER:&[&str]=&["usr","bin","helper"];
const IDENTITY:&[&str]=&["etc","local-ops-helper","installer.identity"];
const CPU:u64=120;
const MEMORY:u64=512<<20;

pub(super) struct Worker { child:Child, fd:File, pub(super) pid:Pid, done:bool }
impl Worker {
    pub(super) fn spawn(exe:&File,peer:&File,order:&File,mode:OfflineSql)->io::Result<Self>{
        require_waitable_children()?;
        let mut cmd=Command::new("/not-an-account-program"); // only pinned execveat in pre_exec
        cmd.env_clear();
        if matches!(mode,OfflineSql::FirstBrowser|OfflineSql::ExtRepairBrowser){
            let null=helper_linux_evidence::smb_views::null_device()?;
            cmd.stdin(Stdio::from(null.try_clone()?)).stdout(Stdio::from(null.try_clone()?)).stderr(Stdio::from(null));
        }else if matches!(mode,OfflineSql::FirstLayout){cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());}
        prepare_account_sql_exec(&mut cmd,exe,peer,order,mode)?;
        let mut child=cmd.spawn()?;
        let actual=i32::try_from(child.id()).ok().and_then(Pid::from_raw);
        let Some(pid)=actual else{let _=child.kill();let _=child.wait();return Err(bad("invalid exact SQLite child PID"));};
        // This Child alone owns the still-waitable child PID; it cannot be
        // reused before wait. Hard failure here never transfers an FD.
        match pidfd_open(pid,PidfdFlags::empty()){
            Ok(fd)=>Ok(Self{child,fd:File::from(fd),pid,done:false}),
            Err(e)=>{let _=child.kill();let _=child.wait();Err(e.into())}
        }
    }
    fn peek(&self)->io::Result<Option<i32>>{
        loop{match waitid(WaitId::PidFd(self.fd.as_fd()),WaitIdOptions::EXITED|WaitIdOptions::NOHANG|WaitIdOptions::NOWAIT){
            Err(Errno::INTR)=>continue,Ok(s)=>return Ok(s.map(|s|s.exit_status().unwrap_or(-1))),Err(e)=>return Err(e.into()),
        }}
    }
    pub(super) fn live(&self)->io::Result<()>{
        if !self.done&&self.peek()?.is_none(){Ok(())}else{Err(bad("the original SQLite worker ended before the next step"))}
    }
    pub(super) fn finish(&mut self,channel:&Channel,until:Instant)->io::Result<()>{
        loop {
            if Instant::now()>=until{return Err(bad("original credential worker completion timed out"));}
            if let Some(status)=self.peek()?{
                let read=self.child.wait()?;
                self.done=true;
                // A clean result and real independent exit are both
                // required; empty or additional control packets are errors.
                return if status==0&&read.code()==Some(0){channel.eof()}else{Err(bad("original credential worker did not exit normally"))};
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Worker {
    fn cancel(&mut self){
        if self.done{return;}
        // Restore Web's tty guard via TERM before considering force. If the
        // kernel is stalled even a kill-and-wait can be delayed. Forced exit
        // never supplies success, and this process keeps the two originals.
        let _=pidfd_send_signal(&self.fd,Signal::TERM);
        let end=Instant::now()+Duration::from_secs(3);
        while Instant::now()<end&&self.peek().ok().flatten().is_none(){thread::sleep(Duration::from_millis(20));}
        if self.peek().ok().flatten().is_none(){let _=pidfd_send_signal(&self.fd,Signal::KILL);}
        loop{match self.child.wait(){
            Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,
            Ok(_)=>{self.done=true;break},Err(_)=>break,
        }}
    }
}
impl Drop for Worker { fn drop(&mut self){self.cancel();} }
fn current_status()->io::Result<String>{
    let mut s=String::new();thread_file(c"thread-self/status")?.take(65537).read_to_string(&mut s)?;
    if s.len()>65536||!s.ends_with('\n'){return Err(bad("invalid current credential process status"));}
    Ok(s)
}
fn can_create(status:&str)->bool{
    let mut fields=status.lines().filter_map(|l|l.strip_prefix("Umask:\t"));
    let Some(raw)=fields.next()else{return false;};
    raw.len()==4&&raw.starts_with('0')&&raw.bytes().all(|b|matches!(b,b'0'..=b'7'))&&
        fields.next().is_none()&&u32::from_str_radix(raw,8).is_ok_and(|mask|mask&0o600==0)
}
fn require_creation_mask()->io::Result<()>{
    if !can_create(&current_status()?){return Err(bad("the current creation mask would not produce the required new 0600 Samba account store"));}
    Ok(())
}
fn prerequisite()->io::Result<()>{
    let s=current_status()?;
    let mut e=s.lines().filter_map(|l|l.strip_prefix("CapEff:"));
    let cap=e.next().and_then(|v|u64::from_str_radix(v.trim_ascii(),16).ok());
    // DAC override/read plus root process-control rights; inadequate signal
    // authority cannot start a worker it would be unable to reap on failure.
    if e.next().is_some()||!cap.is_some_and(|v|v&(1<<5)!=0&&v&6!=0){
        return Err(bad("the site cannot verify or collect this original credential process"));
    }Ok(())
}
fn confined(site:&NativeSite,release:&Release,dir:&File,machine:&File,child:&File)->io::Result<()>{
    let identity=RuntimeFile::open(IDENTITY,false,128)?;
    let metadata=[
        thread_file(c"thread-self/status")?,thread_file(c"thread-self/uid_map")?,thread_file(c"thread-self/gid_map")?,
        thread_file(c"thread-self/maps")?,
    ];
    let fips=match thread_file(c"sys/crypto/fips_enabled"){Ok(f)=>Some(f),Err(e) if e.kind()==io::ErrorKind::NotFound=>None,Err(e)=>return Err(e)};
    let (proc,_tmp)=approval::own_descriptor_table()?;
    let info=File::from(openat2(&proc,c"thread-self/fdinfo",OFlags::PATH|OFlags::DIRECTORY|OFlags::NOFOLLOW|OFlags::CLOEXEC,
        Mode::empty(),ResolveFlags::BENEATH|ResolveFlags::NO_MAGICLINKS|ResolveFlags::NO_XDEV)?);
    let mut allowed=release.reads();
    allowed.extend(site.public_pins());allowed.push(machine);
    allowed.push(&identity.file);allowed.extend(metadata.iter());allowed.extend(fips.iter());
    for (ty,size) in [(Resource::As,MEMORY),(Resource::Cpu,CPU),(Resource::Fsize,64<<20)]{
        let existing=getrlimit(ty);
        let n=existing.maximum.map_or(size, |v|v.min(size));
        setrlimit(ty,Rlimit{current:Some(existing.current.map_or(n,|v|v.min(n))),maximum:Some(n)})?;
    }
    native_confinement::apply(site.host_root(),dir,&allowed,&info,child)
}
#[derive(Debug)]
pub(in crate::sqlite_install::pair) struct Selection {
    pub(in crate::sqlite_install::pair) system:PathBuf,pub(in crate::sqlite_install::pair) pool:Id,pub(in crate::sqlite_install::pair) init:Id,pub(in crate::sqlite_install::pair) instance:Id,pub(in crate::sqlite_install::pair) provision:Id,
    pub(in crate::sqlite_install::pair) op:Id,pub(in crate::sqlite_install::pair) reference:Id,pub(in crate::sqlite_install::pair) role:Role,pub(in crate::sqlite_install::pair) intent:Intent,
    pub(in crate::sqlite_install::pair) bound_sha:[u8;32],
}

fn review(m:&Manifest,row:ResultRow,work:Work,digest:[u8;32])->String{
    let hex=|b:&[u8]|b.iter().map(|v|format!("{v:02x}")).collect::<String>();
    let esc=|b:&[u8]|b.iter().copied().flat_map(std::ascii::escape_default).map(char::from).collect::<String>();
    let roles=match (&m.role,row) {
        (Role::Birth{web,smb},ResultRow::Setup(ids))=>format!("本池首次设置两种身份：Web {} / {}，SMB {} / {}。",
            esc(web),hex(&ids.web_value()),esc(smb),hex(&ids.smb_value())),
        (Role::Reset{setup,account,previous,generation,smb},ResultRow::Reset(target))=>
            format!("仅重设 SMB {} / {}。首设 {}，准确旧凭据 {}（第 {} 代）→第 {} 代；不改 Web。",
                esc(smb),hex(account),hex(setup),hex(previous),generation,target),
        _=>String::new(),
    };
    let (explain,publication)=match (&m.role,work) {
        (Role::Birth{..},Work::Confirm)=>("在同一原批准内复核并同步原账户，确认待办及以下原首发；不重问口令。",
            smb_profile::approval_text(&m.plan)),
        (Role::Birth{..},_)=>("离线首设：仅为当次新账户授权以下普通数据盘目录；无来宾。这不是 Web 登录或启动服务。",
            smb_profile::approval_text(&m.plan)),
        (Role::Reset{..},Work::Confirm)=>("仅重新核验、同步原目标并确认原待办；不询问密码，不改共享或服务。",String::new()),
        _=>("仅按原单重设 SMB 口令，不改共享或服务。",String::new()),
    };
    format!("{roles}\n{explain}\n{publication}若中断保留两原件并查原单，不重新发行。\n池 {} / 原 INIT {}；专用实例 {} / 原域 {}；SYSTEM {} [dev {}, inode {}, mount {}]。\n本次原操作 {}，真实维护主体 {}，审批参考 {}，请求摘要 {}。\n",
        hex(&m.pool),hex(&m.init),hex(&m.instance),hex(&m.provision),esc(m.system.as_os_str().as_bytes()),
        m.origin[0],m.origin[1],m.origin[2],hex(&m.op),hex(&m.actor),hex(&m.approval),hex(&digest))
}

pub(in crate::sqlite_install::pair) fn run(selected:Selection)->io::Result<Original<ResultRow>>{
    let setup=match &selected.role{Role::Birth{..}=>selected.op,Role::Reset{setup,..}=>*setup};
    let site=NativeSite::acquire(&selected.system,Origin{pool:selected.pool,init:selected.init,instance:selected.instance,
        domain:selected.provision,setup},&selected.bound_sha)?;
    prerequisite()?;
    let terminal=PublicConfirmation::select()?;
    let helper=RuntimeFile::open(HELPER,true,64<<20)?;
    helper.require_running_helper()?;
    let (actor,uid,gid)=site.subject();
    let mut invocation=[0;32];invocation[..16].copy_from_slice(&random()?);invocation[16..].copy_from_slice(&random()?);
    let m=Manifest{intent:selected.intent,invocation,pool:selected.pool,init:selected.init,instance:selected.instance,
        provision:selected.provision,op:selected.op,actor,approval:selected.reference,live:site.live_digest(),
        bound:site.bound_bytes(),system:selected.system,origin:site.system_location()?,role:selected.role,plan:site.first_plan().clone()};
    let order=m.order()?;let digest=order.sha();let scope=m.scope()?;
    let meta=sealed(&m.encode()?)?;
    let (parent,child)=transport::pair()?;
    let mut worker=Worker::spawn(&helper.file,&child,&meta,OfflineSql::Account)?;
    drop(child);drop(meta);
    let given=UCred{pid:worker.pid,uid:Uid::from_raw(uid),gid:Gid::from_raw(gid)};
    // This first reply only allows a read-only observation from the original
    // S. There is separately no Act without this foreground operation grant.
    let entry_until=Instant::now()+Duration::from_secs(15);
    loop {
        if Instant::now()>=entry_until{return Err(bad("the original SQLite worker did not confirm its identity"));}
        worker.live()?;
        if give_live_approval(&parent,given,&scope,||{site.require()?;terminal.check()?;helper.require_running_helper()})?{break;}
        thread::sleep(Duration::from_millis(20));
    }
    let channel=Channel::from_owned_child_session(parent,given,scope)?;
    channel.send_original(Kind::Lease,&digest,&[0;65],site.system_lease())?;
    let total=Instant::now()+Duration::from_secs(11*60);
    let (stage,[])=channel.wait::<0>(Kind::Inspect,&digest,Instant::now()+Duration::from_secs(15))?;
    let (which,row)=inspect(&stage)?;
    let existing=match(which,row){
        (0,None)|(1,Some(_))=>Original::Absent,(2,Some(r))=>Original::Pending(r),(3,Some(r))=>Original::Committed(r),
        _=>return Err(bad("wrong original SQLite status result")),
    };
    if m.intent==Intent::Lookup||matches!(existing,Original::Committed(_))||which==0||which==2&&m.intent==Intent::New{
        worker.finish(&channel,Instant::now()+Duration::from_secs(5))?;site.require()?;
        return Ok(existing);
    }
    let row=row.ok_or_else(||bad("no exact original SQL operation proposal"))?;
    let reset=matches!(&m.role,Role::Reset{..});
    let (markers,login)=order.target(row)?;
    let (previous,target)=markers.fields();let want=Expected::for_marker(login,previous,target)?;
    let release=Release::authorize(m.pool,m.instance,m.provision,m.live)?;
    let choice=SelectedInput::before_drop()?;
    let accounts=site.accounts()?;let dir=accounts.leaf_pin()?;
    site.require_write()?;
    let machine=site.original_machine()?;let (instance,private,expected)=site.machine_scope();
    confined(&site,&release,&dir,&machine,&worker.fd)?;
    choice.lock_native_memory()?;
    let (native,crypto)=release.load()?;
    let valid=|worker:&Worker|->io::Result<()> {
        if Instant::now()>=total{return Err(bad("foreground credential maintenance expired"));}
        worker.live()?;site.require_write()?;site.require_machine(&machine)?;release.loaded(&native,&crypto)?;
        native.domain(instance,private,&machine,uid,gid,expected)
    };
    valid(&worker)?;
    let before=accounts.observe()?;
    let seen=match &before {
        NativeSeen::NotPresent=>Native::Missing,
        NativeSeen::Present(file)=>if native.inspect(instance,&dir,file.pin(),&want)?{Native::Target}else{Native::Previous},
    };
    let action=choose(m.intent,existing,seen,reset)?;
    if action==Work::Create{require_creation_mask()?;}
    terminal.confirm_native(reset,review(&m,row,action,digest))?;
    valid(&worker)?;
    let reserve_order=|worker:&Worker|->io::Result<()>{
        valid(worker)?;
        if action==Work::Create{require_creation_mask()?;}
        if let NativeSeen::Present(original)=&before{original.require()?;}
        channel.send(Kind::Act,&digest,&stage_body(which,Some(row)))
    };
    let reserved=|packet| if packet==stage_body(2,Some(row)){Ok(())}
        else{Err(bad("no confirmed original SQLite reservation; do not change the native store"))};
    let material=if action==Work::Create{
        // Allocate *both* protected provider environments before asking for
        // either password. Privileged and low child memory never crosses.
        channel.send(Kind::Warm,&digest,&stage_body(which,Some(row)))?;
        let (warm,[])=channel.wait::<0>(Kind::Prepared,&digest,Instant::now()+Duration::from_secs(30))?;
        if warm!=stage_body(which,Some(row)){return Err(bad("the actual Web worker did not prepare its workspace"));}
        let provider=crypto.qualify()?;
        let stdin=io::stdin();let quiet=choice.hold_for_birth(&stdin)?;
        let attempt=(||->io::Result<_>{
            let secret=quiet.smb(provider)?;
            quiet.require()?;reserve_order(&worker)?;
            let (report,[])=channel.wait_checked::<0>(Kind::Reserved,&digest,total,||quiet.require())?;
            reserved(report)?;
            Ok(secret)
        })();
        if attempt.is_err(){worker.cancel();}
        // Normal exit restores exactly once after both readers have stopped.
        // Errors cancel/reap the original low reader before restoring as well.
        quiet.finish()?;
        Some(attempt?)
    }else{
        let secret=match action{
            Work::Change=>Some(choice.smb_only(crypto.qualify()?)?),
            Work::Confirm=>None,_=>return Err(bad("no native update authorized")),
        };
        terminal.check()?;reserve_order(&worker)?;
        let (report,[])=channel.wait::<0>(Kind::Reserved,&digest,total)?;reserved(report)?;
        secret
    };
    terminal.check()?;valid(&worker)?;
    let returned = match (action,&before,material) {
        (Work::Create,NativeSeen::NotPresent,Some(smb))=>{
            require_creation_mask().map_err(|_|uncertain())?;
            if !matches!(accounts.observe()?,NativeSeen::NotPresent){return Err(uncertain());}
            smb.native_change(&native,instance,&dir,None,&want).map_err(|_|uncertain())?
        }
        (Work::Change,NativeSeen::Present(old),Some(smb))=>{
            old.require()?;smb.native_change(&native,instance,&dir,Some(old.pin()),&want).map_err(|_|uncertain())?
        }
        (Work::Confirm,NativeSeen::Present(old),None)=>{old.require()?;old.pin().try_clone()?},
        _=>return Err(uncertain()),
    };
    let proof=(||->io::Result<_>{
        valid(&worker)?;
        let before=original_stamp(&returned)?;
        native.sync_target(instance,&dir,&returned,&want)?;
        valid(&worker)?;
        let NativeSeen::Present(current)=accounts.observe()?else{return Err(uncertain());};
        current.require()?;
        if original_stamp(current.pin())?!=before||original_stamp(&returned)?!=before{return Err(uncertain());}
        let mut packet=[0;65];packet[..33].copy_from_slice(&row.wire());packet[33..].copy_from_slice(&before);
        channel.send_original(Kind::NativeSynced,&digest,&packet,&returned)?;
        let (complete,[])=channel.wait::<0>(Kind::Complete,&digest,total)?;
        if complete!=stage_body(3,Some(row)){return Err(uncertain());}
        worker.finish(&channel,total)?;
        site.require()?;current.require()?;
        if original_stamp(&returned)?!=before{return Err(uncertain());}
        Ok(Original::Committed(row))
    })();
    proof.map_err(|_|uncertain())
}
fn uncertain()->io::Error{bad("credential result uncertain; retain both stores and query only the exact original operation")}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn never_reserve_a_new_setup_with_a_mask_that_prevents_0600(){
        for mask in ["0000","0022","0077"]{assert!(can_create(&format!("Umask:\t{mask}\n")));}
        for mask in ["0200","0400","0677","1777"," 0077","0080"]{assert!(!can_create(&format!("Umask:\t{mask}\n")));}
        assert!(!can_create("CapEff:\t0\n"));
        assert!(!can_create("Umask:\t0022\nUmask:\t0077\n"));
    }
}
