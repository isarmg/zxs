//! Trusted, pre-negotiation handoff only. The installation/session issuer and
//! the service listener do not exist here. A TCP peer's address is metadata,
//! not an identity; a completely written NEGOTIATE is not authentication.
pub(in crate::smb) mod incoming;
use super::{Connection,Fault,Opening,OpeningSend,Origin,Result,Window,ones,
    sessions::{AcceptSeat,RunningServerGuid},transport::{DirectStream,deadline}};
use crate::smb::wire::{direct::Frame,message::Compound,negotiate::{CmacChoice,MAX_OPENING_BYTES}};
use std::{net::{IpAddr,SocketAddr,TcpStream},sync::{Arc,atomic::AtomicBool}};

/// Exclusive protocol owner. No raw Connection getter, stream replacement,
/// separate response sink or numeric-ID constructor.
#[must_use]
pub(in crate::smb) struct Peer { connection:Connection, remote:IpAddr, first_login_by:Option<std::time::Instant> }

/// Kernel-observed local endpoint of this exact accepted transport, not an
/// approved listener or a principal. There is no constructor from names,
/// peer input or an externally supplied sockaddr.
pub(super) struct AcceptedLocal(SocketAddr);
impl AcceptedLocal {
    fn inspect(socket:&TcpStream)->Result<Self> {
        let addr=socket.local_addr().map_err(|_|Fault::Transport)?;
        if !concrete_445(addr) {return Err(Fault::Transport);}
        Ok(Self(addr))
    }
    pub(super) fn address(&self)->SocketAddr {self.0}
}

fn concrete_445(addr:SocketAddr)->bool {
    let ip=match addr.ip() {
        // Classification only. Retain the kernel's original sockaddr,
        // including its address family and v6 scope, for authorization.
        IpAddr::V6(a)=>a.to_ipv4_mapped().map_or(IpAddr::V6(a),IpAddr::V4),
        v4=>v4,
    };
    addr.port()==445 && !ip.is_unspecified() && !ip.is_multicast() &&
        !matches!(ip,IpAddr::V4(a) if a.is_broadcast())
}

struct Incoming {
    stream:DirectStream,
    local:Option<AcceptedLocal>,
}
impl Incoming {
    fn inspect(stream:DirectStream)->Result<Self> {
        let local=match &stream {
            DirectStream::Tcp(socket)=>{
                let local=AcceptedLocal::inspect(socket)?;
                socket.set_nonblocking(false).map_err(|_|Fault::Transport)?;
                Some(local)
            }
            #[cfg(test)]
            DirectStream::Fixture(_)=>None,
        };
        Ok(Self{stream,local})
    }
}

impl RunningServerGuid {
    /// Future enabled TCP/445 service supplies its sole accepted, unread
    /// socket. No socket is obtained, accepted or connected by this module.
    pub(in crate::smb) fn negotiate_accepted(&self,socket:TcpStream)->Result<Peer> {
        let remote=socket.peer_addr().map_err(|_|Fault::Transport)?.ip();
        Peer::open(self,DirectStream::Tcp(socket),remote)
    }
}
fn parsed(record:&[u8])->Result<Compound<'_>> {
    let Some((frame,rest))=Frame::split(record).map_err(|_|Fault::FrameSize)? else {return Err(Fault::FrameSize);};
    if !rest.is_empty() {return Err(Fault::FrameSize);}
    Compound::parse(frame).map_err(|_|Fault::Opening)
}
impl Peer {
    fn open(server:&RunningServerGuid,stream:DirectStream,remote:IpAddr)->Result<Self> {
        let mut incoming=Incoming::inspect(stream)?;
        let seat=AcceptSeat::reserve(&server.share())?;
        let original=incoming.stream.read_record(MAX_OPENING_BYTES,deadline()?)?;
        let offer=Opening::received(&parsed(&original)?)?;
        let tx=offer.respond(server)?;
        // The immutable SMB bytes are exactly the ones bound to the negotiated
        // hash and grants, written completely through their own incoming stream.
        // A partial prefix/body, EOF or uncertain submission returns no Peer.
        incoming.stream.write_smb(tx.bytes(),||Ok(()))?;
        let connection=tx.after_write(Completion::Written(incoming),seat)?;
        let first_login_by=std::time::Instant::now().checked_add(std::time::Duration::from_secs(60)).ok_or(Fault::Clock)?;
        Ok(Self{connection,remote,first_login_by:Some(first_login_by)})
    }
}
enum Completion { Written(Incoming), #[cfg(test)] WithoutSocket }
impl OpeningSend {
    // Private to this owner: there is no ordinary caller acknowledgement or
    // handoff that could swap in another connection after negotiation.
    fn after_write(self,completed:Completion,seat:AcceptSeat)->Result<Connection> {
        let (stream,accepted_local)=match completed {
            Completion::Written(Incoming{stream,local})=>(Some(stream),local),
            #[cfg(test)] Completion::WithoutSocket=>(None,None),
        };
        if self.signing==CmacChoice::NoCommon {return Err(Fault::NoCommonSigning);}
        let conn=Connection{limits:self.limits,client:self.client,server:self.server,in_flight:0,
            sessions:Vec::new(),stream,origin:Arc::new(Origin{closed:AtomicBool::new(false),hash:self.hash,run:self.run,accepted_local}),
            window:Window{low:1,next:1+u64::from(self.grant),bits:ones(self.grant)}};
        seat.promote(&conn)?;Ok(conn)
    }
    /// Existing pure wire/state fixtures never open sockets. No production
    /// method can manually acknowledge a NEGOTIATE without doing the I/O.
    #[cfg(test)]
    pub(in crate::smb) fn submitted(self)->Result<Connection> {
        let seat=AcceptSeat::reserve(&self.run)?;self.after_write(Completion::WithoutSocket,seat)
    }
}

#[cfg(test)]
#[path = "../../test/smb/accepted.rs"]
mod tests;
