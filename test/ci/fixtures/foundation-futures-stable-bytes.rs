// Stable byte ownership at the AsyncWrite-to-Sink boundary.
use futures::Sink;
use futures::io::{AsyncWrite, AsyncWriteExt};
use std::borrow::Cow;
use std::fmt;
use std::io;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::task::{Context, Poll, Wake, Waker};

const INITIAL: &[u8] = b"ABCD";

#[derive(Debug)]
struct ChangingView {
    changed: Arc<AtomicBool>,
    reads: Arc<AtomicUsize>,
    after: &'static [u8],
}

impl From<ChangingView> for Cow<'static, [u8]> {
    fn from(item: ChangingView) -> Self {
        item.reads.fetch_add(1, Ordering::Relaxed);
        Cow::Borrowed(if item.changed.load(Ordering::Relaxed) {
            item.after
        } else {
            INITIAL
        })
    }
}

#[derive(Debug)]
struct UnexpectedAdditionalPoll;

impl fmt::Display for UnexpectedAdditionalPoll {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fixture stopped an unexpected fourth write poll")
    }
}

impl std::error::Error for UnexpectedAdditionalPoll {}

#[derive(Debug, Default)]
struct Log {
    inputs: Vec<Vec<u8>>,
    output: Vec<u8>,
    flushes: usize,
}

#[derive(Debug)]
struct Writer {
    polls: usize,
    log: Arc<Mutex<Log>>,
}

