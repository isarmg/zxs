//! Internal online-launch handoff, separate from offline SAM creation and from Web.
//! Pair creation and teardown operate only on the two internally born roles.
//! Only a new local foreground grant plus the exact release can start it.
mod process;
mod origins;
mod identity;
mod supervisor;
mod release;
use crate::sqlite_install::Online;
use crate::pinned::{Entity,Mount};
use helper_linux_evidence::{diagnostic_process as signals,smb_process::initially_no_sender_pidfd,
    smb_views::{self,Access,Source}};
use rustix::{fs::{OFlags,fcntl_getfl,fstatfs},net::{AddressFamily,SocketFlags,SocketType,
    socketpair,sockopt::{socket_acceptconn,socket_passcred,socket_peercred,socket_type}},
    process::{getegid,geteuid,getpid,getuid}};
use std::{fs::File,io,os::fd::{AsFd,BorrowedFd},path::Path};

fn no()->io::Error {io::Error::other("original dedicated SMB launch could not be established")}

struct Image<'a>{ release:&'a release::Release, pub(super) file:&'a File, daemon:bool }
impl<'a> Image<'a>{
    fn select(release:&'a release::Release,daemon:bool)->io::Result<Self>{
        let file=release.executable(daemon)?;
        Ok(Self{release,file,daemon})
    }
    fn original(&self)->io::Result<BorrowedFd<'_>>{
        if !same(self.release.executable(self.daemon)?,self.file)?{return Err(no());}
        Ok(self.file.as_fd())
    }
}

/// No worker or browser can mint Online. It owns both original host locks,
/// the exact approved public records and verified original ordinary volumes.
pub(crate) fn foreground(approved:&Online)->io::Result<()>{
    process::host()?;
    let release=release::Release::select()?;
    approved.approve(release.manifest_sha())?;
    signals::watch_smb_cancellation()?;
    approved.require()?;release.require()?;
    if signals::cancelled(){return Err(no());}
    let (pin,system,site,private,state)=approved.host()?;
    let account=approved.account_parent()?;
    let [live,bound]=approved.public_originals();
    let physical=[origins::ident(site)?,origins::ident(private)?,origins::ident(state)?,origins::ident(&account)?];
    for f in std::iter::once(system).chain(approved.roots().map(|(_,f)|f)){
        if physical.contains(&origins::ident(f)?){return Err(no());}
    }
    let null=smb_views::null_device()?;
    fn src<'a>(p:&'static str,f:&'a File,access:Access)->Source<'a>{Source{path:Path::new(p),file:f,access}}
    let mut daemon=release.sources(true);
    for (p,f,access) in [
            ("instance/private",private,Access::Data),("instance/state",state,Access::Data),
            ("instance/accountdb",&account,Access::Public),("instance/smb.conf",live,Access::Public),
            ("instance/domain.bound",bound,Access::Public),("dev/null",&null,Access::Null)
    ]{
        daemon.push(src(p,f,access));
    }
    let mut storage=release.sources(false);
    fn relative(p:&Path)->io::Result<&Path>{p.strip_prefix("/").map_err(|_|no())}
    storage.push(Source{path:relative(pin.path())?,file:system,access:Access::Data});
    for (p,f) in approved.roots(){storage.push(Source{path:relative(p)?,file:f,access:Access::Data});}
    // The assembler leaves the supervising process in its actual host
    // namespace. All attach targets and temporary allocations are private.
    let (daemon_root,storage_root)=smb_views::prepare(&daemon,&storage,&release.public())?;
    let instance=origins::file_at(&daemon_root,"instance",OFlags::PATH|OFlags::DIRECTORY|OFlags::CLOEXEC|OFlags::NOFOLLOW)?;
    let bound_readonly=origins::projected(site,&instance,"domain.bound",128)?;
    let o=approved.plan().origin;
    let (uid,gid)=approved.subject();
    let pair=supervisor::Pair::start(origins::Inputs{approved,release:&release,daemon_root:&daemon_root,
        storage_root:&storage_root,site,system,system_pin:pin,bound_readonly:&bound_readonly,
        system_path:pin.path().to_str().ok_or_else(no)?,uid,gid,pool:o.pool,instance:o.instance,
        operation:o.domain,bound_sha:approved.bound_sha(),live_sha:approved.live_sha(),plan:approved.plan(),
        null:&null,approved_nss_sha:release.serial()})?;
    if let Err(e)=approved.ready(){let _=pair.stop();return Err(e);}
    pair.supervise()?;
    approved.stopped();
    Ok(())
}

