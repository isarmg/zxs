//! Key order does not change encoded byte length. Measure before allocating
//! the canonical Value/byte result; each caller keeps its original named cap.
use crate::{Error, Result};
use data_serialization::Serialize;

pub(crate) fn encoded_length<T: Serialize + ?Sized>(
    value: &T,
    maximum: usize,
    exhausted: impl FnOnce() -> Error,
) -> Result<usize> {
    struct Limit {
        remaining: usize,
        exceeded: bool,
    }
    impl std::io::Write for Limit {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.remaining {
                self.exceeded = true;
                return Err(std::io::Error::other(
                    "Foreign canonical byte budget exceeded",
                ));
            }
            self.remaining -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut limit = Limit {
        remaining: maximum,
        exceeded: false,
    };
    match json_codec::to_writer(&mut limit, value) {
        Ok(()) => Ok(maximum - limit.remaining),
        Err(_) if limit.exceeded => Err(exhausted()),
        Err(error) => Err(error.into()),
    }
}
