//! Trusted, pre-negotiation handoff only. The installation/session issuer and
//! the service listener do not exist here. A TCP peer's address is metadata,
//! not an identity; a completely written NEGOTIATE is not authentication.
pub(in crate::smb) mod incoming;
use super::{Connection,Fault,Opening,OpeningSend,Origin,Result,Window,ones,
    sessions::{AcceptSeat,RunningServerGuid},transport::{DirectStream,deadline}};
use crate::smb::wire::{direct::Frame,message::Compound,negotiate::{CmacChoice,MAX_OPENING_BYTES}};
use crate::system::reader::smb::runtime::QualifiedRustInstall;
use std::{net::{IpAddr,SocketAddr,TcpStream},sync::{Arc,atomic::AtomicBool}};

/// Exclusive protocol owner. No raw Connection getter, stream replacement,
/// separate response sink or numeric-ID constructor.
#[must_use]
pub(in crate::smb) struct Peer {
    connection:Connection, first_login_by:Option<std::time::Instant>, original_move:Option<incoming::MoveFlight>,
    move_backlog:Option<incoming::MoveControl>,
    ordinary_moves:bool,
}

/// A pair observed on the SAME owned kernel socket before reading any SMB
/// bytes. Neither address may be substituted or moved to a different
/// stream. This is evidence for network admission, not admission itself.
pub(in crate::smb) struct AcceptedEndpoints { local:SocketAddr, peer:SocketAddr }
impl AcceptedEndpoints {
    fn inspect(socket:&TcpStream)->Result<Self> {
        let local=socket.local_addr().map_err(|_|Fault::Transport)?;
        let peer=socket.peer_addr().map_err(|_|Fault::Transport)?;
        if !direct_pair(local,peer) {return Err(Fault::Transport);}
        Ok(Self{local,peer})
    }
    pub(in crate::smb) fn local(&self)->SocketAddr {self.local}
    pub(in crate::smb) fn peer(&self)->SocketAddr {self.peer}
}

fn direct_pair(local:SocketAddr,peer:SocketAddr)->bool {
    local.port()==445 && peer.port()!=0 &&
        local.is_ipv4()==peer.is_ipv4() &&
        concrete_address(local) && concrete_address(peer)
}
fn concrete_address(addr:SocketAddr)->bool {
    let ip=match addr.ip() {
        // Classification only. Retain the kernel's original sockaddr,
        // including its address family and v6 scope, for authorization.
        IpAddr::V6(a)=>a.to_ipv4_mapped().map_or(IpAddr::V6(a),IpAddr::V4),
        v4=>v4,
    };
    !ip.is_unspecified() && !ip.is_multicast() &&
        !matches!(ip,IpAddr::V4(a) if a.is_broadcast())
}

// Reviewed source stays off until the separately qualified Linux release.
const ORDINARY_MOVE_RELEASED:bool=false;

struct Incoming {
    stream:DirectStream,
    endpoints:Option<AcceptedEndpoints>,
}
impl Incoming {
    fn inspect(socket:TcpStream)->Result<Self> {
        let endpoints=AcceptedEndpoints::inspect(&socket)?;
        socket.set_nonblocking(false).map_err(|_|Fault::Transport)?;
        Ok(Self{stream:DirectStream::tcp(socket),endpoints:Some(endpoints)})
    }
}

impl RunningServerGuid {
    /// The TCP/445 service supplies its sole accepted, unread
    /// socket. No socket is obtained, accepted or connected by this module.
    pub(in crate::smb) fn negotiate_accepted(&self,accepted:crate::smb::launch::OwnedAccepted,
        installation:&QualifiedRustInstall)->Result<Peer> {
        let (socket,start,index)=accepted.unpack();
        if !self.is_original_start(&start)||!installation.is_original_start(&start){return Err(Fault::NoVerifier);}
        installation.require_current().map_err(|_|Fault::NoVerifier)?;
        let input=Incoming::inspect(socket)?;
        let pair=input.endpoints.as_ref().ok_or(Fault::Transport)?;
        if !installation.original_listener_is(index,pair.local())||
            !installation.admits_original_addresses(pair.local(),pair.peer()){return Err(Fault::Transport);}
        // No bytes have been read or written. This exact OS accepted stream
        // now enters the same origin's normal NEGOTIATE.
        let mut peer=Peer::open(self,input)?;
        peer.ordinary_moves=ORDINARY_MOVE_RELEASED;
        Ok(peer)
    }
}

fn parsed(record:&[u8])->Result<Compound<'_>> {
    let Some((frame,rest))=Frame::split(record).map_err(|_|Fault::FrameSize)? else {return Err(Fault::FrameSize);};
    if !rest.is_empty() {return Err(Fault::FrameSize);}
    Compound::parse(frame).map_err(|_|Fault::Opening)
}
impl Peer {
    fn open(server:&RunningServerGuid,mut incoming:Incoming)->Result<Self> {
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
        Ok(Self{connection,first_login_by:Some(first_login_by),original_move:None,move_backlog:None,
            ordinary_moves:false})
    }
    #[cfg(test)]
    fn open_fixture(server:&RunningServerGuid,stream:DirectStream)->Result<Self> {
        if !matches!(&stream,DirectStream::Fixture(_)) {return Err(Fault::Transport);}
        Self::open(server,Incoming{stream,endpoints:None})
    }
}
enum Completion { Written(Incoming), #[cfg(test)] WithoutSocket }
impl OpeningSend {
    // Private to this owner: there is no ordinary caller acknowledgement or
    // handoff that could swap in another connection after negotiation.
    fn after_write(self,completed:Completion,seat:AcceptSeat)->Result<Connection> {
        let (stream,endpoints)=match completed {
            Completion::Written(Incoming{stream,endpoints})=>(Some(stream),endpoints),
            #[cfg(test)] Completion::WithoutSocket=>(None,None),
        };
        if self.signing==CmacChoice::NoCommon {return Err(Fault::NoCommonSigning);}
        let conn=Connection{limits:self.limits,client:self.client,server:self.server,in_flight:0,
            sessions:Vec::new(),stream,origin:Arc::new(Origin{closed:AtomicBool::new(false),hash:self.hash,run:self.run,endpoints}),
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
