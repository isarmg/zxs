//! The same ZXS startup can consume one original combined approval for its
//! two fixed components. No user-visible separate launcher or selectable
//! role backend. Only the application retains this private scope and both
//! original children during life and drain.
use super::{Fault,Parent,VerifiedPreflight,birth::{Ready,Role},foreground,kernel::{cloexec,get_flags,sock,sock_int},
    listeners::Listeners,preflight::nonce,view::{self,Kind,Source,View}};
use crate::smb::launch::{
    envelope::{self,ENV_MAX},site::{SiteLease,Terminal,media::Media},transport::seal_public,web,
};
use std::{io,net::{Ipv4Addr,SocketAddr,TcpListener},os::fd::{AsFd,AsRawFd,BorrowedFd},time::{Duration,Instant}};

fn no()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"the original ZXS start or drain could not be confirmed")}

#[must_use]
pub(in crate::smb::launch) struct Running<'m,'v,'s,'p>{
    // Dropped FIRST. The original physical media and S/site stay retained
    // through real reap, including abnormal unresolved signal or wait.
    original:Ready<'p>,
    _media:&'m Media<'v,'s,'p>,
    _site:&'s SiteLease<'p>,
}
impl Running<'_,'_,'_,'_>{
    pub(in crate::smb::launch) fn exited(&self)->io::Result<Option<i32>>{self.original.peek().map_err(|_|no())}
    pub(in crate::smb::launch) fn begin_drain(&mut self)->io::Result<()>{self.original.begin_drain().map_err(|_|no())}
    pub(in crate::smb::launch) fn begin_pair(&mut self,web:&mut Self){
        start_both_stops(||self.original.begin_drain(),||web.original.begin_drain());
    }
    pub(in crate::smb::launch) fn finish(self)->io::Result<()>{self.original.finish().map_err(|_|no())}
}
pub(in crate::smb::launch) fn stopping()->bool{foreground::stopping()}

// The two actual pidfd owners are still retained. Never enter a wait on
// one child if the other stop has neither started nor seen exact exit.
fn start_both_stops(mut smb:impl FnMut()->super::Result<()>,mut web:impl FnMut()->super::Result<()>){
    let(mut smb_stopped,mut web_stopped)=(false,false);
    while !smb_stopped||!web_stopped{
        if !smb_stopped{smb_stopped=smb().is_ok();}
        if !web_stopped{web_stopped=web().is_ok();}
        if !smb_stopped||!web_stopped{std::thread::sleep(Duration::from_millis(20));}
    }
}

pub(in crate::smb::launch) fn prepare_site<'s,'p>(parent:&'p Parent,site:&'s SiteLease<'p>,terminal:&Terminal)
    ->io::Result<VerifiedPreflight<'s,'p>>{
    if !site.is_original_parent(parent){return Err(no());}
    // A direct-HTTP-only private product candidate must not silently
    // substitute direct HTTP for the existing configured HTTPS audience.
    if ["ZXS_WEB_TRUSTED_PROXY","ZXS_WEB_PUBLIC_ORIGIN"].into_iter().any(|key|std::env::var_os(key).is_some()){
        return Err(io::Error::new(io::ErrorKind::Unsupported,"configured Web HTTPS proxy is not admitted by this direct-only candidate"));
    }
    terminal.require()?;site.require()?;
    foreground::install().map_err(|_|no())?;
    VerifiedPreflight::execute(parent,site)
}

/// The locally confirmed original scope is single-use in exactly each
/// fixed component. A failed start does not silently re-use approval.
#[must_use]
pub(in crate::smb::launch) struct Authorized<'t,'m,'v,'s,'p>{
    parent:&'p Parent,site:&'s SiteLease<'p>,terminal:&'t Terminal,
    selected:&'v VerifiedPreflight<'s,'p>,media:&'m Media<'v,'s,'p>,smb:bool,web:bool,
}
pub(in crate::smb::launch) fn authorize<'t,'m,'v,'s,'p>(
    parent:&'p Parent,site:&'s SiteLease<'p>,terminal:&'t Terminal,
    selected:&'v VerifiedPreflight<'s,'p>,media:&'m Media<'v,'s,'p>,
)->io::Result<Authorized<'t,'m,'v,'s,'p>>{
    if !site.is_original_parent(parent){return Err(no());}
    let scope=Authorized{parent,site,terminal,selected,media,smb:false,web:false};
    scope.live()?;
    terminal.approve(site,selected,media,||scope.live())?.consume_for(site,selected,media)?;
    scope.live()?;
    Ok(scope)
}
impl Authorized<'_,'_,'_,'_,'_>{
    fn live(&self)->io::Result<()>{
        foreground::require().map_err(|_|no())?;
        self.terminal.require()?;
        self.media.require_for(self.site,self.selected)?;
        foreground::require().map_err(|_|no())
    }
}

