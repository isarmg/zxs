use std::pin::Pin;
use std::task::{ready, Poll};

use crate::rt::{Read, ReadBuf, Write};

use std::future::poll_fn;

pub(crate) async fn read<T>(io: &mut T, buf: &mut [u8]) -> Result<usize, std::io::Error>
where
    T: Read + Unpin,
{
    poll_fn(move |cx| {
        let mut buf = ReadBuf::new(buf);
        ready!(Pin::new(&mut *io).poll_read(cx, buf.unfilled()))?;
        Poll::Ready(Ok(buf.filled().len()))
    })
    .await
}

pub(crate) async fn write_all<T>(io: &mut T, buf: &[u8]) -> Result<(), std::io::Error>
where
    T: Write + Unpin,
{
    let mut n = 0;
    poll_fn(move |cx| {
        while n < buf.len() {
            let written = ready!(Pin::new(&mut *io).poll_write(cx, &buf[n..]))?;
            if written == 0 {
                return Poll::Ready(Err(std::io::ErrorKind::WriteZero.into()));
            }
            n += written;
        }
        Poll::Ready(Ok(()))
    })
    .await
}
