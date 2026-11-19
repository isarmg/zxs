//! Fresh SQLite pool only. No old system record, credential or user-facing write entry.
mod request;
mod engine;
mod originals;
mod approval;
mod reservation;
mod stage;

const DATABASE:&str="SYSTEM.sqlite3";
const MARKER:&[u8]=b".zxs-new-ffffffffffffffffffffffffffffffff-0";
fn bad(message:&'static str)->std::io::Error{std::io::Error::new(std::io::ErrorKind::InvalidData,message)}
fn unsupported(message:&'static str)->std::io::Error{std::io::Error::new(std::io::ErrorKind::Unsupported,message)}