pub(in crate::smb::launch) fn launch_internal_smb<'m,'v,'s,'p>(scope:&mut Authorized<'_,'m,'v,'s,'p>)
    ->io::Result<Running<'m,'v,'s,'p>>{
    if scope.smb{return Err(no());}
    scope.smb=true;
    scope.live()?;
    let (site,selected,media,parent)=(scope.site,scope.selected,scope.media,scope.parent);
    let (path,system,location)=site.system()?;
    let mut sources=vec![Source{path,original:system,kind:Kind::System}];
    sources.extend(media.ordinary_roots().map(|(path,original)|Source{path,original,kind:Kind::Ordinary}));
    if let Some((path,original))=media.recycle_root(){sources.push(Source{path,original,kind:Kind::Recycle});}
    let view=View::prepare(parent,sources,Role::Server).map_err(|_|no())?;
    let vault=site.vault(selected)?; // no host-root verifier read
    scope.live()?;view.require_prepared(parent).map_err(|_|no())?;
    let endpoints=selected.plan().endpoints();
    let listeners=Listeners::bind(endpoints,||scope.live().map_err(|_|Fault::Handoff)).map_err(|_|no())?;
    let cookies=listeners.cookies();
    let (vault_fd,witness)=vault.offer()?;
    let who=site.subject();let subject=(who.uid,who.gid);
    let epoch=&selected.qualified_for(site)?.epoch;
    let (header,public)=envelope::send::encode(envelope::send::Scope{
        nonce:nonce().map_err(|_|no())?,release:site.image_sha(),subject,system:location.entity,
        path:path.to_str().ok_or_else(no)?,plan:site.plan_bytes(),cookies:&cookies,vault:witness,epoch,
    }).map_err(|_|no())?;
    let sealed=seal_public(&public,ENV_MAX).map_err(|_|no())?;
    let mut fds:Vec<BorrowedFd<'_>>=vec![vault_fd];
    fds.extend(listeners.rights());
    let before=||->io::Result<()>{
        scope.live()?;vault.require()?;
        view.require_sources().map_err(|_|no())?;
        listeners.require(endpoints).map_err(|_|no())?;
        foreground::require().map_err(|_|no())
    };
    before()?;
    let born=parent.prepare().and_then(|p|p.birth(site.image().map_err(|_|Fault::Handoff)?,
        &view,subject,Role::Server)).map_err(|_|no())?;
    let ready=born.server(&header,&sealed,&fds,Instant::now()+Duration::from_secs(20),
        ||before().map_err(|_|Fault::Handoff)).map_err(|_|no())?;
    if let Err(e)=before(){
        let _=ready.stop(); // no false READY/confirmed business reply from the product
        return Err(e);
    }
    Ok(Running{original:ready,_media:media,_site:site})
}

pub(in crate::smb::launch) fn launch_internal_web<'m,'v,'s,'p>(scope:&mut Authorized<'_,'m,'v,'s,'p>,
    sibling:&mut Running<'m,'v,'s,'p>)->io::Result<Running<'m,'v,'s,'p>>{
    if scope.web||!scope.smb||!sibling.original.is_smb()||
        !std::ptr::eq(sibling._site,scope.site)||!std::ptr::eq(sibling._media,scope.media){return Err(no());}
    scope.web=true;
    scope.live()?;
    let (site,selected,media,parent)=(scope.site,scope.selected,scope.media,scope.parent);
    let (path,system,_)=site.system()?;
    let mut sources=vec![Source{path,original:system,kind:Kind::System}];
    sources.extend(media.ordinary_roots().map(|(path,original)|Source{path,original,kind:Kind::Ordinary}));
    let view=View::prepare(parent,sources,Role::Web).map_err(|_|no())?;
    scope.live()?;
    // The existing Web product uses just this fixed direct IPv4 listener.
    // The entitled child verifies this same cookie/IP/port, not an env or
    // caller-supplied address. No proxy policy is introduced by this lane.
    let listener=TcpListener::bind((Ipv4Addr::UNSPECIFIED,8443))?;
    listener.set_nonblocking(true)?;
    let cookie=u64::from_ne_bytes(sock(listener.as_raw_fd(),libc::SOL_SOCKET,libc::SO_COOKIE).map_err(|_|no())?);
    let read_only=view::flags(system).map_err(|_|no())?&libc::ST_RDONLY!=0;
    let (header,body)=web::encode(selected.plan(),nonce().map_err(|_|no())?,cookie,read_only).map_err(|_|no())?;
    let sealed=seal_public(&body,web::MAX).map_err(|_|no())?;
    let who=site.subject();
    let before=||->io::Result<()>{
        scope.live()?;require_web(&listener,cookie).map_err(|_|no())?;
        view.require_sources().map_err(|_|no())?;
        if read_only!=(view::flags(system).map_err(|_|no())?&libc::ST_RDONLY!=0){return Err(no());}
        foreground::require().map_err(|_|no())
    };
    before()?;
    let born=parent.prepare().and_then(|p|p.birth(site.image().map_err(|_|Fault::Handoff)?,
        &view,(who.uid,who.gid),Role::Web)).map_err(|_|no())?;
    let mut ready=match born.web(&header,&sealed,listener.as_fd(),Instant::now()+Duration::from_secs(20),
        ||before().map_err(|_|Fault::Handoff)){
        Ok(ready)=>ready,
        Err((_cause,mut failed))=>{
            start_both_stops(||sibling.original.begin_drain(),||failed.begin_failed_launch());
            drop(failed); // only its real pidfd, never a confirmed Web drain
            return Err(no());
        }
    };
    if let Err(e)=before(){
        start_both_stops(||sibling.original.begin_drain(),||ready.begin_drain());
        let _=ready.finish();
        return Err(e);
    }
    Ok(Running{original:ready,_media:media,_site:site})
}
fn require_web(listener:&TcpListener,cookie:u64)->super::Result<()>{
    let raw=listener.as_raw_fd();
    cloexec(raw)?;
    if cookie==0||u64::from_ne_bytes(sock(raw,libc::SOL_SOCKET,libc::SO_COOKIE)?)!=cookie||
        get_flags(raw,libc::F_GETFL)?&libc::O_NONBLOCK==0||
        sock_int(raw,libc::SOL_SOCKET,libc::SO_DOMAIN)?!=libc::AF_INET||
        sock_int(raw,libc::SOL_SOCKET,libc::SO_TYPE)?!=libc::SOCK_STREAM||
        sock_int(raw,libc::SOL_SOCKET,libc::SO_PROTOCOL)?!=libc::IPPROTO_TCP||
        sock_int(raw,libc::SOL_SOCKET,libc::SO_ACCEPTCONN)?!=1||
        sock_int(raw,libc::SOL_SOCKET,libc::SO_REUSEPORT)?!=0||
        listener.local_addr().map_err(|_|Fault::Handoff)?!=SocketAddr::from((Ipv4Addr::UNSPECIFIED,8443)){
        return Err(Fault::Handoff);
    }
    Ok(())
}


#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn the_fixed_pair_votes_both_original_stops_before_any_wait_cut(){
        use std::cell::{Cell,RefCell};
        let calls=RefCell::new(Vec::new());let count=Cell::new(0);
        start_both_stops(||{
            calls.borrow_mut().push("smb");
            count.set(count.get()+1);
            if count.get()==1{Err(Fault::Handoff)}else{Ok(())}
        },||{calls.borrow_mut().push("web");Ok(())});
        calls.borrow_mut().push("reap");
        assert_eq!(calls.into_inner(),vec!["smb","web","smb","reap"]);
    }
}
