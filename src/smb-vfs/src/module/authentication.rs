//! Samba authentication boundary. Never infer authentication from a username,
//! a callback argument, a Unix uid, or a JSON capability alone.
use super::{paths::text, sdk};

pub(super) fn denied() -> std::io::Error {
    std::io::Error::from_raw_os_error(libc::EACCES)
}

/// # Safety
/// The handle and all non-null nested SDK objects obey the module host contract
/// and remain live on Samba's event-loop thread for this call. No host reference
/// escapes: the authenticated account name is copied into owned Rust storage.
pub(super) unsafe fn account_name(handle: *mut sdk::vfs_handle_struct) -> std::io::Result<String> {
    if handle.is_null() {
        return Err(denied());
    }
    // SAFETY: A non-null handle is live under the caller's SDK contract.
    let connection = unsafe { (*handle).conn };
    if connection.is_null() {
        return Err(denied());
    }
    // SAFETY: The checked connection is host-owned and live on this event loop.
    // force user replaces session_info before VFS connect, and guest only also
    // sets force_user. Neither may impersonate the configured account.
    if unsafe { (*connection).force_user } {
        return Err(denied());
    }
    // SAFETY: The connection is checked and immutable during this callback.
    let session = unsafe { (*connection).session_info };
    if session.is_null() {
        return Err(denied());
    }
    // SAFETY: A non-null session belongs to the live Samba connection.
    let (token, info) = unsafe { ((*session).security_token, (*session).info) };
    if token.is_null() || info.is_null() {
        return Err(denied());
    }
    // SAFETY: Samba owns the token and its SID array. Reject absent/empty SID
    // storage before official helpers inspect the primary SID or membership.
    if unsafe { (*token).num_sids == 0 || (*token).sids.is_null() } {
        return Err(denied());
    }
    // SAFETY: These official pinned-SDK helpers read the checked live token;
    // authenticated membership is positive evidence, guest/anonymous deny it.
    if unsafe {
        sdk::security_token_is_anonymous(token)
            || sdk::security_token_has_builtin_guests(token)
            || !sdk::security_token_has_nt_authenticated_users(token)
    } {
        return Err(denied());
    }
    // SAFETY: Samba supplies the session's NUL-terminated authenticated account
    // name. text rejects null and non-UTF-8; no request-supplied name is used.
    let name = unsafe { text((*info).account_name) }
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))?;
    smb_protocol::protocol::validate_username(name)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))?;
    Ok(name.to_owned())
}

#[cfg(test)]
#[path = "../../../../test/smb-vfs/unit/module/authentication_tests.rs"]
pub(super) mod tests;
