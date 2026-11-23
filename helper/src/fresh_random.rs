//! Complete nonzero operation identities from the local OS.
use rustix::rand::{GetRandomFlags, getrandom};
use std::io;

/// Preserve the existing error for installation identities and repair offers.
pub(crate) fn operation<const N: usize>() -> io::Result<[u8; N]> {
    complete(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete or zero identity from the OS; nothing issued",
        )
    })
}

pub(crate) fn headless<const N: usize>() -> io::Result<[u8; N]> {
    complete(|| io::Error::other("OS did not return complete fresh randomness"))
}

fn complete<const N: usize>(incomplete: impl FnOnce() -> io::Error) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    loop {
        match getrandom(&mut bytes, GetRandomFlags::NONBLOCK) {
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
            Ok(written) if written == N && bytes != [0; N] => return Ok(bytes),
            _ => return Err(incomplete()),
        }
    }
}
