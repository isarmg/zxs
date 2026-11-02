use super::read_to_string::finish_string_read;
use super::read_until::read_until_internal;
use crate::io_traits::AsyncBufRead;
use crate::traits::future::Future;
use crate::traits::ready;
use core::task::{Context, Poll};
use std::io;
use std::mem;
use std::pin::Pin;
use std::string::String;
use std::vec::Vec;

/// Future for the [`read_line`](super::AsyncBufReadExt::read_line) method.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ReadLine<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut String,
    bytes: Vec<u8>,
    read: usize,
    finished: bool,
}

impl<R: ?Sized + Unpin> Unpin for ReadLine<'_, R> {}

impl<'a, R: AsyncBufRead + ?Sized + Unpin> ReadLine<'a, R> {
    pub(super) fn new(reader: &'a mut R, buf: &'a mut String) -> Self {
        Self {
            reader,
            bytes: mem::take(buf).into_bytes(),
            buf,
            read: 0,
            finished: false,
        }
    }
}

pub(super) fn read_line_internal<R: AsyncBufRead + ?Sized>(
    reader: Pin<&mut R>,
    cx: &mut Context<'_>,
    buf: &mut String,
    bytes: &mut Vec<u8>,
    read: &mut usize,
) -> Poll<io::Result<usize>> {
    let ret = ready!(read_until_internal(reader, cx, b'\n', bytes, read));
    let start_len = bytes.len() - *read;
    let ret = finish_string_read(buf, bytes, start_len, ret);
    *read = 0;
    Poll::Ready(ret)
}

impl<R: AsyncBufRead + ?Sized + Unpin> Future for ReadLine<'_, R> {
    type Output = io::Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self {
            reader,
            buf,
            bytes,
            read,
            finished: _,
        } = &mut *self;
        let ret = ready!(read_line_internal(Pin::new(reader), cx, buf, bytes, read));
        self.finished = true;
        Poll::Ready(ret)
    }
}

impl<R: ?Sized> Drop for ReadLine<'_, R> {
    fn drop(&mut self) {
        // restore old string contents
        if !self.finished {
            self.bytes.truncate(self.bytes.len() - self.read);
            // Safety: `bytes` is valid UTF-8 because it was taken from a String
            // and the newly read bytes have been removed.
            mem::swap(unsafe { self.buf.as_mut_vec() }, &mut self.bytes);
        }
    }
}
