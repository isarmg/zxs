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
#[path = "../tests/unit/attached/net/tests.rs"]
mod tests;
