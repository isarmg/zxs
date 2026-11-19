//! Exact ZXS display-component spelling for logical objects. A management
//! label is not a host path or an advertised SMB service alias.
use crate::fs::invalid;
use std::io;
pub(crate) const MAX:usize=255;
pub(crate) fn component(raw:&[u8],max:usize)->io::Result<&str>{
    if raw.is_empty()||raw.len()>max{return Err(invalid("display name exceeds its component byte budget or is empty"));}
    let s=std::str::from_utf8(raw).map_err(|_|invalid("display name is not strict UTF-8"))?;
    if s=="."||s==".."||s.ends_with('.')||s.ends_with(' ')||s.encode_utf16().count()>MAX||
        s.chars().any(|c|c.is_control()||matches!(c,'<'|'>'|':'|'"'|'/'|'\\'|'|'|'?'|'*')){
        return Err(invalid("not a single supported business display name"));}
    Ok(s)
}
