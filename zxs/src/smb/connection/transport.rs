//! Synchronous writer for the original TCP transport only. No accept loop,
//! listener, path/socket lookup or production attach-to-existing-session API.
use super::{Fault,Result};
use std::{io::{ErrorKind,Write},net::TcpStream,time::{Duration,Instant}};

pub(super) enum DirectOutput {
    // The future accepted-socket owner must keep the same reader/writer from
    // BEFORE negotiation. No production code currently installs an output.
    Tcp(TcpStream),
    #[cfg(test)]
    Fixture(Box<dyn Write+Send+Sync>),
}
impl Drop for DirectOutput {
    fn drop(&mut self) {
        match self {
            Self::Tcp(s)=>{let _=s.shutdown(std::net::Shutdown::Both);}
            #[cfg(test)]
            Self::Fixture(_)=>{},
        }
    }
}
impl DirectOutput {
    /// One immutable, already-framed record. No concurrent sends via the
    /// exclusive connection; a short syscall is not a completed SMB send.
    pub(super) fn write_record(&mut self,mut bytes:&[u8],mut current:impl FnMut()->Result<()>)->Result<()> {
        let deadline=Instant::now().checked_add(Duration::from_secs(5)).ok_or(Fault::Clock)?;
        while !bytes.is_empty() {
            current()?; // no credential/global lock is held during the write
            let left=deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {return Err(Fault::Transport);}
            let written=match self {
                Self::Tcp(stream)=>{
                    stream.set_write_timeout(Some(left)).map_err(|_|Fault::Transport)?;
                    stream.write(bytes)
                }
                #[cfg(test)]
                Self::Fixture(sink)=>sink.write(bytes),
            };
            match written {
                Ok(n) if n>0 && n<=bytes.len()=>bytes=&bytes[n..],
                Err(e) if e.kind()==ErrorKind::Interrupted=>continue,
                _=>return Err(Fault::Transport), // includes a zero write or ambiguous I/O error
            }
        }
        Ok(())
    }
}
