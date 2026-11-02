use super::read_to_end::read_to_end_internal;
use crate::io_traits::AsyncRead;
use crate::traits::future::Future;
use crate::traits::ready;
use core::task::{Context, Poll};
use std::pin::Pin;
use std::string::String;
use std::vec::Vec;
use std::{io, mem, str};

/// Future for the [`read_to_string`](super::AsyncReadExt::read_to_string) method.
///
/// See that method for how errors and cancellation affect the buffer and reader.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ReadToString<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut String,
    bytes: Vec<u8>,
    start_len: usize,
    finished: bool,
}

impl<R: ?Sized + Unpin> Unpin for ReadToString<'_, R> {}

impl<'a, R: AsyncRead + ?Sized + Unpin> ReadToString<'a, R> {
    pub(super) fn new(reader: &'a mut R, buf: &'a mut String) -> Self {
        let start_len = buf.len();
        Self {
            reader,
            bytes: mem::take(buf).into_bytes(),
            buf,
            start_len,
            finished: false,
        }
    }
}

fn read_to_string_internal<R: AsyncRead + ?Sized>(
    reader: Pin<&mut R>,
    cx: &mut Context<'_>,
    buf: &mut String,
    bytes: &mut Vec<u8>,
    start_len: usize,
) -> Poll<io::Result<usize>> {
    let ret = ready!(read_to_end_internal(reader, cx, bytes, start_len));
    debug_assert!(buf.is_empty());
    Poll::Ready(finish_string_read(buf, bytes, start_len, ret))
}

pub(super) fn finish_string_read(
    buf: &mut String,
    bytes: &mut Vec<u8>,
    start_len: usize,
    result: io::Result<usize>,
) -> io::Result<usize> {
    let result = match str::from_utf8(&bytes[start_len..]) {
        Ok(_) => result,
        Err(error) => {
            bytes.truncate(start_len);
            result.and_then(|_| Err(io::Error::new(io::ErrorKind::InvalidData, error)))
        }
    };
    // SAFETY: The prefix came from a valid String. Its appended suffix has
    // either validated as UTF-8 or been removed. The exclusive borrows keep
    // both allocations live and unaliased throughout this immediate swap.
    mem::swap(unsafe { buf.as_mut_vec() }, bytes);
    result
}

impl<A> Future for ReadToString<'_, A>
where
    A: AsyncRead + ?Sized + Unpin,
{
    type Output = io::Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self {
            reader,
            buf,
            bytes,
            start_len,
            finished: _,
        } = &mut *self;
        let ret = ready!(read_to_string_internal(
            Pin::new(reader),
            cx,
            buf,
            bytes,
            *start_len
        ));
        self.finished = true;
        Poll::Ready(ret)
    }
}

impl<R: ?Sized> Drop for ReadToString<'_, R> {
    fn drop(&mut self) {
        if !self.finished {
            self.bytes.truncate(self.start_len);
            // SAFETY: Reading only appended bytes, so truncation restores the
            // original valid String prefix. No user code runs during the swap.
            mem::swap(unsafe { self.buf.as_mut_vec() }, &mut self.bytes);
        }
    }
}
