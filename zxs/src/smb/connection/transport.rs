//! Duplex I/O for one already accepted TCP stream. No bind/listen/connect,
//! cloning of an OS handle, or attaching a new socket to an existing session.
use super::{Fault,Result};
use crate::smb::wire::direct::Frame;
use std::{io::{ErrorKind,Read,Write},net::TcpStream,time::{Duration,Instant}};

const IO_BUDGET:Duration=Duration::from_secs(5);
pub(super) fn deadline()->Result<Instant> {Instant::now().checked_add(IO_BUDGET).ok_or(Fault::Clock)}
fn remaining(end:Instant)->Result<Duration> {
    let left=end.saturating_duration_since(Instant::now());
    if left.is_zero() {Err(Fault::Transport)} else {Ok(left)}
}

#[cfg(test)]
pub(super) trait FixtureIo:Read+Write+Send+Sync {}
#[cfg(test)]
impl<T:Read+Write+Send+Sync> FixtureIo for T {}
pub(super) enum DirectStream {
    Tcp(TcpStream),
    #[cfg(test)]
    Fixture(Box<dyn FixtureIo>),
}
impl Drop for DirectStream {
    fn drop(&mut self) {
        match self {
            Self::Tcp(s)=>{let _=s.shutdown(std::net::Shutdown::Both);}
            #[cfg(test)]
            Self::Fixture(_)=>{},
        }
    }
}
impl DirectStream {
    /// Read exactly one bounded record, not read_to_end and not one read per
    /// message. The same absolute budget covers the entire prefix AND body.
    /// The caller must discard this stream on an error (position ambiguous).
    pub(super) fn read_record(&mut self,limit:usize,end:Instant)->Result<Vec<u8>> {
        let mut prefix=[0;4];self.fill(&mut prefix,end)?;
        let n=Frame::payload_len(prefix).map_err(|_|Fault::FrameSize)?;
        if n>limit {return Err(Fault::FrameSize);}
        let mut record=Vec::new();
        record.try_reserve_exact(4+n).map_err(|_|Fault::Resources)?;
        record.extend_from_slice(&prefix);record.resize(4+n,0);
        self.fill(&mut record[4..],end)?;Ok(record)
    }
    fn fill(&mut self,mut dst:&mut [u8],end:Instant)->Result<()> {
        while !dst.is_empty() {
            let left=remaining(end)?;
            let got=match self {
                Self::Tcp(s)=>{
                    s.set_read_timeout(Some(left)).map_err(|_|Fault::Transport)?;
                    s.read(dst)
                }
                #[cfg(test)]
                Self::Fixture(s)=>s.read(dst),
            };
            match got {
                Ok(n) if n>0 && n<=dst.len()=>{let tail=dst.split_at_mut(n).1;dst=tail;}
                Err(e) if e.kind()==ErrorKind::Interrupted=>continue,
                _=>return Err(Fault::Transport),
            }
        }
        remaining(end).map(|_|())
    }

    /// Exact SMB bytes already bound into the transcript or reply ledger.
    /// Only their 4-byte framing prefix is added; it never enters the hash.
    pub(super) fn write_smb(&mut self,smb:&[u8],current:impl FnMut()->Result<()>)->Result<()> {
        let prefix=u32::try_from(smb.len()).map_err(|_|Fault::FrameSize)?.to_be_bytes();
        Frame::payload_len(prefix).map_err(|_|Fault::FrameSize)?;
        self.write_parts([prefix.as_slice(),smb],current)
    }
    /// Ordinary signed replies may already own a framed, byte-verified record.
    pub(super) fn write_record(&mut self,record:&[u8],current:impl FnMut()->Result<()>)->Result<()> {
        self.write_parts([record],current)
    }
    fn write_parts<const N:usize>(&mut self,parts:[&[u8];N],mut current:impl FnMut()->Result<()>)->Result<()> {
        let end=deadline()?;
        for mut bytes in parts {
            while !bytes.is_empty() {
                current()?; // never keep a credential/global lock during I/O
                let left=remaining(end)?;
                let written=match self {
                    Self::Tcp(s)=>{
                        s.set_write_timeout(Some(left)).map_err(|_|Fault::Transport)?;
                        s.write(bytes)
                    }
                    #[cfg(test)]
                    Self::Fixture(s)=>s.write(bytes),
                };
                match written {
                    Ok(n) if n>0 && n<=bytes.len()=>bytes=&bytes[n..],
                    Err(e) if e.kind()==ErrorKind::Interrupted=>continue,
                    _=>return Err(Fault::Transport),
                }
            }
        }
        remaining(end).map(|_|())
    }
}
