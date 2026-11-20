//! MS-SMB2 2.2.6 / 3.3.4.3 / 3.3.5.5.3. Only an unsigned, standalone,
//! fresh mechanism-selection/challenge MORE; this cannot encode STATUS_SUCCESS.
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
    let h=&m.header;
    if !plain_header(m) || matches!(sid,0|u64::MAX) || (h.session!=0 && h.session!=sid) ||
        token.is_empty() || token.len()>usize::from(u16::MAX) {
        return Err(Error::InvalidRequest);
    }
    let mut p=Vec::new();p.try_reserve_exact(72+token.len()).map_err(|_|Error::Limit)?;
    p.extend_from_slice(bytes(m.raw,0,64)?); // exact charge, MID and opaque Reserved/ProcessId
    p[8..12].copy_from_slice(&0xc000_0016u32.to_le_bytes()); // STATUS_MORE_PROCESSING_REQUIRED
    p[14..16].copy_from_slice(&grant.to_le_bytes());p[16..20].copy_from_slice(&(1|h.flags).to_le_bytes());
    p[20..24].fill(0);p[40..48].copy_from_slice(&sid.to_le_bytes()); // flags unsigned, Signature already zero
    p.extend_from_slice(&9u16.to_le_bytes());p.extend_from_slice(&0u16.to_le_bytes()); // no GUEST/NULL/ENCRYPT
    p.extend_from_slice(&72u16.to_le_bytes());p.extend_from_slice(&(token.len() as u16).to_le_bytes());
    p.extend_from_slice(token);Ok(p)
}
