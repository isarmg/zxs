//! The same complete-body rule applies at HTTP/import intake and recycle copy.
use super::{Body, Error, invalid};
use crate::hash::Sha256;
use std::io::{self, Read, Write};

impl Body {
    /// Reader is already bounded to this one request/file. This verifies it,
    /// not the medium or authorization: callers retain their own custody and
    /// decide when to sync and publish the output.
    pub(super) fn copy_exact(
        &self,
        input: &mut impl Read,
        output: &mut impl Write,
        before_chunk: impl Fn() -> io::Result<()>,
    ) -> Result<(), Error> {
        let mut digest = Sha256::new();
        let mut left = self.length;
        let mut block = vec![0; 64 * 1024];
        while left != 0 {
            before_chunk()?;
            let take = usize::try_from(left.min(block.len() as u64))
                .map_err(|_| invalid("invalid node intake window"))?;
            let n = read(input, &mut block[..take])?;
            if n == 0 {
                return Err(Error::WrongLength);
            }
            digest.update(&block[..n])?;
            output.write_all(&block[..n])?;
            left -= n as u64;
        }
        if read(input, &mut block[..1])? != 0 {
            return Err(Error::WrongLength);
        }
        if digest.finish() != self.sha256 {
            return Err(Error::WrongDigest);
        }
        Ok(())
    }
}

fn read(input: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        match input.read(buffer) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            value => return value,
        }
    }
}
