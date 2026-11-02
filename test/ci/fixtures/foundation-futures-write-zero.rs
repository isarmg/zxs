// Public AsyncWrite-to-Sink progress and error propagation contracts.
mod into_sink_write_zero_contracts {
    use futures::Sink;
    use futures::io::{AsyncWrite, AsyncWriteExt, IntoSink};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::error::Error;
    use std::fmt;
    use std::io;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::task::{Context, Poll, Wake, Waker};

    #[derive(Debug)]
    struct RepollAfterWriteZero;

    impl fmt::Display for RepollAfterWriteZero {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("fixture stopped a second write after zero progress")
        }
    }

    impl Error for RepollAfterWriteZero {}

    #[derive(Debug)]
    struct ScriptExhausted(&'static str);

    impl fmt::Display for ScriptExhausted {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "fixture exhausted the {} script", self.0)
        }
    }

    impl Error for ScriptExhausted {}

    #[derive(Debug)]
    struct FixtureCause(&'static str);

    impl fmt::Display for FixtureCause {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    impl Error for FixtureCause {}

    #[derive(Debug)]
    struct FixtureIoFailure {
        phase: &'static str,
        cause: Arc<FixtureCause>,
    }

    impl fmt::Display for FixtureIoFailure {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "fixture {} failure", self.phase)
        }
    }

    impl Error for FixtureIoFailure {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(self.cause.as_ref())
        }
    }

    #[derive(Debug)]
    enum WriteStep {
        Take(usize),
        Remainder,
        Pending,
        Error(io::Error),
    }

    #[derive(Debug)]
    enum FinishStep {
        Ready,
        Pending,
        Error(io::Error),
    }

    #[derive(Debug, Default)]
    struct WriteLog {
        inputs: Vec<Vec<u8>>,
        output: Vec<u8>,
        flush_output: Vec<Vec<u8>>,
        close_output: Vec<Vec<u8>>,
    }

    #[derive(Debug)]
    struct ScriptedWriter {
        writes: VecDeque<WriteStep>,
        flushes: VecDeque<FinishStep>,
        closes: VecDeque<FinishStep>,
        log: Rc<RefCell<WriteLog>>,
    }

    impl AsyncWrite for ScriptedWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            let this = self.get_mut();
            this.log.borrow_mut().inputs.push(bytes.to_vec());
            match this.writes.pop_front() {
                Some(WriteStep::Take(count)) => {
                    assert!(
                        count <= bytes.len(),
                        "invariant: the fixture never writes beyond the supplied buffer"
                    );
                    this.log
                        .borrow_mut()
                        .output
                        .extend_from_slice(&bytes[..count]);
                    Poll::Ready(Ok(count))
                }
                Some(WriteStep::Remainder) => {
                    this.log.borrow_mut().output.extend_from_slice(bytes);
                    Poll::Ready(Ok(bytes.len()))
                }
                Some(WriteStep::Pending) => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                Some(WriteStep::Error(error)) => Poll::Ready(Err(error)),
                None => Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::Other,
                    ScriptExhausted("write"),
                ))),
            }
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            let mut log = this.log.borrow_mut();
            let output = log.output.clone();
            log.flush_output.push(output);
            drop(log);
            poll_finish(&mut this.flushes, "flush", cx)
        }

        fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            let mut log = this.log.borrow_mut();
            let output = log.output.clone();
            log.close_output.push(output);
            drop(log);
            poll_finish(&mut this.closes, "close", cx)
        }
    }

    fn poll_finish(
        steps: &mut VecDeque<FinishStep>,
        phase: &'static str,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        match steps.pop_front() {
            Some(FinishStep::Ready) => Poll::Ready(Ok(())),
            Some(FinishStep::Pending) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Some(FinishStep::Error(error)) => Poll::Ready(Err(error)),
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::Other,
                ScriptExhausted(phase),
            ))),
        }
    }

    type WriterSink = IntoSink<ScriptedWriter, Vec<u8>>;

    fn writer(steps: Vec<WriteStep>) -> (ScriptedWriter, Rc<RefCell<WriteLog>>) {
        let log = Rc::new(RefCell::new(WriteLog::default()));
        (
            ScriptedWriter {
                writes: steps.into(),
                flushes: VecDeque::from([FinishStep::Ready]),
                closes: VecDeque::from([FinishStep::Ready]),
                log: Rc::clone(&log),
            },
            log,
        )
    }

    fn queue(mut sink: Pin<&mut WriterSink>, cx: &mut Context<'_>, bytes: &[u8]) {
        assert!(matches!(sink.as_mut().poll_ready(cx), Poll::Ready(Ok(()))));
        sink.start_send(bytes.to_vec())
            .expect("invariant: a ready fixture sink accepts the queued item");
    }

    fn ready_error(poll: Poll<io::Result<()>>) -> io::Error {
        match poll {
            Poll::Ready(Err(error)) => error,
            other => panic!("expected one sink poll to finish with an error, got {other:?}"),
        }
    }

    fn noop_context() -> Context<'static> {
        Context::from_waker(futures::task::noop_waker_ref())
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

    fn source_error(
        phase: &'static str,
        kind: io::ErrorKind,
        cause: &Arc<FixtureCause>,
    ) -> io::Error {
        io::Error::new(
            kind,
            FixtureIoFailure {
                phase,
                cause: Arc::clone(cause),
            },
        )
    }

    fn assert_original_source(error: &io::Error, cause: &Arc<FixtureCause>) {
        let original = error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<FixtureIoFailure>())
            .expect("invariant: the original typed writer error is retained");
        assert!(Arc::ptr_eq(&original.cause, cause));
        let retained_cause = original
            .source()
            .and_then(|source| source.downcast_ref::<FixtureCause>())
            .expect("invariant: the writer error retains its concrete cause");
        assert!(std::ptr::eq(retained_cause, cause.as_ref()));
        assert!(error.source().is_some());
    }

    #[derive(Debug, Clone, Copy)]
    enum Drain {
        Ready,
        Flush,
        Close,
    }

    #[test]
    fn nonempty_zero_write_finishes_each_first_sink_poll_with_writezero() {
        for drain in [Drain::Ready, Drain::Flush, Drain::Close] {
            let (writer, log) = writer(vec![
                WriteStep::Take(0),
                WriteStep::Error(io::Error::new(io::ErrorKind::Other, RepollAfterWriteZero)),
            ]);
            let mut sink = writer.into_sink::<Vec<u8>>();
            let mut cx = noop_context();
            queue(Pin::new(&mut sink), &mut cx, b"abc");
            let polled = match drain {
                Drain::Ready => Pin::new(&mut sink).poll_ready(&mut cx),
                Drain::Flush => Pin::new(&mut sink).poll_flush(&mut cx),
                Drain::Close => Pin::new(&mut sink).poll_close(&mut cx),
            };
            let error = ready_error(polled);
            assert_eq!(error.kind(), io::ErrorKind::WriteZero, "{drain:?}: {error}");
            let log = log.borrow();
            assert_eq!(log.inputs, [b"abc".to_vec()]);
            assert!(log.output.is_empty());
            assert!(log.flush_output.is_empty());
            assert!(log.close_output.is_empty());
        }
    }

    #[test]
    fn empty_item_zero_write_remains_successful() {
        let (writer, log) = writer(vec![WriteStep::Take(0)]);
        let mut sink = writer.into_sink::<Vec<u8>>();
        let mut cx = noop_context();
        queue(Pin::new(&mut sink), &mut cx, b"");
        assert!(matches!(
            Pin::new(&mut sink).poll_flush(&mut cx),
            Poll::Ready(Ok(()))
        ));
        assert!(log.borrow().output.is_empty());
        assert_eq!(log.borrow().flush_output, [Vec::<u8>::new()]);
    }

    #[test]
    fn partial_writes_keep_byte_order_before_flush() {
        let (writer, log) = writer(vec![
            WriteStep::Take(2),
            WriteStep::Take(2),
            WriteStep::Remainder,
        ]);
        let mut sink = writer.into_sink::<Vec<u8>>();
        let mut cx = noop_context();
        queue(Pin::new(&mut sink), &mut cx, b"abcde");
        assert!(matches!(
            Pin::new(&mut sink).poll_flush(&mut cx),
            Poll::Ready(Ok(()))
        ));
        let log = log.borrow();
        assert_eq!(
            log.inputs,
            [b"abcde".to_vec(), b"cde".to_vec(), b"e".to_vec()]
        );
        assert_eq!(log.output, b"abcde");
        assert_eq!(log.flush_output, [b"abcde".to_vec()]);
    }

    #[test]
    fn pending_write_retains_the_partial_offset_and_wakes() {
        let (writer, log) = writer(vec![
            WriteStep::Take(2),
            WriteStep::Pending,
            WriteStep::Remainder,
        ]);
        let mut sink = writer.into_sink::<Vec<u8>>();
        let wakes = Arc::new(WakeCount::default());
        let waker = Waker::from(Arc::clone(&wakes));
        let mut cx = Context::from_waker(&waker);
        queue(Pin::new(&mut sink), &mut cx, b"abcde");
        assert!(matches!(
            Pin::new(&mut sink).poll_flush(&mut cx),
            Poll::Pending
        ));
        assert_eq!(log.borrow().output, b"ab");
        assert!(log.borrow().flush_output.is_empty());
        assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
        assert!(matches!(
            Pin::new(&mut sink).poll_flush(&mut cx),
            Poll::Ready(Ok(()))
        ));
        let log = log.borrow();
        assert_eq!(
            log.inputs,
            [b"abcde".to_vec(), b"cde".to_vec(), b"cde".to_vec()]
        );
        assert_eq!(log.output, b"abcde");
        assert_eq!(log.flush_output, [b"abcde".to_vec()]);
    }

    #[test]
    fn pending_flush_does_not_rewrite_the_completed_item() {
        let (mut writer, log) = writer(vec![WriteStep::Remainder]);
        writer.flushes = VecDeque::from([FinishStep::Pending, FinishStep::Ready]);
        let mut sink = writer.into_sink::<Vec<u8>>();
        let wakes = Arc::new(WakeCount::default());
        let waker = Waker::from(Arc::clone(&wakes));
        let mut cx = Context::from_waker(&waker);
        queue(Pin::new(&mut sink), &mut cx, b"abc");
        assert!(matches!(
            Pin::new(&mut sink).poll_flush(&mut cx),
            Poll::Pending
        ));
        assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
        assert!(matches!(
            Pin::new(&mut sink).poll_flush(&mut cx),
            Poll::Ready(Ok(()))
        ));
        let log = log.borrow();
        assert_eq!(log.inputs, [b"abc".to_vec()]);
        assert_eq!(log.output, b"abc");
        assert_eq!(log.flush_output, [b"abc".to_vec(), b"abc".to_vec()]);
    }

    #[test]
    fn original_write_error_keeps_its_kind_payload_and_source() {
        let cause = Arc::new(FixtureCause("original write cause"));
        let (writer, log) = writer(vec![WriteStep::Error(source_error(
            "write",
            io::ErrorKind::BrokenPipe,
            &cause,
        ))]);
        let mut sink = writer.into_sink::<Vec<u8>>();
        let mut cx = noop_context();
        queue(Pin::new(&mut sink), &mut cx, b"abc");
        let error = ready_error(Pin::new(&mut sink).poll_flush(&mut cx));
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_original_source(&error, &cause);
        assert_eq!(log.borrow().inputs, [b"abc".to_vec()]);
        assert!(log.borrow().flush_output.is_empty());
    }

    #[test]
    fn original_write_error_without_source_stays_unwrapped() {
        let (writer, log) = writer(vec![WriteStep::Error(io::Error::from(
            io::ErrorKind::ConnectionReset,
        ))]);
        let mut sink = writer.into_sink::<Vec<u8>>();
        let mut cx = noop_context();
        queue(Pin::new(&mut sink), &mut cx, b"abc");
        let error = ready_error(Pin::new(&mut sink).poll_flush(&mut cx));
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
        assert!(error.get_ref().is_none());
        assert!(error.source().is_none());
        assert!(log.borrow().flush_output.is_empty());
    }

    #[test]
    fn original_flush_error_keeps_its_kind_payload_and_source() {
        let cause = Arc::new(FixtureCause("original flush cause"));
        let (mut writer, log) = writer(vec![WriteStep::Remainder]);
        writer.flushes = VecDeque::from([FinishStep::Error(source_error(
            "flush",
            io::ErrorKind::PermissionDenied,
            &cause,
        ))]);
        let mut sink = writer.into_sink::<Vec<u8>>();
        let mut cx = noop_context();
        queue(Pin::new(&mut sink), &mut cx, b"abc");
        let error = ready_error(Pin::new(&mut sink).poll_flush(&mut cx));
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_original_source(&error, &cause);
        assert_eq!(log.borrow().output, b"abc");
        assert_eq!(log.borrow().flush_output, [b"abc".to_vec()]);
    }

    #[test]
    fn close_drains_partial_writes_before_delegating_to_the_writer() {
        let (writer, log) = writer(vec![WriteStep::Take(2), WriteStep::Remainder]);
        let mut sink = writer.into_sink::<Vec<u8>>();
        let mut cx = noop_context();
        queue(Pin::new(&mut sink), &mut cx, b"abcde");
        assert!(matches!(
            Pin::new(&mut sink).poll_close(&mut cx),
            Poll::Ready(Ok(()))
        ));
        let log = log.borrow();
        assert_eq!(log.output, b"abcde");
        assert_eq!(log.close_output, [b"abcde".to_vec()]);
    }
}
