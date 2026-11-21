//! Structural parsing and command-body encoding only. This module does not
//! issue credits, authenticate, call a ZXS operation or send a reply.
pub(in crate::smb) mod credit_fields;
pub(in crate::smb) mod create_fields;
pub(in crate::smb) mod direct;
pub(in crate::smb) mod file_name_reply;
pub(in crate::smb) mod fixed_reply;
pub(in crate::smb) mod io_fields;
pub(in crate::smb) mod io_reply;
pub(in crate::smb) mod read_reply;
pub(in crate::smb) mod message;
pub(in crate::smb) mod negotiate;
pub(in crate::smb) mod negotiate_reply;
pub(in crate::smb) mod network_probe;
pub(in crate::smb) mod path_fields;
pub(in crate::smb) mod query_fields;
pub(in crate::smb) mod query_reply;
pub(in crate::smb) mod preauth;
pub(in crate::smb) mod set_info;
pub(in crate::smb) mod setup_reply;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum Error {
    InvalidFrame,
    UnsupportedEnvelope,
    InvalidRequest,
    Limit,
    UnsupportedDialect,
    NoPreauthHashInCommon,
}

type Result<T> = std::result::Result<T, Error>;

fn bytes(data: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    data.get(offset..)
        .and_then(|rest| rest.get(..length))
        .ok_or(Error::InvalidRequest)
}

fn field<const N: usize>(data: &[u8], at: usize) -> Result<[u8; N]> {
    bytes(data, at, N)?.try_into().map_err(|_| Error::InvalidRequest)
}

fn u16le(data: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(field(data, at)?))
}
fn u32le(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(field(data, at)?))
}
fn u64le(data: &[u8], at: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(field(data, at)?))
}
