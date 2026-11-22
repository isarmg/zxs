//! The ZXS subset of SMB2 FILE SET_INFO (ordinary disposition and EOF).
//! Only the authenticated paid member can reach the public request facade.
//! Parsed intent is never an OPEN, a storage grant, or a successful change.
use super::{
    bytes, io_fields::{IoFieldError, OpenReference}, message::Message, u16le, u32le, u64le,
};
use crate::smb::connection::{Fault, sessions::ordinary::VerifiedMember};

mod rename;
pub(in crate::smb) use rename::{RenameError,RenameRequest};

pub(in crate::smb) const SET_INFO: u16 = 0x0011;
const INFO_FILE: u8 = 1;
const FILE_DISPOSITION: u8 = 13;
const FILE_END_OF_FILE: u8 = 20;
const FIXED_END: usize = 96;
const NO_CURRENT_TARGET: u32 = 0x1000_0000 | 0x2000_0000; // DFS or replay

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum Error {
    Session(Fault),
    WrongCommand,
    NotInProfile,
    Malformed,
    ExceedsMaxTransact,
    NoExplicitOpen,
    InformationLength,
    NegativeEof,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum Change {
    /// Ask the original OPEN's owner to establish ordinary disposition.
    /// Do not remove an object or issue an R receipt from this alone.
    MarkForDelete,
    /// Remove ordinary disposition if still authorized. This cannot remove
    /// a CREATE-supplied FILE_DELETE_ON_CLOSE mode or undo a committed delete.
    ClearDisposition,
    /// Set the absolute end on the original staged writer, not allocation
    /// size, append position, or a separately resolved pathname.
    EndOfFile(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Fields { open: OpenReference, change: Change, input_length: u32 }

#[must_use]
pub(in crate::smb) struct FileSetRequest<'step, 'raw> {
    original: VerifiedMember<'step, 'raw>,
    fields: Fields,
}

impl<'step, 'raw> FileSetRequest<'step, 'raw> {
    pub(in crate::smb) fn parse(original: VerifiedMember<'step, 'raw>)
        -> Result<Self, (VerifiedMember<'step, 'raw>, Error)>
    {
        if let Err(e) = original.session().recheck() {
            return Err((original, Error::Session(e)));
        }
        let fields = match fields(original.message(), original.max_transact()) {
            Ok(f) => f,
            Err(e) => return Err((original, e)),
        };
        Ok(Self { original, fields })
    }

    pub(in crate::smb) fn original(&self) -> &VerifiedMember<'step, 'raw> { &self.original }
    pub(in crate::smb) fn open(&self) -> OpenReference { self.fields.open }
    pub(in crate::smb) fn change(&self) -> Change { self.fields.change }
    /// Keep the declared size; a fixed class prefix does not lower the bill.
    pub(in crate::smb) fn input_length(&self) -> u32 { self.fields.input_length }
}

fn fixed(m:&Message<'_>)->Result<(),Error>{
    use Error::{WrongCommand, Malformed};
    if m.header.command != SET_INFO { return Err(WrongCommand); }
    if m.raw.len() < FIXED_END || u16le(m.raw,64).map_err(|_|Malformed)? != 33 {
        return Err(Malformed);
    }
    Ok(())
}
impl Message<'_>{
    /// Shape selector for the known compound class; not an OPEN or authority.
    pub(in crate::smb) fn is_basic_disposition_request(&self)->bool{
        self.header.command==SET_INFO&&self.raw.get(66..68)==Some(&[INFO_FILE,FILE_DISPOSITION][..])
    }

    /// Original bounded coordinate, not a decoded info class or mutation.
    /// The dispatcher resolves its Session/Tree/OPEN before acting on either.
    pub(in crate::smb) fn set_info_target(&self)->Result<OpenReference,IoFieldError>{
        fixed(self).map_err(|e|match e{
            Error::WrongCommand=>IoFieldError::WrongCommand,
            _=>IoFieldError::Malformed,
        })?;
        self.io_open_reference(80)
    }
}

fn fields(m: &Message<'_>, max_transact: u32) -> Result<Fields, Error> {
    use Error::{NotInProfile, Malformed, ExceedsMaxTransact, InformationLength, NegativeEof, NoExplicitOpen};
    fixed(m)?;
    if m.header.flags & NO_CURRENT_TARGET != 0 { return Err(NotInProfile); }
    let p = m.raw;
    if p[66] != INFO_FILE || !matches!(p[67], FILE_DISPOSITION | FILE_END_OF_FILE) {
        // Including local/extended flags such as FileDispositionInformationEx.
        // Do not ever reinterpret a different class as ordinary deletion.
        return Err(NotInProfile);
    }
    // §2.2.39: ignore Reserved, but AdditionalInformation is zero for FILE.
    if u32le(p, 76).map_err(|_| Malformed)? != 0 { return Err(Malformed); }
    let input_length = u32le(p, 68).map_err(|_| Malformed)?;
    if input_length > max_transact { return Err(ExceedsMaxTransact); }
    if input_length == 0 { return Err(Malformed); }
    let at = usize::from(u16le(p, 72).map_err(|_| Malformed)?);
    if at < FIXED_END { return Err(Malformed); }
    let input = bytes(p, at, usize::try_from(input_length).map_err(|_| Malformed)?)
        .map_err(|_| Malformed)?;
    let change = if p[67] == FILE_DISPOSITION {
        // MS-FSCC §§2.4.11/2.1.8: this byte is a Boolean, not flags.
        if input[0] == 0 { Change::ClearDisposition } else { Change::MarkForDelete }
    } else {
        // MS-FSCC §2.4.14: the wire integer is signed and nonnegative.
        let n = u64le(input, 0).map_err(|_| InformationLength)?;
        if n > i64::MAX as u64 { return Err(NegativeEof); }
        Change::EndOfFile(n)
    };
    let open = m.io_open_reference(80).map_err(|e| match e {
        IoFieldError::NoExplicitOpen => NoExplicitOpen,
        _ => Malformed,
    })?;
    Ok(Fields { open, change, input_length })
}

#[cfg(test)]
#[path = "../../../tests/smb/set_info.rs"]
mod tests;
