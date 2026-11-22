//! Shared error classification for accept on an already qualified Linux TCP listener.
use std::io;

pub(crate) fn retry_tcp_accept(error: &io::Error) -> bool {
    // Linux can return the new connection's pending network error directly.
    // Apply this only to accept(), never to listener or installation checks.
    error.raw_os_error().is_some_and(|code| {
        code == libc::EWOULDBLOCK
            || matches!(
                code,
                libc::EINTR
                    | libc::ECONNABORTED
                    | libc::ECONNRESET
                    | libc::ENETDOWN
                    | libc::EPROTO
                    | libc::ENOPROTOOPT
                    | libc::EHOSTDOWN
                    | libc::ENONET
                    | libc::EHOSTUNREACH
                    | libc::EOPNOTSUPP
                    | libc::ENETUNREACH
                    | libc::ENETRESET
                    | libc::ETIMEDOUT
                    | libc::ENOSR
                    | libc::ESOCKTNOSUPPORT
                    | libc::EPROTONOSUPPORT
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_accept_pending_connection_errors_are_not_listener_faults() {
        for code in [
            libc::EAGAIN,
            libc::EINTR,
            libc::ECONNABORTED,
            libc::ECONNRESET,
            libc::ENETDOWN,
            libc::EPROTO,
            libc::ENOPROTOOPT,
            libc::EHOSTDOWN,
            libc::ENONET,
            libc::EHOSTUNREACH,
            libc::EOPNOTSUPP,
            libc::ENETUNREACH,
            libc::ENETRESET,
            libc::ETIMEDOUT,
            libc::ENOSR,
            libc::ESOCKTNOSUPPORT,
            libc::EPROTONOSUPPORT,
        ] {
            assert!(retry_tcp_accept(&io::Error::from_raw_os_error(code)));
        }
        for code in [libc::EBADF, libc::EINVAL, libc::EMFILE, libc::ENOTSOCK] {
            assert!(!retry_tcp_accept(&io::Error::from_raw_os_error(code)));
        }
        assert!(!retry_tcp_accept(&io::ErrorKind::PermissionDenied.into()));
    }
}
