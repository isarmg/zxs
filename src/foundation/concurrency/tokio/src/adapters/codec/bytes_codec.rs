use crate::adapters::codec::decoder::Decoder;
use crate::adapters::codec::encoder::Encoder;

use bytes::{BufMut, Bytes, BytesMut};
use std::io;

/// A simple [`Decoder`] and [`Encoder`] implementation that just ships bytes around.
///
/// Decoding returns all bytes currently in the input buffer as one chunk, or
/// `None` when the buffer is empty. With `FramedRead`, chunk boundaries depend on
/// how data is buffered, not on message boundaries or a fixed frame length.
/// Encoding appends the supplied `Bytes` or `BytesMut` to the output buffer
/// without adding framing.
///
/// [`Decoder`]: tokio::adapters::codec::Decoder
/// [`Encoder`]: tokio::adapters::codec::Encoder
///
/// # Example
///
/// Turn an [`AsyncRead`] into a stream of `Result<`[`BytesMut`]`, `[`Error`]`>`.
/// This example requires the `adapters-codec`, `fs`, `macros`, and `rt` features.
///
/// [`AsyncRead`]: tokio::io::AsyncRead
/// [`BytesMut`]: bytes::BytesMut
/// [`Error`]: std::io::Error
///
/// ```
/// # mod hidden {
/// # #[allow(unused_imports)]
/// use tokio::fs::File;
/// # }
/// use tokio::io::AsyncRead;
/// use tokio::adapters::codec::{FramedRead, BytesCodec};
///
/// # enum File {}
/// # impl File {
/// #     async fn open(_name: &str) -> Result<impl AsyncRead, std::io::Error> {
/// #         use std::io::Cursor;
/// #         Ok(Cursor::new(vec![0, 1, 2, 3, 4, 5]))
/// #     }
/// # }
/// #
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), std::io::Error> {
/// let my_async_read = File::open("filename.txt").await?;
/// let my_stream_of_bytes = FramedRead::new(my_async_read, BytesCodec::new());
/// # Ok(())
/// # }
/// ```
///
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Default)]
pub struct BytesCodec(());

impl BytesCodec {
    /// Creates a new `BytesCodec` for shipping around raw bytes.
    pub fn new() -> BytesCodec {
        BytesCodec(())
    }
}

impl Decoder for BytesCodec {
    type Item = BytesMut;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<BytesMut>, io::Error> {
        if !buf.is_empty() {
            let len = buf.len();
            Ok(Some(buf.split_to(len)))
        } else {
            Ok(None)
        }
    }
}

impl Encoder<Bytes> for BytesCodec {
    type Error = io::Error;

    fn encode(&mut self, data: Bytes, buf: &mut BytesMut) -> Result<(), io::Error> {
        buf.reserve(data.len());
        buf.put(data);
        Ok(())
    }
}

impl Encoder<BytesMut> for BytesCodec {
    type Error = io::Error;

    fn encode(&mut self, data: BytesMut, buf: &mut BytesMut) -> Result<(), io::Error> {
        buf.reserve(data.len());
        buf.put(data);
        Ok(())
    }
}
