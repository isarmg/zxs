//! One fixed, read-only NETLINK_AUDIT request for this process's TTY policy.
//! Never changes that policy, reads audit records, or accepts a socket/command.
use rustix::{net::{self, netlink::{self,SocketAddrNetlink}, sockopt::{set_socket_timeout,Timeout},
    AddressFamily,RecvFlags,SendFlags,SocketFlags,SocketType},thread::gettid};
use std::{io,marker::PhantomData,rc::Rc,time::{Duration,Instant}};

const TTY_GET:u16=1016; // Linux UAPI linux/audit.h: AUDIT_TTY_GET
const SEQ:u32=1; // A fresh, unshared socket carries exactly this one question.
fn refused()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,
    "the current session's TTY audit policy could not be verified as inactive; no credential accepted")}

fn packet(n:u32,flags:u16,port:u32)->[u8;16] {
    let mut b=[0u8;16];
    b[..4].copy_from_slice(&n.to_ne_bytes()); b[4..6].copy_from_slice(&TTY_GET.to_ne_bytes());
    b[6..8].copy_from_slice(&flags.to_ne_bytes()); b[8..12].copy_from_slice(&SEQ.to_ne_bytes());
    b[12..].copy_from_slice(&port.to_ne_bytes()); b
}
fn inactive(source:SocketAddrNetlink,bytes:&[u8])->io::Result<()> {
    // The kernel sender must be corroborated by the actual sockaddr, not just
    // an easily copied nlmsg_pid inside a message. A single, fixed-size reply,
    // not ACK, multipart, NLMSG_ERROR, partial data, or any future state.
    if source!=SocketAddrNetlink::new(0,0)||bytes.len()!=24||bytes[..16]!=packet(24,0,0)
        ||bytes[16..]!=[0;8]{return Err(refused());}
    Ok(())
}
pub(super) struct UnauditedInput {
    pid:u32,tid:i32,until:Instant,_thread:PhantomData<Rc<()>>,
}
impl UnauditedInput {
    /// Run before this same task permanently drops its capabilities. Linux
    /// requires the initial PID namespace and CAP_AUDIT_CONTROL even for GET.
    /// The authoritative flags belong to current->signal; no global auditd
    /// config, mount option or independent process is a substitute.
    pub(super) fn query_current()->io::Result<Self>{
        let fd=net::socket_with(AddressFamily::NETLINK,SocketType::RAW,SocketFlags::CLOEXEC,Some(netlink::AUDIT))?;
        let kernel=SocketAddrNetlink::new(0,0);
        net::bind(&fd,&kernel)?; // pid=0: kernel assigns this socket's local port
        let me=SocketAddrNetlink::try_from(net::getsockname(&fd)?)?;
        if me.pid()==0||me.groups()!=0{return Err(refused());}
        for which in [Timeout::Send,Timeout::Recv]{set_socket_timeout(&fd,which,Some(Duration::from_secs(2)))?;}
        let q=packet(16,1,me.pid()); // NLM_F_REQUEST; not ACK/SET/subscribe
        if net::sendto(&fd,&q,SendFlags::empty(),&kernel)?!=q.len(){return Err(refused());}
        let mut b=[0u8;24];
        let (_,n,addr)=net::recvfrom(&fd,&mut b[..],RecvFlags::TRUNC)?;
        if n!=b.len(){return Err(refused());}
        inactive(SocketAddrNetlink::try_from(addr.ok_or_else(refused)?)?,&b)?;
        // No audit socket or privilege travels to the input reader.
        Ok(Self{pid:std::process::id(),tid:gettid().as_raw_pid(),
            until:Instant::now()+Duration::from_secs(9*60),_thread:PhantomData})
    }
    pub(super) fn require_current(&self)->io::Result<()>{
        if self.pid!=std::process::id()||self.tid!=gettid().as_raw_pid()||Instant::now()>=self.until{
            return Err(refused());
        }Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn only_the_exact_kernel_response_with_both_local_tty_flags_clear_is_evidence() {
        let k=SocketAddrNetlink::new(0,0);let mut b=[0u8;24];b[..16].copy_from_slice(&packet(24,0,0));
        assert!(inactive(k,&b).is_ok());
        for i in [0,4,6,8,12,16,20]{let mut bad=b;bad[i]^=1;assert!(inactive(k,&bad).is_err());}
        assert!(inactive(SocketAddrNetlink::new(1,0),&b).is_err());
        assert!(inactive(SocketAddrNetlink::new(0,1),&b).is_err());
        assert!(inactive(k,&b[..23]).is_err());assert!(inactive(k,&[0;25]).is_err());
    }
}
