// Public buffered-seek boundaries using fixed bytes and virtual u64 positions.
use futures::io::{AsyncBufRead, AsyncRead, AsyncSeek, BufReader, Cursor, SeekFrom};
use std::fmt;
use std::future::Future;
use std::io;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

const WINDOW: &[u8; 8] = b"abcdefgh";
const LARGE_POSITION: u64 = 1_u64 << 63;

#[derive(Debug, PartialEq, Eq)]
struct SeekFailure {
    poll: usize,
}

impl fmt::Display for SeekFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "injected seek failure on poll {}", self.poll)
    }
}

impl std::error::Error for SeekFailure {}

struct VirtualReader {
    window_start: u64,
    position: u64,
    reads: usize,
    requests: Vec<SeekFrom>,
    pending_on: Option<usize>,
    error_on: Option<usize>,
}

impl VirtualReader {
    fn new(window_start: u64) -> Self {
        assert!(window_start <= u64::MAX - WINDOW.len() as u64);
        Self {
            window_start,
            position: window_start,
            reads: 0,
            requests: Vec::new(),
            pending_on: None,
            error_on: None,
        }
    }
}

impl AsyncRead for VirtualReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        output: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.reads += 1;
        let Some(relative) = self.position.checked_sub(self.window_start) else {
            return Poll::Ready(Ok(0));
        };
        if relative >= WINDOW.len() as u64 {
            return Poll::Ready(Ok(0));
        }
        // The window bound makes this conversion valid on both supported widths.
        let index = relative as usize;
        let count = output.len().min(WINDOW.len() - index);
        output[..count].copy_from_slice(&WINDOW[index..index + count]);
        self.position += count as u64;
        Poll::Ready(Ok(count))
    }
}

impl AsyncSeek for VirtualReader {
    fn poll_seek(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        request: SeekFrom,
    ) -> Poll<io::Result<u64>> {
        self.requests.push(request);
        let poll = self.requests.len();
        if self.pending_on == Some(poll) {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        if self.error_on == Some(poll) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                SeekFailure { poll },
            )));
        }

        // A widened mathematical position avoids reproducing the production
        // signed-magnitude calculation or allocating up to a virtual position.
        let position = match request {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => {
                i128::from(self.window_start) + WINDOW.len() as i128 + i128::from(offset)
            }
        };
        match u64::try_from(position) {
            Ok(position) => {
                self.position = position;
                Poll::Ready(Ok(position))
            }
            Err(_) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "virtual seek target is outside the u64 position range",
            ))),
        }
    }
}

fn buffered(window_start: u64, consumed: usize) -> BufReader<VirtualReader> {
    let mut reader = BufReader::with_capacity(WINDOW.len(), VirtualReader::new(window_start));
    match Pin::new(&mut reader).poll_fill_buf(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(bytes)) => assert_eq!(bytes, WINDOW),
        result => panic!("fixed fixture window must be immediately readable: {result:?}"),
    }
    Pin::new(&mut reader).consume(consumed);
    assert_eq!(reader.buffer(), &WINDOW[consumed..]);
    reader
}

fn ready_seek<R: AsyncRead + AsyncSeek + Unpin>(
    reader: &mut BufReader<R>,
    offset: i64,
) -> io::Result<()> {
    let mut future = pin!(Pin::new(reader).seek_relative(offset));
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("this fixture seek has no pending step"),
    }
}

fn assert_injected_error(error: io::Error, poll: usize) {
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    let payload = error.get_ref().expect("injected error payload is retained");
    assert_eq!(
        payload.downcast_ref::<SeekFailure>(),
        Some(&SeekFailure { poll })
    );
}

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn pending_then_ready(reader: &mut BufReader<VirtualReader>, offset: i64) {
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(Pin::new(reader).seek_relative(offset));
    assert!(future.as_mut().poll(&mut cx).is_pending());
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    let result = future.as_mut().poll(&mut cx);
    assert!(matches!(result, Poll::Ready(Ok(()))), "{result:?}");
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
}

