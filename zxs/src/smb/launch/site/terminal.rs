//! A bounded fresh public confirmation on the same foreground slave or
//! VT. It never reads passwords, changes termios or accepts a caller path.
use super::{SiteLease,denied,held::{fd_mount,proc_dir,proc_pin},hex,media::Media};
use crate::smb::launch::{custody::{Parent,VerifiedPreflight},kernel};
use std::{ffi::CStr,fmt::Write as _,fs::File,io::{self,Read,Write},os::{fd::{AsRawFd,RawFd},unix::fs::MetadataExt},thread,time::{Duration,Instant}};

#[derive(Clone,Copy,PartialEq,Eq)]
struct Surface{device:u64,inode:u64,rdev:u64,mount:u64}
impl Surface{
    fn read(fd:RawFd)->io::Result<Self>{
        // SAFETY: isatty observes only this live local fd.
        if unsafe{libc::isatty(fd)}!=1{return Err(denied());}
        Self::identity(fd)
    }
    fn identity(fd:RawFd)->io::Result<Self>{
        // SAFETY: fstat writes this initialized C output.
        let mut st:libc::stat=unsafe{std::mem::zeroed()};
        if unsafe{libc::fstat(fd,&mut st)}!=0||
            st.st_mode&libc::S_IFMT!=libc::S_IFCHR||st.st_ino==0||
            (libc::major(st.st_rdev),libc::minor(st.st_rdev))==(5,0){
            return Err(denied());
        }
        Ok(Self{device:st.st_dev as u64,inode:st.st_ino as u64,rdev:st.st_rdev as u64,mount:fd_mount(fd)?})
    }
}
pub(in crate::smb::launch) struct Terminal{input:File,output:File,original:Surface,session:i32,group:i32}
impl Terminal{
    pub(in crate::smb::launch) fn select(parent:&Parent)->io::Result<Self>{
        let task=parent.own_task().map_err(|_|denied())?;
        let fds=proc_dir(&task,c"fd")?;
        let original=Surface::read(0)?;
        if [1,2].into_iter().any(|fd|Surface::read(fd).ok()!=Some(original))||!access(0,false)?||!access(2,true)?{
            return Err(denied());
        }
        let options=libc::O_NONBLOCK|libc::O_NOCTTY;
        // Reopen two genuine old stdio links, not dup (would share O_NONBLOCK)
        // and never a device name. Original access rights were already checked.
        let input=open_terminal(&fds,c"0",options|libc::O_RDONLY,original)?;
        let output=open_terminal(&fds,c"2",options|libc::O_WRONLY,original)?;
        // SAFETY: integers query the actual calling session/process group.
        let session=unsafe{libc::getsid(0)};let group=unsafe{libc::getpgrp()};
        if session<=0||group<=0{return Err(denied());}
        let t=Self{input,output,original,session,group};
        t.require()?;Ok(t)
    }
    pub(in crate::smb::launch) fn require(&self)->io::Result<()>{
        let i=self.input.as_raw_fd();let o=self.output.as_raw_fd();
        if !access(0,false)?||!access(2,true)?||
            [0,1,2,i,o].into_iter().any(|fd|Surface::read(fd).ok()!=Some(self.original)){
            return Err(denied());
        }
        let flags=libc::O_ACCMODE|libc::O_NONBLOCK|libc::O_PATH;
        if kernel::get_flags(i,libc::F_GETFL).map_err(|_|denied())?&flags!=(libc::O_RDONLY|libc::O_NONBLOCK)||
            kernel::get_flags(o,libc::F_GETFL).map_err(|_|denied())?&flags!=(libc::O_WRONLY|libc::O_NONBLOCK){
            return Err(denied());
        }
        // SAFETY: read foreground identity and termios on our owned real tty.
        let mut t:libc::termios=unsafe{std::mem::zeroed()};
        if unsafe{libc::tcgetpgrp(i)}!=self.group||unsafe{libc::getpgrp()}!=self.group||
            unsafe{libc::tcgetsid(i)}!=self.session||unsafe{libc::getsid(0)}!=self.session||
            unsafe{libc::tcgetattr(i,&mut t)}!=0||t.c_line!=0||t.c_lflag&libc::ICANON==0{
            return Err(denied());
        }
        Ok(())
    }
    /// Render only the real matched ACTIVE reply and actual selected media.
    /// The one-shot grant cannot be reused for a different reply or view.
    pub(in crate::smb::launch) fn approve<'t,'m,'v,'s,'p>(
        &'t self,site:&'s SiteLease<'p>,selected:&'v VerifiedPreflight<'s,'p>,media:&'m Media<'v,'s,'p>,
        live:impl Fn()->io::Result<()>,
    )->io::Result<StartApproval<'t,'m,'v,'s,'p>>{
        site.require()?;live()?;self.require()?;
        media.require_for(site,selected)?;
        let facts=selected.qualified_for(site)?;
        let plan=selected.plan();
        let mut detail=format!("本配置的直接 Web 监听 0.0.0.0:8443。\n站点 {}，唯一共享 ZXS；原 SMB 用户 {}（第 {} 代），禁用来宾。\n",
            escaped(plan.hostname().as_bytes()),escaped(facts.epoch.login.as_bytes()),facts.epoch.generation);
        if plan.planned_folders().len()!=facts.ordinary.len(){return Err(denied());}
        for &(ip,prefix) in plan.endpoints(){
            writeln!(detail,"  监听 {}；仅限相应客户前缀 {ip}/{prefix}。",
                std::net::SocketAddr::new(ip,445)).expect("write to String");
        }
        for (i,((volume,label,rw),claim)) in plan.planned_folders().zip(&facts.ordinary).enumerate(){
            if volume!=claim.volume{return Err(denied());}
            writeln!(detail,"  普通数据卷 {}，目录 \"{}\"，原首设{}；{}。",
                hex(&volume.0),escaped(label.as_bytes()),if rw{"读写"}else{"只读"},
                if media.has_ordinary(i){"原件已核，权限按当前原池"}else{"未核，不投射"}).expect("write to String");
        }
        writeln!(detail,"  专用回收目录 \"{}\"；{}。",escaped(plan.recycle_label().as_bytes()),
            if media.recycle_available(){"同一原件和独立文件系统已核"}else{"本次未核，不投射，不判定空或健康"}).expect("write to String");
        if detail.len()>96*1024{return Err(denied());}
        let (system,_,p)=site.system()?;let scope=site.origin();let actor=site.subject();
        let mut random=[0u8;16];
        // SAFETY: a bounded output array; no fallback or user supplied token.
        if unsafe{libc::getrandom(random.as_mut_ptr().cast(),random.len(),libc::GRND_NONBLOCK)}!=random.len() as isize||
            random==[0;16]{return Err(denied());}
        let answer=format!("RUN-ZXS {}\n",hex(&random));
        let summary=format!(
            "只启动本次前台运行的整套 ZXS，不迁移池、不改密码、宿主挂载、防火墙或系统服务。已批准操作的原待办可能在真正就绪后继续核对或清退。\n\
             Pool {} / INIT {} / Setup {} / Instance {} / Bind {}。\n\
             原 SYSTEM \"{}\" [dev {}, inode {}, mount {}]；原 INIT 主体 {}，存储 UID/GID {}/{}。\n\
             原计划 SHA-256 {}；已核准的同一个 ZXS ELF \"{}\" SHA-256 {}。\n{detail}\n",
            hex(&scope.pool),hex(&scope.init),hex(&scope.setup),hex(&scope.instance),hex(&scope.bind),
            escaped(system.as_os_str().as_encoded_bytes()),p.entity.device,p.entity.inode,p.mount,hex(&actor.init_actor),actor.uid,actor.gid,
            hex(&site.install_sha()),escaped(site.image_path().as_os_str().as_encoded_bytes()),hex(&site.image_sha()));
        let deadline=Instant::now()+Duration::from_secs(120);
        let check=||{live()?;self.require()};
        check()?;self.flush_input()?;
        let outcome=(||->io::Result<()>{
            write_until(&self.output,summary.as_bytes(),deadline,&check)?;
            self.flush_input()?;
            let prompt=format!("同意以上原件与范围时输入 {answer}此处不输入口令。\n");
            write_until(&self.output,prompt.as_bytes(),deadline,&check)?;
            let mut input=[0;96];
            loop{
                check()?;later_than(deadline)?;
                match (&self.input).read(&mut input){
                    Ok(n) if &input[..n]==answer.as_bytes()=>return Ok(()),
                    Ok(_)=>return Err(denied()),
                    Err(e) if retry(&e)=>{},
                    Err(e)=>return Err(e),
                }
            }
        })();
        self.flush_input()?;outcome?;
        check()?;site.require()?;media.require_for(site,selected)?;
        selected.qualified_for(site)?;
        Ok(StartApproval{terminal:self,site,selected,media})
    }
    pub(in crate::smb::launch) fn report(&self,msg:&str,check:impl Fn()->io::Result<()>)->io::Result<()>{
        if msg.len()>12*1024{return Err(denied());}
        write_until(&self.output,msg.as_bytes(),Instant::now()+Duration::from_secs(2),&||{check()?;self.require()})
    }
    fn flush_input(&self)->io::Result<()>{
        // SAFETY: discard only queued input on the same retained selected tty.
        self.require()?;
        if unsafe{libc::tcflush(self.input.as_raw_fd(),libc::TCIFLUSH)}!=0{return Err(io::Error::last_os_error());}
        Ok(())
    }
}
#[must_use]
pub(in crate::smb::launch) struct StartApproval<'t,'m,'v,'s,'p>{
    terminal:&'t Terminal,site:&'s SiteLease<'p>,
    selected:&'v VerifiedPreflight<'s,'p>,media:&'m Media<'v,'s,'p>,
}
impl<'v,'s,'p> StartApproval<'_,'_,'v,'s,'p>{
    pub(in crate::smb::launch) fn consume_for(
        self,site:&SiteLease<'p>,selected:&VerifiedPreflight<'s,'p>,media:&Media<'v,'s,'p>,
    )->io::Result<()>{
        if !std::ptr::eq(self.site,site)||!std::ptr::eq(self.selected,selected)||!std::ptr::eq(self.media,media){
            return Err(denied());
        }
        self.terminal.require()?;self.site.require()?;
        self.selected.qualified_for(site)?;
        self.media.require_for(site,selected)
    }
}
fn escaped(value:&[u8])->String{
    let Ok(text)=std::str::from_utf8(value)else{
        return value.iter().copied().flat_map(std::ascii::escape_default).map(char::from).collect();
    };
    let mut out=String::new();
    for c in text.chars(){match c{
        '"'|'\\'=>{out.push('\\');out.push(c);}
        c if c.is_control()||matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')=>{
            write!(out,"{}",c.escape_default()).expect("write to String");
        }
        c=>out.push(c),
    }}
    out
}
fn access(fd:RawFd,write:bool)->io::Result<bool>{
    let f=kernel::get_flags(fd,libc::F_GETFL).map_err(|_|denied())?;
    let a=f&libc::O_ACCMODE;
    Ok(f&libc::O_PATH==0&&(a==libc::O_RDWR||a==(if write{libc::O_WRONLY}else{libc::O_RDONLY})))
}
fn open_terminal(fds:&File,name:&CStr,flags:i32,expect:Surface)->io::Result<File>{
    if !matches!(name.to_bytes(),b"0"|b"2"){return Err(denied());}
    let link=proc_pin(fds,name)?;
    let link_stamp=link.metadata()?;
    if !link_stamp.is_symlink(){return Err(denied());}
    let follow=|flags|->io::Result<File>{
        // SAFETY: only literal 0/2 in the proved, retained own task fd table;
        // pre-existing exact stdio rights/types and fresh returned descriptor.
        unsafe{
            let k=libc::openat(fds.as_raw_fd(),name.as_ptr(),flags|libc::O_CLOEXEC);
            Ok(File::from(kernel::new_fd(k.into()).map_err(|_|denied())?))
        }
    };
    let pin=follow(libc::O_PATH)?;
    if Surface::identity(pin.as_raw_fd())?!=expect{return Err(denied());}
    let out=follow(flags)?;
    let now=proc_pin(fds,name)?.metadata()?;
    if Surface::read(out.as_raw_fd())?!=expect||Surface::identity(pin.as_raw_fd())?!=expect||
       !now.is_symlink()||(now.dev(),now.ino())!=(link_stamp.dev(),link_stamp.ino()){return Err(denied());}
    Ok(out)
}
fn write_until(mut out:&File,mut text:&[u8],deadline:Instant,check:&impl Fn()->io::Result<()>)->io::Result<()>{
    while !text.is_empty(){
        check()?;later_than(deadline)?;
        match out.write(text){
            Ok(0)=>return Err(denied()),Ok(n)=>text=&text[n..],
            Err(e) if retry(&e)=>{},Err(e)=>return Err(e),
        }
    }
    Ok(())
}
fn later_than(deadline:Instant)->io::Result<()>{
    if Instant::now()>=deadline{return Err(io::Error::new(io::ErrorKind::TimedOut,"ZXS foreground confirmation expired"));}
    Ok(())
}
fn retry(e:&io::Error)->bool{
    match e.kind(){
        io::ErrorKind::Interrupted=>true,
        io::ErrorKind::WouldBlock=>{thread::sleep(Duration::from_millis(20));true}
        _=>false,
    }
}
