use super::*;

#[test]
fn kernel_timeouts_never_encode_zero_or_the_infinite_sentinel() {
    assert!(matches!(
        CommandTimeout::remaining(Duration::ZERO),
        Err(Error::TimedOut)
    ));
    assert_eq!(
        CommandTimeout::remaining(Duration::from_nanos(1))
            .unwrap()
            .0,
        1
    );
    assert_eq!(
        CommandTimeout::remaining(Duration::from_millis(2))
            .unwrap()
            .0,
        2
    );
    assert_eq!(
        CommandTimeout::remaining(Duration::from_micros(2001))
            .unwrap()
            .0,
        3
    );
    assert!(CommandTimeout::remaining(Duration::from_millis(u64::from(u32::MAX))).is_err());
    assert!(CommandTimeout::remaining(Duration::from_secs(u64::MAX)).is_err());
}

#[test]
fn block_descriptor_boundary_rejects_regular_files_and_sockets() {
    use std::os::fd::AsFd;
    let file = tempfile::tempfile().unwrap();
    assert!(matches!(
        BlockDevice::bind(file.as_fd()),
        Err(Error::InvalidResponse { .. })
    ));
    let (socket, _) = std::os::unix::net::UnixStream::pair().unwrap();
    assert!(matches!(
        BlockDevice::bind(socket.as_fd()),
        Err(Error::InvalidResponse { .. })
    ));
}
