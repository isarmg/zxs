//! MS-SMB2 2.2.6 / 3.3.4.3 / 3.3.5.5.3. Only an unsigned, standalone,
//! fresh mechanism-selection/challenge MORE and the unsigned final template.
//! The latter cannot be sent or signed by this wire module.
use super::{Error,Result,bytes,message::{Message,Routing}};

/// RFC 4178 NegTokenResp { negState=request-mic, supportedMech=NTLM }.
/// No responseToken, no unverified list MIC, no NTLM challenge or user data.
const REQUEST_MIC: [u8;23] = [
    0xa1,21,0x30,19,0xa0,3,0x0a,1,3,0xa1,12,6,10,0x2b,6,1,4,1,0x82,0x37,2,2,10,
];

pub(in crate::smb) fn plain_header(m:&Message<'_>)->bool {
    let h=&m.header;
    h.command == 1 && h.routing == (Routing::Sync{tree:0}) && h.flags & !0x70 == 0 &&
        m.raw.get(48..64) == Some(&[0u8;16][..])
}
pub(in crate::smb) fn select_more(m: &Message<'_>, sid:u64, grant:u16) -> Result<Vec<u8>> {
    if m.header.session!=0 {return Err(Error::InvalidRequest);}
    more(m,sid,grant,&REQUEST_MIC)
}
/// Only a MORE. No path here can return authenticated flags or success.
pub(in crate::smb) fn more(m:&Message<'_>,sid:u64,grant:u16,token:&[u8])->Result<Vec<u8>> {
    if token.is_empty() || (m.header.session!=0 && m.header.session!=sid) {return Err(Error::InvalidRequest);}
    Echo::from_request(m)?.encode(0xc000_0016,sid,grant,token)
}

/// Exact original request header, including the opaque 4-byte Reserved
/// value. Never infer these fields from a second "matching" request.
pub(super) struct Echo([u8;64]);
impl Echo {
    pub(super) fn from_request(m:&Message<'_>)->Result<Self> {
        if !plain_header(m) {return Err(Error::InvalidRequest);}
        Ok(Self(bytes(m.raw,0,64)?.try_into().map_err(|_|Error::InvalidRequest)?))
    }
    fn priority(&self)->u32 {u32::from(self.0[16])&0x70} // plain_header accepted only these bits
    pub(super) fn same_unsigned(&self,p:&[u8])->bool {
        p.get(6..8)==Some(&self.0[6..8]) && p.get(32..36)==Some(&self.0[32..36]) &&
            p.get(16..20)==Some(&(1|self.priority()).to_le_bytes()[..])
    }
    pub(super) fn unsigned_final(&self,grant:u16,token:&[u8])->Result<Vec<u8>> {
        self.encode(0,super::u64le(&self.0,40)?,grant,token)
    }
    fn encode(&self,status:u32,sid:u64,grant:u16,token:&[u8])->Result<Vec<u8>> {
        if matches!(sid,0|u64::MAX) || token.len()>usize::from(u16::MAX) {return Err(Error::InvalidRequest);}
        let mut p=Vec::new();p.try_reserve_exact(72+token.len()).map_err(|_|Error::Limit)?;
        p.extend_from_slice(&self.0); // same charge, MID, priority, Reserved and zero Signature
        p[8..12].copy_from_slice(&status.to_le_bytes());p[14..16].copy_from_slice(&grant.to_le_bytes());
        p[16..20].copy_from_slice(&(1|self.priority()).to_le_bytes());
        p[20..24].fill(0);p[40..48].copy_from_slice(&sid.to_le_bytes());
        p.extend_from_slice(&9u16.to_le_bytes());p.extend_from_slice(&0u16.to_le_bytes());
        p.extend_from_slice(&72u16.to_le_bytes());p.extend_from_slice(&(token.len() as u16).to_le_bytes());
        p.extend_from_slice(token);Ok(p)
    }
}
