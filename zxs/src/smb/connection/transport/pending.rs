//! Incremental input only while the original peer owns a parked request.
//! Never use a second reader, a duplicated fd, or change the stream's mode.
use super::{DirectStream,Fault,Frame,Result,deadline,remaining};
use std::{io::{self,ErrorKind},net::TcpStream,os::fd::{AsRawFd,BorrowedFd},sync::{Arc,Weak},time::{Duration,Instant}};

const STEP_BYTES:usize=16*1024;
const WORK_TICK:Duration=Duration::from_millis(32);

#[derive(Default)]
pub(in crate::smb::connection) struct PendingRead{
    prefix:[u8;4],prefix_used:usize,body:Option<Body>,end:Option<Instant>,
}
struct Body{record:Vec<u8>,used:usize}
impl PendingRead{
    pub(in crate::smb::connection) fn has_partial(&self)->bool{self.end.is_some()}

    /// One bounded nonwaiting read, at most one Direct TCP record. Quiet or
    /// partial input is not EOF. The ordinary five-second framing budget
    /// starts at the first consumed byte; no idle timeout extends it.
    pub(in crate::smb::connection) fn advance(&mut self,stream:&mut DirectStream,limit:usize)
        ->Result<Option<Vec<u8>>>{
        stream.require_pending_peer()?;
        if let Some(end)=self.end{remaining(end)?;}
        if let Some(body)=&mut self.body{
            let end=body.record.len().min(body.used+STEP_BYTES);
            let Some(n)=stream.read_pending(&mut body.record[body.used..end])?else{return Ok(None);};
            body.used+=n;
            if body.used!=body.record.len(){return Ok(None);}
            remaining(self.end.ok_or(Fault::WrongBatch)?)?;
            let Body{record,..}=self.body.take().ok_or(Fault::WrongBatch)?;
            *self=Self::default();
            return Ok(Some(record));
        }
        let Some(n)=stream.read_pending(&mut self.prefix[self.prefix_used..])?else{return Ok(None);};
        if self.end.is_none(){self.end=Some(deadline()?);}
        self.prefix_used+=n;
        if self.prefix_used==4{
            let size=Frame::payload_len(self.prefix).map_err(|_|Fault::FrameSize)?;
            if size>limit{return Err(Fault::FrameSize);}
            let mut record=Vec::new();
            record.try_reserve_exact(4+size).map_err(|_|Fault::Resources)?;
            record.extend_from_slice(&self.prefix);
            record.resize(4+size,0);
            self.body=Some(Body{record,used:4});
        }
        Ok(None)
    }
    /// After settlement only: finish this exact partly consumed frame as
    /// ordinary synchronous input, without starting a new framing timer.
    pub(in crate::smb::connection) fn finish_pending(&mut self,stream:&mut DirectStream,limit:usize)
        ->Result<Option<Vec<u8>>>{
        let Some(end)=self.end else{return Ok(None);};
        remaining(end)?;
        let record=if let Some(mut body)=self.body.take(){
            stream.fill(&mut body.record[body.used..],end)?;
            body.record
        }else{
            stream.fill(&mut self.prefix[self.prefix_used..],end)?;
            stream.body(self.prefix,limit,end)?
        };
        *self=Self::default();
        Ok(Some(record))
    }
    /// One idle wait for this same original stream and its single worker.
    /// A wake signal is only a hint; the caller still checks the real
    /// result channel and authenticated input. No read or descriptor dup.
    pub(in crate::smb::connection) fn wait_for_worker_or_input(&self,stream:&DirectStream,completion:BorrowedFd<'_>)
        ->Result<()>{
        if crate::smb::launch::stopping(){return Err(Fault::Closed);}
        let fd=match stream{
            DirectStream::Tcp(s)=>s.0.as_raw_fd(),
            #[cfg(test)]
            DirectStream::Fixture(_)=>return Err(Fault::Transport),
        };
        let left=if let Some(end)=self.end{remaining(end)?.min(WORK_TICK)}else{WORK_TICK};
        let mut p=[
            libc::pollfd{fd,events:libc::POLLIN|libc::POLLRDHUP,revents:0},
            libc::pollfd{fd:completion.as_raw_fd(),events:libc::POLLIN,revents:0},
        ];
        // SAFETY: both borrowed descriptors outlive the fixed initialized
        // array; floor the absolute remaining duration to at most 32ms.
        let ready=unsafe{libc::poll(p.as_mut_ptr(),2,left.as_millis() as libc::c_int)};
        if crate::smb::launch::stopping(){return Err(Fault::Closed);}
        if ready<0{
            return if io::Error::last_os_error().kind()==ErrorKind::Interrupted{Ok(())}else{Err(Fault::Transport)};
        }
        pending_events(p[0].revents)?;
        if p[1].revents&(libc::POLLERR|libc::POLLHUP|libc::POLLNVAL)!=0{return Err(Fault::WriteOutcomeUnknown);}
        Ok(())
    }
}

/// One-way observation of the original owned TCP socket. This does not
/// expose the stream, its fd, recv/send, shutdown, or socket configuration.
/// There is no persistent strong reference on the worker and no fd dup.
#[must_use]
pub(in crate::smb::connection) struct OriginalPeerProbe(Weak<TcpStream>);
impl OriginalPeerProbe{
    pub(in crate::smb::connection) fn observe(&self)->Result<()>{
        let socket=self.0.upgrade().ok_or(Fault::Closed)?;
        observe_peer(&socket)
    }
}
fn observe_peer(socket:&TcpStream)->Result<()>{
    let mut p=libc::pollfd{fd:socket.as_raw_fd(),events:libc::POLLRDHUP,revents:0};
    // SAFETY: the socket retains ownership of this fd for the whole
    // syscall. One initialized pollfd, zero wait, and no retry.
    if unsafe{libc::poll(&mut p,1,0)}<0{return Err(Fault::Transport);}
    pending_events(p.revents)
}
fn pending_events(events:libc::c_short)->Result<()>{
    if events&(libc::POLLERR|libc::POLLNVAL)!=0{return Err(Fault::Transport);}
    if events&(libc::POLLRDHUP|libc::POLLHUP)!=0{return Err(Fault::Closed);}
    Ok(())
}
impl DirectStream{
    /// Only issue from the actual already owned TCP, never a test sink,
    /// saved descriptor number, remote address or another stream.
    pub(in crate::smb::connection) fn original_peer_probe(&self)->Result<OriginalPeerProbe>{
        match self{
            Self::Tcp(socket)=>Ok(OriginalPeerProbe(Arc::downgrade(&socket.0))),
            #[cfg(test)]
            Self::Fixture(_)=>Err(Fault::Transport),
        }
    }
    /// A local observation of this exact socket, not an ongoing liveness
    /// guarantee. Even FIN with queued bytes ends a parked turn; known loss
    /// cannot be converted into a cancellation reply or grant.
    pub(in crate::smb::connection) fn require_pending_peer(&self)->Result<()>{
        if crate::smb::launch::stopping(){return Err(Fault::Closed);}
        match self{
            Self::Tcp(socket)=>observe_peer(&socket.0),
            #[cfg(test)]
            Self::Fixture(_)=>Ok(()), // synthetic EOF is seen only by read_pending
        }
    }

    fn read_pending(&mut self,dst:&mut [u8])->Result<Option<usize>>{
        if dst.is_empty(){return Err(Fault::WrongBatch);}
        let got=match self{
            Self::Tcp(socket)=>{
                // SAFETY: dst is writable for the given finite size and
                // the original, exclusively owned socket outlives recv.
                let n=unsafe{libc::recv(socket.0.as_raw_fd(),dst.as_mut_ptr().cast(),dst.len(),libc::MSG_DONTWAIT)};
                if n<0{Err(io::Error::last_os_error())}else{Ok(n as usize)}
            },
            #[cfg(test)]
            Self::Fixture(s)=>std::io::Read::read(s,dst),
        };
        match got{
            Ok(0)=>Err(Fault::Closed),
            Ok(n) if n<=dst.len()=>Ok(Some(n)),
            Err(e) if matches!(e.kind(),ErrorKind::WouldBlock|ErrorKind::Interrupted)=>Ok(None),
            _=>Err(Fault::Transport),
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/attached/smb/connection/transport/pending/tests.rs"]
mod tests;
