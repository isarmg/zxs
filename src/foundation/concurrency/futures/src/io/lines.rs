use super::read_line::read_line_internal;
use crate::io_traits::AsyncBufRead;
use crate::traits::ready;
use crate::traits::stream::Stream;
use core::task::{Context, Poll};
use pin_project_lite::pin_project;
use std::io;
use std::mem;
use std::pin::Pin;
use std::string::String;
use std::vec::Vec;

pin_project! {
    /// Stream for the [`lines`](super::AsyncBufReadExt::lines) method.
    #[derive(Debug)]
    #[must_use = "streams do nothing unless polled"]
    pub struct Lines<R> {
        #[pin]
        reader: R,
        buf: String,
        bytes: Vec<u8>,
        read: usize,
    }
}

impl<R: AsyncBufRead> Lines<R> {
    pub(super) fn new(reader: R) -> Self {
        Self {
            reader,
            buf: String::new(),
            bytes: Vec::new(),
            read: 0,
        }
    }
}

impl<R: AsyncBufRead> Stream for Lines<R> {
    type Item = io::Result<String>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let n = match ready!(read_line_internal(
            this.reader,
            cx,
            this.buf,
            this.bytes,
            this.read
        )) {
            Ok(n) => n,
            Err(error) => {
                this.buf.clear();
                return Poll::Ready(Some(Err(error)));
            }
        };
        *this.read = 0;
        if n == 0 && this.buf.is_empty() {
            return Poll::Ready(None);
        }
        if this.buf.ends_with('\n') {
            this.buf.pop();
            if this.buf.ends_with('\r') {
                this.buf.pop();
            }
        }
        Poll::Ready(Some(Ok(mem::take(this.buf))))
    }
}
