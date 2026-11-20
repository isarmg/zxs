//! Trusted, pre-negotiation handoff only. The installation/session issuer and
//! the service listener do not exist here. A TCP peer's address is metadata,
//! not an identity; a completely written NEGOTIATE is not authentication.
pub(in crate::smb) mod incoming;
use super::{Connection,Fault,Opening,OpeningSend,Origin,Result,Window,ones,
    sessions::{AcceptSeat,RunningServerGuid},transport::{DirectStream,deadline}};
use crate::smb::wire::{direct::Frame,message::Compound,negotiate::{CmacChoice,MAX_OPENING_BYTES}};
use std::{net::{IpAddr,TcpStream},sync::{Arc,atomic::AtomicBool}};

/// Exclusive protocol owner. No raw Connection getter, stream replacement,
/// separate response sink or numeric-ID constructor.
#[must_use]
pub(in crate::smb) struct Peer { connection:Connection, remote:IpAddr, first_login_by:Option<std::time::Instant> }

impl RunningServerGuid {
    /// Future enabled TCP/445 service supplies its sole accepted, unread
    /// socket. No socket is obtained, accepted or connected by this module.
    pub(in crate::smb) fn negotiate_accepted(&self,socket:TcpStream)->Result<Peer> {
        let wire=DirectStream::Tcp(socket);
        let remote=match &wire {
            DirectStream::Tcp(s)=>{
                if s.local_addr().map_err(|_|Fault::Transport)?.port()!=445 {return Err(Fault::Transport);}
                s.set_nonblocking(false).map_err(|_|Fault::Transport)?;
                s.peer_addr().map_err(|_|Fault::Transport)?.ip()
            }
            #[cfg(test)]
            DirectStream::Fixture(_)=>unreachable!(),
        };
        Peer::open(self,wire,remote)
    }
}
fn parsed(record:&[u8])->Result<Compound<'_>> {
    let Some((frame,rest))=Frame::split(record).map_err(|_|Fault::FrameSize)? else {return Err(Fault::FrameSize);};
    if !rest.is_empty() {return Err(Fault::FrameSize);}
    Compound::parse(frame).map_err(|_|Fault::Opening)
}
impl Peer {
    fn open(server:&RunningServerGuid,mut stream:DirectStream,remote:IpAddr)->Result<Self> {
        let seat=AcceptSeat::reserve(&server.share())?;
        let original=stream.read_record(MAX_OPENING_BYTES,deadline()?)?;
        let offer=Opening::received(&parsed(&original)?)?;
        let tx=offer.respond(server)?;
        // The immutable SMB bytes are exactly the ones bound to the negotiated
        // hash and grants, written completely through their own incoming stream.
        // A partial prefix/body, EOF or uncertain submission returns no Peer.
        stream.write_smb(tx.bytes(),||Ok(()))?;
        let connection=tx.after_write(Completion::Written(stream),seat)?;
        let first_login_by=std::time::Instant::now().checked_add(std::time::Duration::from_secs(60)).ok_or(Fault::Clock)?;
        Ok(Self{connection,remote,first_login_by:Some(first_login_by)})
    }
}
enum Completion { Written(DirectStream), #[cfg(test)] WithoutSocket }
impl OpeningSend {
    // Private to this owner: there is no ordinary caller acknowledgement or
    // handoff that could swap in another connection after negotiation.
    fn after_write(self,completed:Completion,seat:AcceptSeat)->Result<Connection> {
        let stream=match completed {Completion::Written(s)=>Some(s),#[cfg(test)] Completion::WithoutSocket=>None};
        if self.signing==CmacChoice::NoCommon {return Err(Fault::NoCommonSigning);}
        let conn=Connection{limits:self.limits,client:self.client,server:self.server,in_flight:0,
            sessions:Vec::new(),stream,origin:Arc::new(Origin{closed:AtomicBool::new(false),hash:self.hash,run:self.run}),
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
