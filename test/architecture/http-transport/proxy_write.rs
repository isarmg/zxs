mod transport_write_progress_regressions {
    use http_transport::adapters::client::legacy::connect::proxy::Tunnel;
    use http_transport::rt::{Read, ReadBufCursor, Write};
    use http_transport::Uri;
    use service_stack::Service;
    use std::error::Error as StdError;
    use std::future::{ready, Future, Ready};
    use std::io;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Wake, Waker};

    const CONNECT_REQUEST: &[u8] =
        b"CONNECT target.example:443 HTTP/1.1\r\nHost: target.example:443\r\n\r\n";
    const CONNECT_RESPONSE: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";

    #[derive(Clone, Copy, Debug)]
    enum WriteMode {
        ZeroFirst,
        ZeroAfterPartial,
        Partial,
        Fail,
    }

    #[derive(Debug)]
    struct StopUnexpectedWrite;

    impl std::fmt::Display for StopUnexpectedWrite {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("fixture stopped an unexpected write after zero progress")
        }
    }

    impl StdError for StopUnexpectedWrite {}

    #[derive(Debug)]
    struct OriginalWriteCause(u32);

    impl std::fmt::Display for OriginalWriteCause {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "fixture original write cause {}", self.0)
        }
    }

    impl StdError for OriginalWriteCause {}

    #[derive(Debug)]
    struct Observed {
        mode: WriteMode,
        write_calls: usize,
        read_calls: usize,
        written: Vec<u8>,
        original_error: Option<io::Error>,
    }

    #[derive(Debug)]
    struct ScriptedIo {
        observed: Arc<Mutex<Observed>>,
        response_position: usize,
    }

    impl Read for ScriptedIo {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            mut buf: ReadBufCursor<'_>,
        ) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            this.observed
                .lock()
                .expect("invariant: finite fixture callbacks do not panic while holding the mutex")
                .read_calls += 1;
            let remaining = &CONNECT_RESPONSE[this.response_position..];
            let count = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..count]);
            this.response_position += count;
            Poll::Ready(Ok(()))
        }
    }

    impl Write for ScriptedIo {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            let mut observed = self
                .observed
                .lock()
                .expect("invariant: finite fixture callbacks do not panic while holding the mutex");
            observed.write_calls += 1;
            if buf.is_empty() {
                return Poll::Ready(Ok(0));
            }
            let count = match observed.mode {
                WriteMode::ZeroFirst => {
                    if observed.write_calls == 1 {
                        0
                    } else {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::Other,
                            StopUnexpectedWrite,
                        )));
                    }
                }
                WriteMode::ZeroAfterPartial => match observed.write_calls {
                    1 => 4.min(buf.len()),
                    2 => 0,
                    _ => {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::Other,
                            StopUnexpectedWrite,
                        )));
                    }
                },
                WriteMode::Partial => 3.min(buf.len()),
                WriteMode::Fail => {
                    return Poll::Ready(Err(observed
                        .original_error
                        .take()
                        .expect("invariant: the fixture offers its original error once")));
                }
            };
            observed.written.extend_from_slice(&buf[..count]);
            Poll::Ready(Ok(count))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[derive(Debug)]
    struct Connector(Option<ScriptedIo>);

    impl Service<Uri> for Connector {
        type Response = ScriptedIo;
        type Error = io::Error;
        type Future = Ready<io::Result<ScriptedIo>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _dst: Uri) -> Self::Future {
            ready(Ok(self.0.take().expect(
                "invariant: this one-connection fixture is called once",
            )))
        }
    }

    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    fn immediate<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(NoopWake));
        match Box::pin(future)
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
        {
            Poll::Ready(result) => result,
            Poll::Pending => {
                panic!("invariant: the fixed in-memory fixture performs no pending I/O")
            }
        }
    }

    type TunnelFailure = <Tunnel<Connector> as Service<Uri>>::Error;

    fn connect(mode: WriteMode) -> (Result<ScriptedIo, TunnelFailure>, Arc<Mutex<Observed>>) {
        let observed = Arc::new(Mutex::new(Observed {
            mode,
            write_calls: 0,
            read_calls: 0,
            written: Vec::new(),
            original_error: Some(io::Error::new(
                io::ErrorKind::BrokenPipe,
                OriginalWriteCause(17),
            )),
        }));
        let io = ScriptedIo {
            observed: Arc::clone(&observed),
            response_position: 0,
        };
        let mut tunnel = Tunnel::new(
            "http://proxy.example:8080"
                .parse()
                .expect("invariant: the fixture proxy URI is valid"),
            Connector(Some(io)),
        );
        let waker = Waker::from(Arc::new(NoopWake));
        assert!(matches!(
            tunnel.poll_ready(&mut Context::from_waker(&waker)),
            Poll::Ready(Ok(()))
        ));
        let result = immediate(
            tunnel.call(
                "https://target.example:443"
                    .parse()
                    .expect("invariant: the fixture target URI is valid"),
            ),
        );
        (result, observed)
    }

    fn io_source(error: &TunnelFailure) -> &io::Error {
        StdError::source(error)
            .expect("Tunnel must expose the original I/O error source")
            .downcast_ref::<io::Error>()
            .expect("Tunnel's source must retain the io::Error type")
    }

    #[test]
    fn zero_progress_returns_write_zero_without_polling_writer_again() {
        let (result, observed) = connect(WriteMode::ZeroFirst);
        let error =
            result.expect_err("a nonempty CONNECT request cannot finish after a zero write");
        assert_eq!(io_source(&error).kind(), io::ErrorKind::WriteZero);
        let observed = observed
            .lock()
            .expect("invariant: fixture mutex is not poisoned");
        assert_eq!(observed.write_calls, 1);
        assert_eq!(observed.read_calls, 0);
        assert!(observed.written.is_empty());
    }

    #[test]
    fn zero_after_partial_write_keeps_progress_and_returns_write_zero() {
        let (result, observed) = connect(WriteMode::ZeroAfterPartial);
        let error =
            result.expect_err("the remaining CONNECT bytes cannot finish after a zero write");
        assert_eq!(io_source(&error).kind(), io::ErrorKind::WriteZero);
        let observed = observed
            .lock()
            .expect("invariant: fixture mutex is not poisoned");
        assert_eq!(observed.write_calls, 2);
        assert_eq!(observed.read_calls, 0);
        assert_eq!(observed.written, &CONNECT_REQUEST[..4]);
    }

    #[test]
    fn partial_writes_preserve_request_bytes_before_reading_response() {
        let (result, observed) = connect(WriteMode::Partial);
        let _io = result.expect("the fixed success response must finish a fully written CONNECT");
        let observed = observed
            .lock()
            .expect("invariant: fixture mutex is not poisoned");
        assert_eq!(observed.written, CONNECT_REQUEST);
        assert_eq!(observed.write_calls, (CONNECT_REQUEST.len() + 2) / 3);
        assert_eq!(observed.read_calls, 1);
    }

    #[test]
    fn existing_write_error_retains_kind_and_concrete_source() {
        let (result, observed) = connect(WriteMode::Fail);
        let error = result.expect_err("the scripted original I/O error must propagate");
        let io = io_source(&error);
        assert_eq!(io.kind(), io::ErrorKind::BrokenPipe);
        let original = io
            .get_ref()
            .expect("the fixture I/O error owns an original cause")
            .downcast_ref::<OriginalWriteCause>()
            .expect("the original concrete write cause must survive unchanged");
        assert_eq!(original.0, 17);
        let observed = observed
            .lock()
            .expect("invariant: fixture mutex is not poisoned");
        assert_eq!(observed.write_calls, 1);
        assert_eq!(observed.read_calls, 0);
        assert!(observed.written.is_empty());
    }
}
