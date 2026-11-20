//! The first TREE_CONNECT boundary: one original authenticated request,
//! parsed target spelling, and no host lookup, tree, access mask or reply.
use crate::smb::{
    connection::{Fault as ConnectionFault, sessions::ordinary::VerifiedMember},
    wire::path_fields::{FieldError, TREE_SERVER_UNITS, TREE_SHARE_UNITS},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum Error {
    Session(ConnectionFault),
    Field(FieldError),
    BadUnc,
    UnknownShare,
    Resources,
}

/// A request for the one ZXS share. This is not an accepted share: the
/// actual listener's server/alias scope and the current Core principal and
/// unified root still need to be checked. Keep the original paid member for
/// both the operation and its eventual reply; never use naked wire IDs.
#[must_use]
pub(in crate::smb) struct TreeConnectRequest<'step, 'raw> {
    original: VerifiedMember<'step, 'raw>,
    server: String,
}

impl<'step, 'raw> TreeConnectRequest<'step, 'raw> {
    pub(in crate::smb) fn parse(original: VerifiedMember<'step, 'raw>)
        -> Result<Self, (VerifiedMember<'step, 'raw>, Error)>
    {
        let parsed = original.session().recheck().map_err(Error::Session)
            .and_then(|()| original.message().tree_connect_name_field().map_err(Error::Field))
            .and_then(decode_unc);
        match parsed {
            Ok(server) => Ok(Self { original, server }),
            Err(why) => Err((original, why)),
        }
    }

    pub(in crate::smb) fn original(&self) -> &VerifiedMember<'step, 'raw> { &self.original }

    /// Client spelling only, not a canonical hostname or an authority for
    /// a pool. The eventual resolver uses only trusted service/alias scope.
    pub(in crate::smb) fn server_spelling(&self) -> &str { &self.server }

    /// Kernel metadata through this same member, never a separate endpoint
    /// from a different connection or a name from this client's UNC.
    pub(in crate::smb) fn accepted_local(&self) -> Option<std::net::SocketAddr> {
        self.original.session().accepted_local()
    }
}

// Apply the share-name rules from MS-FSCC 2.1.6, not the ZXS business-file
// rules. A well-formed different share is not a malformed UNC.
fn illegal_share_char(c: char) -> bool {
    c <= '\x1f' || matches!(c, '"' | '\\' | '/' | '[' | ']' | ':' | '|' |
                                  '<' | '>' | '+' | '=' | ';' | ',' | '*' | '?')
}

fn server_shape(server: &str) -> bool {
    // A bare textual IPv6 address is not host:port. Windows' IPv6 literal
    // DNS spelling is left intact just like any other named alias.
    if server.contains(':') {
        return server.parse::<std::net::Ipv6Addr>().is_ok();
    }
    let labels = server.strip_suffix('.').unwrap_or(server);
    if labels.is_empty() || labels.split('.').any(|p| p.trim().is_empty()) ||
        server.chars().any(|c| illegal_share_char(c) || matches!(c, '@' | '#' | '\x7f'))
    {
        return false;
    }
    // An unqualified short computer name may be a NetBIOS spelling. Leave
    // those and non-ASCII candidate aliases opaque for the trusted resolver.
    if !server.is_ascii() || (server.len() <= 15 && !server.contains('.')) {
        return true;
    }
    // RFC 1035 §2.3.1/§2.3.4 and RFC 1123 §2.1: do not take an impossible
    // ASCII DNS label as a valid long server spelling. No IDNA or DNS here.
    labels.len() <= 253 && labels.split('.').all(|label| {
        let b = label.as_bytes();
        b.len() <= 63 && b.first().is_some_and(u8::is_ascii_alphanumeric) &&
            b.last().is_some_and(u8::is_ascii_alphanumeric) &&
            b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
    })
}

fn decode_unc(raw: &[u8]) -> Result<String, Error> {
    // Called only on the bounded UTF-16LE field of this message.
    let mut text = String::new();
    text.try_reserve_exact(raw.len() / 2 * 3).map_err(|_| Error::Resources)?;
    let units = raw.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]]));
    for c in char::decode_utf16(units) {
        text.push(c.map_err(|_| Error::BadUnc)?);
    }
    let rest = text.strip_prefix("\\\\").ok_or(Error::BadUnc)?;
    let (server, share) = rest.split_once('\\').ok_or(Error::BadUnc)?;
    if server.encode_utf16().count() > TREE_SERVER_UNITS || !server_shape(server) ||
        share.is_empty() || share.encode_utf16().count() > TREE_SHARE_UNITS ||
        share.chars().any(illegal_share_char)
    {
        return Err(Error::BadUnc);
    }
    // MS-SMB2 2.1 share-name matching is case-insensitive. This *one ASCII
    // service name* is not the Linux-case-sensitive child namespace.
    if !share.eq_ignore_ascii_case("ZXS") { return Err(Error::UnknownShare); }
    let end = 2 + server.len();
    text.truncate(end);
    drop(text.drain(..2));
    Ok(text)
}

#[cfg(test)]
#[path = "../../test/smb/backend_tree_connect.rs"]
mod tests;
