//! Approved exact host listeners only: no externally adopted sockets,
//! wildcard, reuseport, system network setting, routing or name discovery.
use super::{Fault,Result,kernel::{cloexec,get_flags,sock,sock_int}};
use std::{collections::BTreeSet,mem::size_of,net::{IpAddr,Ipv4Addr,Ipv6Addr,SocketAddr,TcpListener},
    os::fd::{AsFd,AsRawFd,FromRawFd,BorrowedFd,OwnedFd},ptr};

const PORT:u16=445;
const BACKLOG:i32=128;
struct Original{listener:TcpListener,endpoint:(IpAddr,u8),cookie:u64}
pub(super) struct Listeners(Vec<Original>);
impl Listeners{
    pub(super) fn bind(endpoints:&[(IpAddr,u8)],check:impl Fn()->Result<()>)->Result<Self>{
        check()?;require_local(endpoints)?;
        let originals=endpoints.iter().copied().map(Original::bind).collect::<Result<Vec<_>>>()?;
        check()?;
        for p in &originals{
            // SAFETY: only our original already bound TCP fd, same argument on all.
            if unsafe{libc::listen(p.listener.as_raw_fd(),BACKLOG)}!=0{return Err(Fault::Handoff);}
        }
        let this=Self(originals);
        this.require(endpoints)?;check()?;
        Ok(this)
    }
    pub(super) fn require(&self,endpoints:&[(IpAddr,u8)])->Result<()>{
        require_local(endpoints)?;
        if self.0.len()!=endpoints.len(){return Err(Fault::Handoff);}
        let mut seen=BTreeSet::new();
        for (o,&e) in self.0.iter().zip(endpoints){
            if o.endpoint!=e||!seen.insert(o.cookie){return Err(Fault::Handoff);}
            o.require(true)?;
        }
        Ok(())
    }
    pub(super) fn cookies(&self)->Vec<u64>{self.0.iter().map(|p|p.cookie).collect()}
    pub(super) fn rights(&self)->impl Iterator<Item=BorrowedFd<'_>>{self.0.iter().map(|p|p.listener.as_fd())}
}
impl Original{
    fn bind(endpoint:(IpAddr,u8))->Result<Self>{
        let (addr,prefix)=endpoint;
        if addr.is_unspecified()||addr.is_loopback()||addr.is_multicast()||prefix==0||
            matches!(addr,IpAddr::V4(v) if v.is_broadcast()||prefix>32)||
            matches!(addr,IpAddr::V6(v) if v.is_unicast_link_local()||v.to_ipv4().is_some()||prefix>128){
            return Err(Fault::Handoff);
        }
        let domain=if addr.is_ipv4(){libc::AF_INET}else{libc::AF_INET6};
        // SAFETY: creates only an original Linux TCP socket, CLOEXEC and NB.
        let n=unsafe{libc::socket(domain,libc::SOCK_STREAM|libc::SOCK_CLOEXEC|libc::SOCK_NONBLOCK,libc::IPPROTO_TCP)};
        if n<0{return Err(Fault::Handoff);}
        // SAFETY: the successful syscall yielded an owned descriptor.
        let fd=unsafe{OwnedFd::from_raw_fd(n)};
        let set=|layer,opt|->Result<()>{
            let value:libc::c_int=1;
            // SAFETY: own socket only and one fixed checked integer option.
            if unsafe{libc::setsockopt(n,layer,opt,(&raw const value).cast(),size_of::<libc::c_int>()as _)}!=0{
                return Err(Fault::Handoff);
            }
            Ok(())
        };
        set(libc::SOL_SOCKET,libc::SO_REUSEADDR)?;
        if addr.is_ipv6(){set(libc::IPPROTO_IPV6,libc::IPV6_V6ONLY)?;}
        let rc=match addr{
            IpAddr::V4(v)=>{
                let local=libc::sockaddr_in{sin_family:libc::AF_INET as _,sin_port:PORT.to_be(),
                    sin_addr:libc::in_addr{s_addr:u32::from_ne_bytes(v.octets())},sin_zero:[0;8]};
                // SAFETY: exact approved address and typed Linux sockaddr.
                unsafe{libc::bind(n,(&raw const local).cast(),size_of::<libc::sockaddr_in>()as _)}
            }
            IpAddr::V6(v)=>{
                let local=libc::sockaddr_in6{sin6_family:libc::AF_INET6 as _,sin6_port:PORT.to_be(),
                    sin6_flowinfo:0,sin6_addr:libc::in6_addr{s6_addr:v.octets()},sin6_scope_id:0};
                // SAFETY: exact approved address and typed Linux sockaddr.
                unsafe{libc::bind(n,(&raw const local).cast(),size_of::<libc::sockaddr_in6>()as _)}
            }
        };
        if rc!=0{return Err(Fault::Handoff);}
        let cookie=u64::from_ne_bytes(sock(n,libc::SOL_SOCKET,libc::SO_COOKIE)?);
        let original=Self{listener:TcpListener::from(fd),endpoint,cookie};
        original.require(false)?;Ok(original)
    }
    fn require(&self,listening:bool)->Result<()>{
        let fd=self.listener.as_raw_fd();let ip=self.endpoint.0;
        let family=if ip.is_ipv4(){libc::AF_INET}else{libc::AF_INET6};
        cloexec(fd)?;
        if get_flags(fd,libc::F_GETFL)?&libc::O_NONBLOCK==0||self.cookie==0||
            u64::from_ne_bytes(sock(fd,libc::SOL_SOCKET,libc::SO_COOKIE)?)!=self.cookie||
            sock_int(fd,libc::SOL_SOCKET,libc::SO_DOMAIN)?!=family||sock_int(fd,libc::SOL_SOCKET,libc::SO_TYPE)?!=libc::SOCK_STREAM||
            sock_int(fd,libc::SOL_SOCKET,libc::SO_PROTOCOL)?!=libc::IPPROTO_TCP||
            sock_int(fd,libc::SOL_SOCKET,libc::SO_REUSEPORT)?!=0||
            sock_int(fd,libc::SOL_SOCKET,libc::SO_ACCEPTCONN)?!=listening as i32||
            ip.is_ipv6()&&sock_int(fd,libc::IPPROTO_IPV6,libc::IPV6_V6ONLY)?!=1||
            self.listener.local_addr().map_err(|_|Fault::Handoff)?!=SocketAddr::new(ip,PORT){return Err(Fault::Handoff);}
        // SAFETY: bounded kernel query on the original socket; no peer path.
        let mut address:libc::sockaddr_storage=unsafe{std::mem::zeroed()};
        let mut n=size_of::<libc::sockaddr_storage>()as libc::socklen_t;
        if unsafe{libc::getpeername(fd,(&raw mut address).cast(),&raw mut n)}!= -1||
            std::io::Error::last_os_error().raw_os_error()!=Some(libc::ENOTCONN){return Err(Fault::Handoff);}
        Ok(())
    }
}
struct Interfaces(*mut libc::ifaddrs);
impl Drop for Interfaces{
    fn drop(&mut self){
        // SAFETY: getifaddrs gave this one owned list.
        unsafe{libc::freeifaddrs(self.0)}
    }
}
/// Require one current assignment to an up, non-loopback interface.
/// A socket bind alone can succeed with IP_NONLOCAL_BIND.
fn require_local(approved:&[(IpAddr,u8)])->Result<()>{
    if !(1..=16).contains(&approved.len()){return Err(Fault::Shape);}
    let chosen=approved.iter().map(|p|p.0).collect::<BTreeSet<_>>();
    if chosen.len()!=approved.len(){return Err(Fault::Shape);}
    let mut raw: *mut libc::ifaddrs=ptr::null_mut();
    // SAFETY: a writable pointer to the list head; libc owns it until success.
    if unsafe{libc::getifaddrs(&raw mut raw)}!=0{return Err(Fault::Handoff);}
    let addresses=Interfaces(raw);let mut p=addresses.0;let mut found=BTreeSet::new();let mut n=0u32;
    while !p.is_null(){
        n+=1;if n>65_536{return Err(Fault::Resources);}
        // SAFETY: list and each typed socket address are owned by getifaddrs.
        let row=unsafe{&*p};p=row.ifa_next;
        if row.ifa_addr.is_null()||row.ifa_flags&libc::IFF_UP as u32==0||row.ifa_flags&libc::IFF_LOOPBACK as u32!=0{continue;}
        let ip=unsafe{match i32::from((*row.ifa_addr).sa_family){
            libc::AF_INET=>IpAddr::V4(Ipv4Addr::from((*row.ifa_addr.cast::<libc::sockaddr_in>()).sin_addr.s_addr.to_ne_bytes())),
            libc::AF_INET6=>{
                let v=&*row.ifa_addr.cast::<libc::sockaddr_in6>();
                if v.sin6_scope_id!=0{continue;}
                IpAddr::V6(Ipv6Addr::from(v.sin6_addr.s6_addr))
            }
            _=>continue,
        }};
        if chosen.contains(&ip)&&!found.insert(ip){return Err(Fault::Handoff);}
    }
    if chosen!=found{return Err(Fault::Handoff);}
    Ok(())
}