#[test]
fn min_offset_from_zero_returns_cursor_invalid_input() {
    let cursor = Cursor::new(&WINDOW[..]);
    let mut reader = BufReader::with_capacity(WINDOW.len(), cursor);
    let error = ready_seek(&mut reader, i64::MIN).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(reader.get_ref().position(), 0);
    assert!(reader.buffer().is_empty());
}

#[test]
fn min_offset_from_large_cursor_reaches_zero_without_allocation() {
    let mut cursor = Cursor::new(&WINDOW[..]);
    cursor.set_position(LARGE_POSITION);
    let mut reader = BufReader::with_capacity(WINDOW.len(), cursor);
    ready_seek(&mut reader, i64::MIN).unwrap();
    assert_eq!(reader.get_ref().position(), 0);
    assert!(reader.buffer().is_empty());
}

#[test]
fn min_offset_with_unread_buffer_aligns_then_seeks() {
    let mut reader = buffered(LARGE_POSITION, 3);
    let result =
        Pin::new(&mut reader).poll_seek_relative(&mut Context::from_waker(Waker::noop()), i64::MIN);
    assert!(matches!(result, Poll::Ready(Ok(()))), "{result:?}");
    assert_eq!(reader.get_ref().position, 3);
    assert_eq!(
        reader.get_ref().requests,
        [SeekFrom::Current(-5), SeekFrom::Current(i64::MIN)]
    );
    assert_eq!(reader.get_ref().reads, 1);
    assert!(reader.buffer().is_empty());
}

#[test]
fn min_offset_with_consumed_buffer_delegates_once() {
    let mut reader = buffered(LARGE_POSITION, WINDOW.len());
    ready_seek(&mut reader, i64::MIN).unwrap();
    assert_eq!(reader.get_ref().position, WINDOW.len() as u64);
    assert_eq!(reader.get_ref().requests, [SeekFrom::Current(i64::MIN)]);
    assert!(reader.buffer().is_empty());
}

#[test]
fn positive_negative_and_zero_offsets_within_buffer_avoid_inner_seek() {
    let mut reader = buffered(0, 3);
    ready_seek(&mut reader, -2).unwrap();
    assert_eq!(reader.buffer(), &WINDOW[1..]);
    ready_seek(&mut reader, 4).unwrap();
    assert_eq!(reader.buffer(), &WINDOW[5..]);
    ready_seek(&mut reader, 0).unwrap();
    assert_eq!(reader.buffer(), &WINDOW[5..]);
    ready_seek(&mut reader, 3).unwrap();
    assert!(reader.buffer().is_empty());
    ready_seek(&mut reader, -8).unwrap();
    assert_eq!(reader.buffer(), WINDOW);
    assert_eq!(reader.get_ref().position, WINDOW.len() as u64);
    assert_eq!(reader.get_ref().reads, 1);
    assert!(reader.get_ref().requests.is_empty());
}

#[test]
fn offsets_outside_buffer_delegate_adjusted_current_positions() {
    for (offset, adjusted, expected) in [(6, 1, 29), (-4, -9, 19)] {
        let mut reader = buffered(20, 3);
        ready_seek(&mut reader, offset).unwrap();
        assert_eq!(reader.get_ref().position, expected);
        assert_eq!(reader.get_ref().requests, [SeekFrom::Current(adjusted)]);
        assert!(reader.buffer().is_empty());
    }
}

#[test]
fn offsets_larger_than_u32_keep_their_full_width() {
    let wide = 1_i64 << 32;
    for (start, offset, adjusted, expected) in [
        (0, wide, wide - 5, (1_u64 << 32) + 3),
        (1_u64 << 32, -wide, -wide - 5, 3),
    ] {
        let mut reader = buffered(start, 3);
        ready_seek(&mut reader, offset).unwrap();
        assert_eq!(reader.get_ref().position, expected);
        assert_eq!(reader.get_ref().requests, [SeekFrom::Current(adjusted)]);
        assert!(reader.buffer().is_empty());
    }
}

