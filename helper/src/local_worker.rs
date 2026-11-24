//! Private child-process custody, fixed local messages and sealed request bodies.
mod child;
pub(crate) mod transport;

pub(crate) use child::Worker;

fn bad(message: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}
