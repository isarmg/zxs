//! Helpers for IO related tasks.
//!
//! Use `ReaderStream` to turn an [`AsyncRead`] into a stream of byte chunks, and
//! `StreamReader` for the reverse conversion. These adapters work with streams
//! rather than a particular HTTP client or body type.
//!
//! With the `adapters-io-util` feature, `SyncIoBridge` exposes asynchronous I/O
//! through synchronous I/O traits. It is intended for a synchronous context,
//! such as a closure passed to `tokio::task::spawn_blocking`.
//!
//! [`AsyncRead`]: tokio::io::AsyncRead

mod copy_to_bytes;
mod inspect;
mod read_buf;
mod reader_stream;
pub mod simplex;
mod sink_writer;
mod stream_reader;
mod write_all_vectored;

cfg_adapters_io_util! {
    mod read_arc;
    pub use self::read_arc::read_exact_arc;

    mod sync_bridge;
    pub use self::sync_bridge::SyncIoBridge;
}

pub use self::copy_to_bytes::CopyToBytes;
pub use self::inspect::{InspectReader, InspectWriter};
pub use self::read_buf::read_buf;
pub use self::reader_stream::ReaderStream;
pub use self::sink_writer::SinkWriter;
pub use self::stream_reader::StreamReader;
pub use self::write_all_vectored::{write_all_vectored, WriteAllVectored};
pub use crate::adapters::util::{poll_read_buf, poll_write_buf};
