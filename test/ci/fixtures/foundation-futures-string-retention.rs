// Public, bounded in-memory consumers of the String-reading futures.
use futures::Stream;
use futures::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt};
use futures::task::noop_waker_ref;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::str::Utf8Error;
use std::task::{Context, Poll};

const PREFIX: &str = "保留β:";

#[derive(Clone, Copy, Debug)]
enum Step {
    Bytes(&'static [u8]),
    Pending,
    Error(u8),
    Eof,
}

#[derive(Debug)]
struct ReadFailure(u8);

impl fmt::Display for ReadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fixture read failure {}", self.0)
    }
}

impl Error for ReadFailure {}

#[derive(Debug)]
struct ScriptedReader {
    steps: &'static [Step],
    step: usize,
    offset: usize,
    read_polls: usize,
    fill_polls: usize,
    consumed: usize,
}

impl ScriptedReader {
    fn new(steps: &'static [Step]) -> Self {
        Self {
            steps,
            step: 0,
            offset: 0,
            read_polls: 0,
            fill_polls: 0,
            consumed: 0,
        }
    }
}

impl AsyncBufRead for ScriptedReader {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        this.fill_polls += 1;
        match this.steps.get(this.step).copied().unwrap_or(Step::Eof) {
            Step::Bytes(bytes) => Poll::Ready(Ok(&bytes[this.offset..])),
            Step::Pending => {
                this.step += 1;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Step::Error(marker) => {
                this.step += 1;
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    ReadFailure(marker),
                )))
            }
            Step::Eof => Poll::Ready(Ok(&[])),
        }
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        let this = self.get_mut();
        match this.steps.get(this.step).copied().unwrap_or(Step::Eof) {
            Step::Bytes(bytes) => {
                assert!(amount <= bytes.len() - this.offset);
                this.offset += amount;
                this.consumed += amount;
                if this.offset == bytes.len() {
                    this.step += 1;
                    this.offset = 0;
                }
            }
            Step::Pending | Step::Error(_) | Step::Eof => assert_eq!(amount, 0),
        }
    }
}

impl AsyncRead for ScriptedReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.as_mut().get_mut().read_polls += 1;
        let count = match self.as_mut().poll_fill_buf(cx) {
            Poll::Ready(Ok(available)) => {
                let count = available.len().min(output.len());
                output[..count].copy_from_slice(&available[..count]);
                count
            }
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Pending => return Poll::Pending,
        };
        self.consume(count);
        Poll::Ready(Ok(count))
    }
}

fn poll_once<F: Future + Unpin>(future: &mut F) -> Poll<F::Output> {
    Pin::new(future).poll(&mut Context::from_waker(noop_waker_ref()))
}

fn complete<F: Future + Unpin>(mut future: F) -> F::Output {
    match poll_once(&mut future) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("invariant: this fixture's completion script has no Pending step"),
    }
}

fn assert_primary_io_error(error: &io::Error, marker: u8) {
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    let cause = error
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<ReadFailure>())
        .expect("invariant: the reader's concrete I/O cause must be retained");
    assert_eq!(cause.0, marker);
}

fn assert_utf8_error(error: &io::Error, valid_up_to: usize, error_len: Option<usize>) {
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    let cause = error
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<Utf8Error>())
        .expect("invariant: InvalidData must retain the concrete UTF-8 validation error");
    assert_eq!(cause.valid_up_to(), valid_up_to);
    assert_eq!(cause.error_len(), error_len);
}

#[test]
fn unpolled_read_to_string_preserves_the_original_string_without_reading() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"new"), Step::Eof]);
    let mut output = PREFIX.to_owned();
    drop(reader.read_to_string(&mut output));
    assert_eq!(output, PREFIX);
    assert_eq!(reader.read_polls, 0);
    assert_eq!(reader.fill_polls, 0);
    assert_eq!(reader.consumed, 0);
}

#[test]
fn pending_read_to_string_cancel_restores_original_string_after_valid_bytes() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"new"), Step::Pending]);
    let mut output = PREFIX.to_owned();
    let mut future = reader.read_to_string(&mut output);
    assert!(poll_once(&mut future).is_pending());
    drop(future);
    // Candidate policy matches ReadLine: discard uncommitted appended bytes.
    // This restores the caller's String; it does not rewind the reader.
    assert_eq!(output, PREFIX);
    assert_eq!(reader.consumed, 3);
}

#[test]
fn pending_read_to_string_cancel_restores_original_string_after_incomplete_utf8() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"ok\xc3"), Step::Pending]);
    let mut output = PREFIX.to_owned();
    let mut future = reader.read_to_string(&mut output);
    assert!(poll_once(&mut future).is_pending());
    drop(future);
    assert_eq!(output, PREFIX);
    assert_eq!(reader.consumed, 3);
}

#[test]
fn read_to_string_success_appends_and_counts_only_new_bytes() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"ok"), Step::Eof]);
    let mut output = PREFIX.to_owned();
    assert_eq!(complete(reader.read_to_string(&mut output)).unwrap(), 2);
    assert_eq!(output, format!("{PREFIX}ok"));
}

#[test]
fn read_to_string_empty_eof_preserves_existing_string() {
    let mut reader = ScriptedReader::new(&[Step::Eof]);
    let mut output = PREFIX.to_owned();
    assert_eq!(complete(reader.read_to_string(&mut output)).unwrap(), 0);
    assert_eq!(output, PREFIX);
}