impl AsyncWrite for Writer {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.polls += 1;
        let mut log = this
            .log
            .lock()
            .expect("invariant: fixture log is not poisoned");
        log.inputs.push(bytes.to_vec());
        match this.polls {
            1 => {
                assert!(
                    bytes.len() >= 2,
                    "invariant: the initial view has four bytes"
                );
                log.output.extend_from_slice(&bytes[..2]);
                Poll::Ready(Ok(2))
            }
            2 => {
                drop(log);
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            3 => {
                log.output.extend_from_slice(bytes);
                Poll::Ready(Ok(bytes.len()))
            }
            _ => Poll::Ready(Err(io::Error::other(UnexpectedAdditionalPoll))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.log
            .lock()
            .expect("invariant: fixture log is not poisoned")
            .flushes += 1;
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[derive(Debug, Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn require_send_sync_unpin<T: Send + Sync + Unpin>(_: &T) {}

fn exercise(after: &'static [u8], change_after_pending: bool) {
    let changed = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let log = Arc::new(Mutex::new(Log::default()));
    let writer = Writer {
        polls: 0,
        log: Arc::clone(&log),
    };
    let mut sink = writer.into_sink::<ChangingView>();
    require_send_sync_unpin(&sink);
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(
        Pin::new(&mut sink).poll_ready(&mut cx),
        Poll::Ready(Ok(()))
    ));
    Pin::new(&mut sink)
        .start_send(ChangingView {
            changed: Arc::clone(&changed),
            reads: Arc::clone(&reads),
            after,
        })
        .expect("invariant: the ready fixture accepts its item");
    assert!(matches!(
        Pin::new(&mut sink).poll_flush(&mut cx),
        Poll::Pending
    ));
    {
        let log = log.lock().expect("invariant: fixture log is not poisoned");
        assert_eq!(log.inputs, [b"ABCD".to_vec(), b"CD".to_vec()]);
        assert_eq!(log.output, b"AB");
        assert_eq!(log.flushes, 0);
    }
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    changed.store(change_after_pending, Ordering::Relaxed);

    assert!(matches!(
        Pin::new(&mut sink).poll_flush(&mut cx),
        Poll::Ready(Ok(()))
    ));
    let log = log.lock().expect("fixture log");
    assert_eq!(
        log.inputs,
        [b"ABCD".to_vec(), b"CD".to_vec(), b"CD".to_vec()]
    );
    assert_eq!(log.output, b"ABCD");
    assert_eq!(log.flushes, 1);
    assert_eq!(reads.load(Ordering::Relaxed), 1);
}

#[test]
fn shorter_view_after_pending_keeps_original_block() {
    exercise(b"x", true);
}
#[test]
fn offset_length_view_after_pending_does_not_truncate() {
    exercise(b"xy", true);
}
#[test]
fn same_length_view_after_pending_does_not_mix_bytes() {
    exercise(b"wxyz", true);
}
#[test]
fn unchanged_view_control_keeps_original_block() {
    exercise(b"wxyz", false);
}

#[test]
fn conversion_occurs_at_start_send_before_first_poll() {
    let changed = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let mut output = Vec::new();
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    {
        let mut sink = (&mut output).into_sink::<ChangingView>();
        assert!(matches!(
            Pin::new(&mut sink).poll_ready(&mut cx),
            Poll::Ready(Ok(()))
        ));
        assert_eq!(reads.load(Ordering::Relaxed), 0);
        changed.store(true, Ordering::Relaxed);
        Pin::new(&mut sink)
            .start_send(ChangingView {
                changed: Arc::clone(&changed),
                reads: Arc::clone(&reads),
                after: b"wxyz",
            })
            .unwrap();
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        changed.store(false, Ordering::Relaxed);
        assert!(matches!(
            Pin::new(&mut sink).poll_flush(&mut cx),
            Poll::Ready(Ok(()))
        ));
    }
    assert_eq!(output, b"wxyz");
    assert_eq!(reads.load(Ordering::Relaxed), 1);
}

struct PointerWriter {
    expected: usize,
    written: usize,
}
impl AsyncWrite for PointerWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        assert_eq!(bytes.as_ptr() as usize, self.expected + self.written);
        self.written += bytes.len();
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[test]
fn vec_is_moved_without_copying_its_allocation() {
    let bytes = vec![1, 2, 3, 4];
    let writer = PointerWriter {
        expected: bytes.as_ptr() as usize,
        written: 0,
    };
    let mut sink = writer.into_sink::<Vec<u8>>();
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        Pin::new(&mut sink).poll_ready(&mut cx),
        Poll::Ready(Ok(()))
    ));
    Pin::new(&mut sink).start_send(bytes).unwrap();
    assert!(matches!(
        Pin::new(&mut sink).poll_flush(&mut cx),
        Poll::Ready(Ok(()))
    ));
}

#[test]
fn local_slice_is_borrowed_without_copying() {
    let bytes = vec![1, 2, 3, 4];
    let writer = PointerWriter {
        expected: bytes.as_ptr() as usize,
        written: 0,
    };
    let mut sink = writer.into_sink::<&[u8]>();
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        Pin::new(&mut sink).poll_ready(&mut cx),
        Poll::Ready(Ok(()))
    ));
    Pin::new(&mut sink).start_send(bytes.as_slice()).unwrap();
    assert!(matches!(
        Pin::new(&mut sink).poll_flush(&mut cx),
        Poll::Ready(Ok(()))
    ));
    drop(sink);
    assert_eq!(bytes, [1, 2, 3, 4]);
}

#[test]
fn cow_owned_and_borrowed_items_share_one_sink() {
    let bytes = vec![3, 4];
    let mut output = Vec::new();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut sink = (&mut output).into_sink::<Cow<'_, [u8]>>();
        assert!(matches!(
            Pin::new(&mut sink).poll_ready(&mut cx),
            Poll::Ready(Ok(()))
        ));
        Pin::new(&mut sink)
            .start_send(Cow::Owned(vec![1, 2]))
            .unwrap();
        assert!(matches!(
            Pin::new(&mut sink).poll_ready(&mut cx),
            Poll::Ready(Ok(()))
        ));
        Pin::new(&mut sink)
            .start_send(Cow::Borrowed(&bytes))
            .unwrap();
        assert!(matches!(
            Pin::new(&mut sink).poll_close(&mut cx),
            Poll::Ready(Ok(()))
        ));
    }
    assert_eq!(output, [1, 2, 3, 4]);
}
