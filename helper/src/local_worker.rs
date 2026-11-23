//! Private child-process custody, fixed local messages and sealed request bodies.
pub(crate) mod transport;
mod child;
mod material;

pub(crate) use child::Worker;
pub(crate) use material::{sealed, sealed_owner};

fn bad(message:&'static str)->std::io::Error{std::io::Error::new(std::io::ErrorKind::InvalidData,message)}