#[test]
fn read_to_string_valid_partial_io_error_keeps_appended_bytes_and_primary_cause() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"ok"), Step::Error(7)]);
    let mut output = PREFIX.to_owned();
    let error = complete(reader.read_to_string(&mut output)).unwrap_err();
    assert_primary_io_error(&error, 7);
    assert_eq!(output, format!("{PREFIX}ok"));
}

#[test]
fn read_to_string_invalid_utf8_restores_prefix_and_retains_validation_error() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"ok\xff"), Step::Eof]);
    let mut output = PREFIX.to_owned();
    let error = complete(reader.read_to_string(&mut output)).unwrap_err();
    assert_eq!(output, PREFIX);
    assert_utf8_error(&error, 2, Some(1));
}

#[test]
fn read_to_string_incomplete_utf8_at_eof_retains_precise_validation_error() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"ok\xc3"), Step::Eof]);
    let mut output = PREFIX.to_owned();
    let error = complete(reader.read_to_string(&mut output)).unwrap_err();
    assert_eq!(output, PREFIX);
    assert_utf8_error(&error, 2, None);
}

#[test]
fn read_to_string_invalid_partial_bytes_do_not_replace_primary_io_error() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"ok\xff"), Step::Error(8)]);
    let mut output = PREFIX.to_owned();
    let error = complete(reader.read_to_string(&mut output)).unwrap_err();
    assert_primary_io_error(&error, 8);
    assert_eq!(output, PREFIX);
}

#[test]
fn read_to_string_immediate_io_error_preserves_existing_string_and_cause() {
    let mut reader = ScriptedReader::new(&[Step::Error(9)]);
    let mut output = PREFIX.to_owned();
    let error = complete(reader.read_to_string(&mut output)).unwrap_err();
    assert_primary_io_error(&error, 9);
    assert_eq!(output, PREFIX);
}

#[test]
fn read_line_unpolled_and_pending_cancellation_are_existing_controls() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"new"), Step::Pending]);
    let mut output = PREFIX.to_owned();
    drop(reader.read_line(&mut output));
    assert_eq!(output, PREFIX);
    assert_eq!(reader.fill_polls, 0);

    let mut future = reader.read_line(&mut output);
    assert!(poll_once(&mut future).is_pending());
    drop(future);
    assert_eq!(output, PREFIX);
    assert_eq!(reader.consumed, 3);
}

#[test]
fn read_line_success_and_valid_partial_io_error_preserve_prefix() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"ok\n"), Step::Eof]);
    let mut output = PREFIX.to_owned();
    assert_eq!(complete(reader.read_line(&mut output)).unwrap(), 3);
    assert_eq!(output, format!("{PREFIX}ok\n"));

    let mut reader = ScriptedReader::new(&[Step::Bytes(b"ok"), Step::Error(10)]);
    let mut output = PREFIX.to_owned();
    let error = complete(reader.read_line(&mut output)).unwrap_err();
    assert_primary_io_error(&error, 10);
    assert_eq!(output, format!("{PREFIX}ok"));
}

#[test]
fn read_line_invalid_utf8_restores_prefix_and_retains_validation_error() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"ok\xff\n"), Step::Eof]);
    let mut output = PREFIX.to_owned();
    let error = complete(reader.read_line(&mut output)).unwrap_err();
    assert_eq!(output, PREFIX);
    assert_utf8_error(&error, 2, Some(1));
}

#[test]
fn read_line_invalid_partial_bytes_do_not_replace_primary_io_error() {
    let mut reader = ScriptedReader::new(&[Step::Bytes(b"ok\xff"), Step::Error(11)]);
    let mut output = PREFIX.to_owned();
    let error = complete(reader.read_line(&mut output)).unwrap_err();
    assert_primary_io_error(&error, 11);
    assert_eq!(output, PREFIX);
}

#[test]
fn lines_successful_lf_crlf_and_eof_are_shared_helper_controls() {
    let reader = ScriptedReader::new(&[Step::Bytes(b"first\nsecond\r\n"), Step::Eof]);
    let mut lines = reader.lines();
    let mut cx = Context::from_waker(noop_waker_ref());
    for expected in ["first", "second"] {
        match Pin::new(&mut lines).poll_next(&mut cx) {
            Poll::Ready(Some(Ok(line))) => assert_eq!(line, expected),
            other => panic!("invariant: complete line fixture must yield its next line: {other:?}"),
        }
    }
    assert!(matches!(
        Pin::new(&mut lines).poll_next(&mut cx),
        Poll::Ready(None)
    ));
}

#[test]
fn lines_invalid_utf8_retains_validation_error_without_asserting_resume_semantics() {
    let reader = ScriptedReader::new(&[Step::Bytes(b"ok\xff\n"), Step::Eof]);
    let mut lines = reader.lines();
    match Pin::new(&mut lines).poll_next(&mut Context::from_waker(noop_waker_ref())) {
        Poll::Ready(Some(Err(error))) => assert_utf8_error(&error, 2, Some(1)),
        other => panic!("invariant: invalid line fixture must yield its UTF-8 error: {other:?}"),
    }
    // Later polling after a valid-partial I/O error has a separate pending design.
}