pub(crate) struct Control { daemon:File, storage:File }
impl Control{
    /// Fresh kernel peers with no public address, not a listener or a connection
    /// supplied by a socket path/client. No protocol data is consumed here.
    pub(crate) fn fresh()->io::Result<Self>{
        if getuid().as_raw()!=0||geteuid().as_raw()!=0{return Err(no());}
        let (l,r)=socketpair(AddressFamily::UNIX,SocketType::SEQPACKET,SocketFlags::CLOEXEC,None)?;
        let v=Self{daemon:File::from(l),storage:File::from(r)};v.untouched()?;Ok(v)
    }
    fn untouched(&self)->io::Result<()>{
        for fd in [&self.daemon,&self.storage]{
            let who=socket_peercred(fd)?;
            if who.pid!=getpid()||who.uid.as_raw()!=0||who.gid!=getegid()||
                fstatfs(fd)?.f_type as u64!=0x534f434b||socket_type(fd)?!=SocketType::SEQPACKET
                ||socket_acceptconn(fd)?||socket_passcred(fd)?||fcntl_getfl(fd)?.contains(OFlags::NONBLOCK){
                return Err(no());
            }
            initially_no_sender_pidfd(fd)?;
        }
        if Entity::of(&self.daemon.metadata()?)?==Entity::of(&self.storage.metadata()?)?{return Err(no());}
        Ok(())
    }
    /// Sole dispatch of the first endpoint to the selected foreground ELF is
    /// the launcher's job. Do not pass the storage side or other inherited fds.
    pub(crate) fn first_endpoint(&self)->io::Result<BorrowedFd<'_>>{
        self.untouched()?;Ok(self.daemon.as_fd())
    }
    /// Provenance at creation is a precondition, not reconstructed from waitid
    /// or PPid: a subreaper can also wait for a child it adopted. No caller or
    /// enrollment interface for arbitrary pidfds exists. The process ref stays with its
    /// supervisor, so failure cannot consume its ability to stop/reap the child.
    fn for_storage<'a>(self,birth:&'a File,image:&Image<'_>)->io::Result<StorageSide<'a>>{
        self.untouched()?;image.original()?;
        process::qualify(birth,image.file,&self.daemon,&self.storage)?;
        self.untouched()?;image.original()?;process::waitable_live(birth)?;
        // No extra daemon-side reference is passed to the storage child.
        drop(self.daemon);
        Ok(StorageSide{endpoint:self.storage,master:birth})
    }
}
/// The storage program additionally needs its own independently verified BOUND
/// fd4, restricted identity/namespace and approved launch profile. No raw endpoint
/// or pidfd here is an account, pool write permission, readiness or drain proof.
pub(crate) struct StorageSide<'a>{endpoint:File,master:&'a File}
impl StorageSide<'_>{
    pub(crate) fn original_3_5(&self)->io::Result<(BorrowedFd<'_>,BorrowedFd<'_>)>{
        process::waitable_live(self.master)?;Ok((self.endpoint.as_fd(),self.master.as_fd()))
    }
}
fn same(a:&File,b:&File)->io::Result<bool>{
    let sa=Entity::of(&a.metadata()?)?;let sb=Entity::of(&b.metadata()?)?;
    let ma=Mount::on(a)?.ok_or_else(no)?;let mb=Mount::on(b)?.ok_or_else(no)?;
    Ok(sa==sb&&ma.id==mb.id)
}
