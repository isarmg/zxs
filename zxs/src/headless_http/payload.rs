//! Length-bounded reads from the already selected immutable node descriptor.
use super::{HeaderExtras, headers};
use crate::{container_store::NodeBytes, workspace_protocol::PayloadWindow};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::FileExt,
};

pub(crate) struct PayloadReply {
    pub(crate) bytes: NodeBytes,
    pub(crate) window: PayloadWindow,
}

#[derive(Clone, Copy)]
pub(super) enum ContentRange {
    Interval { start: u64, end: u64, total: u64 },
    NoneAt { total: u64 },
}

pub(super) fn write_payload(
    output: &mut impl Write,
    reply: PayloadReply,
    keep: bool,
) -> io::Result<()> {
    let total = reply.bytes.node.body.length;
    let sha = reply.bytes.node.body.sha256;
    let (status, start, length, digest, content_range) = match reply.window {
        PayloadWindow::Full => ("200 OK", 0, total, Some(sha), None),
        PayloadWindow::Span(span) => {
            let (start, end) = span.bounds();
            (
                "206 Partial Content",
                start,
                span.length(),
                None,
                Some(ContentRange::Interval { start, end, total }),
            )
        }
        PayloadWindow::Unsatisfiable => {
            return headers(
                output,
                "416 Range Not Satisfiable",
                None,
                0,
                keep,
                HeaderExtras {
                    accept_ranges: true,
                    content_range: Some(ContentRange::NoneAt { total }),
                    ..HeaderExtras::default()
                },
            );
        }
    };
    let mut body = FileWindow::new(reply.bytes.file, start, length, total)?;
    headers(
        output,
        status,
        Some("application/octet-stream"),
        length,
        keep,
        HeaderExtras {
            digest,
            etag: Some(sha),
            content_range,
            accept_ranges: true,
            ..HeaderExtras::default()
        },
    )?;
    if io::copy(&mut body, output)? != length {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(())
}

struct FileWindow {
    exact: File,
    position: u64,
    end: u64,
}

impl FileWindow {
    fn new(exact: File, start: u64, length: u64, total: u64) -> io::Result<Self> {
        let end = start
            .checked_add(length)
            .filter(|end| *end <= total)
            .ok_or(io::ErrorKind::InvalidData)?;
        Ok(Self {
            exact,
            position: start,
            end,
        })
    }
}

impl Read for FileWindow {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let available = u64::try_from(output.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
        let want = usize::try_from(available.min(self.end - self.position))
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        if want == 0 {
            return Ok(0);
        }
        let size = self.exact.read_at(&mut output[..want], self.position)?;
        if size == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.position += u64::try_from(size).map_err(|_| io::ErrorKind::InvalidInput)?;
        Ok(size)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/payload_wire.rs"]
mod tests;
