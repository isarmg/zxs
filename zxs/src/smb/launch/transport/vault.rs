//! The already-selected descriptor is consumed once. Never reopen a path,
//! accept independent ids, or expose verifier bytes to the launch caller.
use super::{InheritedRun,vault_file};
use crate::{smb::session::VaultDecoder,system::reader::smb::runtime::QualifiedRustInstall};
use std::{io,os::unix::fs::FileExt,sync::Arc};
use zeroize::Zeroizing;

fn denied()->io::Error{
    io::Error::new(io::ErrorKind::PermissionDenied,"the original Rust SMB credential is unavailable")
}
impl InheritedRun {
    /// Only the strict decoder can supply the private gate. No local path,
    /// fd number, metadata alone or synthesized buffer is a production key.
    pub(in crate::smb) fn read_for_decoder(self:&Arc<Self>,_gate:&VaultDecoder,install:&QualifiedRustInstall)
        ->io::Result<Zeroizing<[u8;240]>>{
        if !install.is_original_start(self){return Err(denied());}
        install.require_current().map_err(|_|denied())?;
        // The lock covers a single move, never a read, lookup or credential check.
        let original=self.vault.lock().map_err(|_|denied())?.take().ok_or_else(denied)?;
        vault_file(&original.file,&original.stamp).map_err(|_|denied())?;
        let mut bytes=Zeroizing::new([0u8;240]);
        original.file.read_exact_at(&mut bytes[..],0).map_err(|_|denied())?;
        let mut tail=Zeroizing::new([0u8;1]);
        loop{
            match original.file.read_at(&mut tail[..],240){
                Ok(0)=>break,
                Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,
                _=>return Err(denied()),
            }
        }
        vault_file(&original.file,&original.stamp).map_err(|_|denied())?;
        drop(original);
        // A matching file is not authority if the actual original SQL/lock
        // owner changes before the decoder compares its entire public prefix.
        install.require_current().map_err(|_|denied())?;
        Ok(bytes)
    }
}
