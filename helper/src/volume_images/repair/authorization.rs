//! Entry boundary for explicit repair of the caller's original selected copy.
//! The retained transport is shared support, not authority to run an installer.
use super::RepairFormat;
use std::io;

pub(super) fn confirm(
    format: RepairFormat, preview: String, web: Option<u64>,
    require: impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    crate::sqlite_install::confirm_copy_repair(format, preview, web, require)
}

pub(crate) fn browser_worker() -> io::Result<()> {
    crate::sqlite_install::exec_copy_repair_browser()
}

pub(crate) const TAG:&[u8]=b"ZXS-HELPER/COPY-REPAIR-WEB-2\0";
pub(crate) const SHAPE:usize=TAG.len()+32+1+8;
pub(crate) fn no()->io::Error{io::Error::new(io::ErrorKind::InvalidData,"the original working-copy request was not returned by its local browser worker; no repair was started")}
fn byte_size(n:u64)->bool{(1..=i64::MAX as u64).contains(&n)}
pub(crate) fn raw_scope(raw:&[u8])->io::Result<(RepairFormat,u64)>{
    let b=raw.strip_prefix(TAG).filter(|r|r.len()==41).ok_or_else(no)?;
    let format=RepairFormat::from_scope_code(b[32]).ok_or_else(no)?;
    let n=u64::from_be_bytes(b[33..].try_into().expect("bounded fixed size"));
    if b[..32]==[0;32]||!byte_size(n){return Err(no());}Ok((format,n))
}

#[cfg(test)]
#[path="../../../tests/disk_web/repair_scope.rs"]
mod tests;
