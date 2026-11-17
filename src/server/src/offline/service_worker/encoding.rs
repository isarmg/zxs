//! Collect the actual compact JSON frame, rejecting before extending the
//! buffer past its real pipe limit. Never write a partial frame to a child
//! or caller: a successful local encode is not a native completion receipt.
use super::{Failure, Result, Serialize};
use std::{fmt, io};

struct Frame {
    bytes: Vec<u8>,
    maximum: usize,
    exceeded: bool,
}
impl io::Write for Frame {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.exceeded || bytes.len() > self.maximum - self.bytes.len() {
            self.exceeded = true;
            return Err(io::Error::other("bounded pipe encoding exceeds its limit"));
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            let target = self.bytes.capacity().max(128).saturating_mul(2)
                .max(required).min(self.maximum);
            self.bytes.try_reserve_exact(target - self.bytes.len()).map_err(io::Error::other)?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

pub(super) fn frame(
    value: &impl Serialize, maximum: usize, on_limit: impl FnOnce() -> Failure,
) -> Result<Vec<u8>> {
    let mut output = Frame { bytes: Vec::new(), maximum, exceeded: false };
    match json_codec::to_writer(&mut output, value) {
        _ if output.exceeded => Err(on_limit().into()),
        Ok(()) => Ok(output.bytes),
        Err(error) => Err(error.into()),
    }
}

// A top-level Display is all the child wire already permits. If it cannot fit,
// discard even the prefix instead of serializing an unbounded source or string.
const MAX_DIAGNOSTIC: usize = 4096;
pub(super) fn diagnostic(error: &(impl fmt::Display + ?Sized)) -> String {
    struct Message { text: String, exceeded: bool }
    impl fmt::Write for Message {
        fn write_str(&mut self, chunk: &str) -> fmt::Result {
            if self.exceeded || chunk.len() > MAX_DIAGNOSTIC - self.text.len() {
                self.exceeded = true;
                return Err(fmt::Error);
            }
            self.text.push_str(chunk);
            Ok(())
        }
    }
    let mut message = Message { text: String::with_capacity(MAX_DIAGNOSTIC), exceeded: false };
    match fmt::write(&mut message, format_args!("{error}")) {
        Ok(()) if !message.exceeded => message.text,
        _ => "storage worker failed; inspect the original operation before retrying".into(),
    }
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/offline/worker_frame_tests.rs"]
mod tests;
