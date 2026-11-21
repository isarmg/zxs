//! The one signed, standalone, no-OPEN interface probe that we refuse.
//! No general IOCTL parser, output buffer, or interface enumeration.
use super::{bytes,message::{Message,Routing,SIGNED},u16le,u32le};

pub(in crate::smb) const IOCTL:u16=0x000b;
const QUERY_NETWORK_INTERFACE_INFO:u32=0x0014_01fc;
const FIXED:usize=120; // the header and 56 fixed bytes; StructureSize is still 57

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum NetworkProbe{
    NotFsctl,
    InvalidFileId,
    NoHandle{max_output:u32},
}

impl Message<'_>{
    pub(in crate::smb) fn network_interface_probe(&self)->Option<NetworkProbe>{
        let p=self.raw;
        if self.header.command!=IOCTL||self.header.flags&SIGNED==0||
            self.header.flags&!(SIGNED|0x70)!=0||
            !matches!(self.header.routing,Routing::Sync{tree} if tree!=0&&tree!=u32::MAX)||
            u32le(p,20)!=Ok(0)||!matches!(p.get(FIXED..),Some([]|[0]))||
            u16le(p,64)!=Ok(57)||u32le(p,68)!=Ok(QUERY_NETWORK_INTERFACE_INFO)||
            u32le(p,92)!=Ok(0)||u32le(p,96)!=Ok(0)||u32le(p,104)!=Ok(0)
        {return None;}
        // The same code still requires definitive errors for bad Flags and
        // either non-sentinel FileId half, after the session/tree lookup.
        if u32le(p,112)!=Ok(1){return Some(NetworkProbe::NotFsctl);}
        if bytes(p,72,16)!=Ok(&[0xff;16][..]){return Some(NetworkProbe::InvalidFileId);}
        // Neither offset is used for this zero-input/no-output request. The
        // reserved words are likewise opaque, but all bytes remain in the MAC.
        // Linux v6.12 includes the optional one zero byte after the fixed part.
        u32le(p,108).ok().map(|max_output|NetworkProbe::NoHandle{max_output})
    }
}
