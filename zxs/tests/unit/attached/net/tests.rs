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