#[test]
fn max_positive_offset_allows_a_u64_position_above_i64_max() {
    let mut reader = buffered(0, 3);
    ready_seek(&mut reader, i64::MAX).unwrap();
    assert_eq!(reader.get_ref().position, i64::MAX as u64 + 3);
    assert_eq!(reader.get_ref().requests, [SeekFrom::Current(i64::MAX - 5)]);
    assert!(reader.buffer().is_empty());
}

#[test]
fn ordinary_underlying_error_retains_payload_and_buffer() {
    let mut reader = buffered(20, 3);
    reader.get_mut().error_on = Some(1);
    assert_injected_error(ready_seek(&mut reader, 64).unwrap_err(), 1);
    assert_eq!(reader.get_ref().position, 28);
    assert_eq!(reader.get_ref().requests, [SeekFrom::Current(59)]);
    assert_eq!(reader.buffer(), &WINDOW[3..]);
}

#[test]
fn min_alignment_error_retains_payload_and_unread_buffer() {
    let mut reader = buffered(LARGE_POSITION, 3);
    reader.get_mut().error_on = Some(1);
    assert_injected_error(ready_seek(&mut reader, i64::MIN).unwrap_err(), 1);
    assert_eq!(reader.get_ref().position, LARGE_POSITION + 8);
    assert_eq!(reader.get_ref().requests, [SeekFrom::Current(-5)]);
    assert_eq!(reader.buffer(), &WINDOW[3..]);
}

#[test]
fn min_target_error_retains_payload_after_completed_alignment() {
    let mut reader = buffered(LARGE_POSITION, 3);
    reader.get_mut().error_on = Some(2);
    assert_injected_error(ready_seek(&mut reader, i64::MIN).unwrap_err(), 2);
    assert_eq!(reader.get_ref().position, LARGE_POSITION + 3);
    assert_eq!(
        reader.get_ref().requests,
        [SeekFrom::Current(-5), SeekFrom::Current(i64::MIN)]
    );
    assert!(reader.buffer().is_empty());
}

#[test]
fn invalid_min_target_leaves_the_documented_aligned_position() {
    let mut reader = buffered(0, 3);
    let error = ready_seek(&mut reader, i64::MIN).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(reader.get_ref().position, 3);
    assert_eq!(
        reader.get_ref().requests,
        [SeekFrom::Current(-5), SeekFrom::Current(i64::MIN)]
    );
    assert!(reader.buffer().is_empty());
}

#[test]
fn min_offset_resumes_each_pending_stage_without_repeating_completed_seek() {
    for pending_on in [1, 2] {
        let mut reader = buffered(LARGE_POSITION, 3);
        reader.get_mut().pending_on = Some(pending_on);
        pending_then_ready(&mut reader, i64::MIN);
        let expected = if pending_on == 1 {
            [
                SeekFrom::Current(-5),
                SeekFrom::Current(-5),
                SeekFrom::Current(i64::MIN),
            ]
        } else {
            [
                SeekFrom::Current(-5),
                SeekFrom::Current(i64::MIN),
                SeekFrom::Current(i64::MIN),
            ]
        };
        assert_eq!(reader.get_ref().requests, expected);
        assert_eq!(reader.get_ref().position, 3);
        assert_eq!(reader.get_ref().reads, 1);
        assert!(reader.buffer().is_empty());
    }
}

#[test]
fn ordinary_outside_seek_resumes_pending_once() {
    let mut reader = buffered(20, 3);
    reader.get_mut().pending_on = Some(1);
    pending_then_ready(&mut reader, 6);
    assert_eq!(reader.get_ref().position, 29);
    assert_eq!(
        reader.get_ref().requests,
        [SeekFrom::Current(1), SeekFrom::Current(1)]
    );
    assert!(reader.buffer().is_empty());
}
