// Project-authored public-API regressions for the HTTP/1 shutdown backport.
// These run without tokio-test, private module copies, sockets, or timing sleeps.
mod backport_shutdown {
    use http_body::Empty;
    use http_transport::{
        Request, StatusCode,
        adapters::rt::TokioIo,
        body::Bytes,
        client::conn::http1::{self, Connection},
    };
    use std::{
        cell::{Cell, RefCell},
        error::Error as _,
        future::Future,
        io,
        pin::Pin,
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        task::{Context, Poll, Wake, Waker},
    };
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n";
    const UPGRADE: &[u8] = b"HTTP/1.1 101 Switching Protocols\r\nconnection: upgrade\r\nupgrade: test-stream\r\n\r\nearly";

    #[derive(Clone, Copy)]
    enum Close {
        Reset,
        Eof,
    }

    #[derive(Clone, Copy)]
    enum Shutdown {
        Transport,
        KeepOpen,
    }

    #[derive(Clone, Copy)]
    enum ReadStep {
        Pending,
        Bytes(&'static [u8]),
        Close(Close),
        Closed,
    }

    struct Control {
        read: Cell<ReadStep>,
        written: RefCell<Vec<u8>>,
        shutdowns: Cell<usize>,
    }

    struct ScriptedIo(Rc<Control>);

    impl AsyncRead for ScriptedIo {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            // The test drives every state transition and the corresponding
            // connection poll directly. Unlike Tokio IO, this consumes no
            // cooperative budget before the dispatch channel is polled.
            match self.0.read.get() {
                ReadStep::Pending => Poll::Pending,
                ReadStep::Bytes(bytes) => {
                    let count = bytes.len().min(buf.remaining());
                    buf.put_slice(&bytes[..count]);
                    self.0.read.set(if count == bytes.len() {
                        ReadStep::Pending
                    } else {
                        ReadStep::Bytes(&bytes[count..])
                    });
                    Poll::Ready(Ok(()))
                }
                ReadStep::Close(close) => {
                    self.0.read.set(ReadStep::Closed);
                    Poll::Ready(match close {
                        Close::Reset => Err(io::Error::new(
                            io::ErrorKind::ConnectionReset,
                            "scripted connection reset",
                        )),
                        Close::Eof => Ok(()),
                    })
                }
                ReadStep::Closed => panic!("completed IO must not be polled while draining"),
            }
        }
    }

    impl AsyncWrite for ScriptedIo {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.0.written.borrow_mut().extend_from_slice(bytes);
            Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.0.shutdowns.set(self.0.shutdowns.get() + 1);
            Poll::Ready(Ok(()))
        }
    }

    struct WakeObserved(AtomicBool);

    impl Wake for WakeObserved {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    type ClientConnection = Connection<TokioIo<ScriptedIo>, Empty<Bytes>>;

    fn poll_connection(
        connection: &mut ClientConnection,
        shutdown: Shutdown,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), http_transport::Error>> {
        match shutdown {
            Shutdown::Transport => Pin::new(connection).poll(cx),
            Shutdown::KeepOpen => connection.poll_without_shutdown(cx),
        }
    }

    fn control() -> Rc<Control> {
        Rc::new(Control {
            read: Cell::new(ReadStep::Pending),
            written: RefCell::new(Vec::new()),
            shutdowns: Cell::new(0),
        })
    }

    fn request(path: &str) -> Request<Empty<Bytes>> {
        Request::builder()
            .uri(path)
            .header("host", "local.test")
            .body(Empty::new())
            .unwrap()
    }

    fn exhaust_cooperative_budget() {
        // A different waker ensures the observed connection wake cannot come
        // from exhausting this unrelated channel. Bound the loop so disabling
        // cooperative scheduling fails rather than hanging the test.
        let mut cx = Context::from_waker(Waker::noop());
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        for _ in 0..1024 {
            sender.send(()).unwrap();
            if receiver.poll_recv(&mut cx).is_pending() {
                return;
            }
        }
        panic!("the pinned Tokio runtime must enforce a cooperative budget");
    }

    async fn drain_queued_request(close: Close, shutdown: Shutdown) {
        let control = control();
        let (mut sender, mut connection) =
            http1::handshake(TokioIo::new(ScriptedIo(Rc::clone(&control))))
                .await
                .unwrap();
        let observed = Arc::new(WakeObserved(AtomicBool::new(false)));
        let waker = Waker::from(Arc::clone(&observed));
        let mut cx = Context::from_waker(&waker);

        // Establish idle keep-alive through the public request API before
        // making the peer close with a second request already queued.
        let mut first = Box::pin(sender.try_send_request(request("/warmup")));
        assert!(poll_connection(&mut connection, shutdown, &mut cx).is_pending());
        control.read.set(ReadStep::Bytes(RESPONSE));
        assert!(poll_connection(&mut connection, shutdown, &mut cx).is_pending());
        let Poll::Ready(Ok(response)) = first.as_mut().poll(&mut cx) else {
            panic!("warmup response must be delivered")
        };
        assert_eq!(response.status(), StatusCode::OK);
        drop(response);
        assert!(sender.is_ready());
        let warmup_bytes = control.written.borrow().len();
        assert!(warmup_bytes > 0);

        control.read.set(ReadStep::Close(close));
        let mut queued = Box::pin(sender.try_send_request(request("/not-sent")));
        exhaust_cooperative_budget();
        observed.0.store(false, Ordering::SeqCst);
        assert!(
            poll_connection(&mut connection, shutdown, &mut cx).is_pending(),
            "connection must wait for cancellation drain before completing"
        );
        assert!(sender.is_closed(), "draining must reject new requests");

        tokio::task::yield_now().await;
        assert!(
            observed.0.load(Ordering::SeqCst),
            "pending drain must arrange another connection poll"
        );
        let Poll::Ready(result) = poll_connection(&mut connection, shutdown, &mut cx) else {
            panic!("drain must finish after cooperative budget is replenished")
        };
        match close {
            Close::Reset => {
                let error = result.expect_err("original reset must remain the connection result");
                let mut source = error.source();
                let reset = loop {
                    let current = source.expect("reset must retain its original IO source");
                    if let Some(reset) = current.downcast_ref::<io::Error>() {
                        break reset;
                    }
                    source = current.source();
                };
                assert_eq!(reset.kind(), io::ErrorKind::ConnectionReset);
                assert_eq!(reset.to_string(), "scripted connection reset");
            }
            Close::Eof => result.expect("idle EOF must remain a normal shutdown"),
        }

        // Neither destructor may be responsible for canceling this request.
        let Poll::Ready(Err(mut canceled)) = queued.as_mut().poll(&mut cx) else {
            panic!("queued request must be canceled while connection and sender are alive")
        };
        assert!(canceled.error().is_canceled());
        assert_eq!(canceled.take_message().unwrap().uri(), "/not-sent");
        assert!(sender.is_closed());
        assert_eq!(control.written.borrow().len(), warmup_bytes);
        let shutdown_calls = match (close, shutdown) {
            (Close::Eof, Shutdown::Transport) => 1,
            (Close::Reset, _) | (Close::Eof, Shutdown::KeepOpen) => 0,
        };
        assert_eq!(control.shutdowns.get(), shutdown_calls);
        drop(connection);
        drop(sender);
    }

    #[tokio::test]
    async fn reset_drains_queued_request_before_connection_completion() {
        drain_queued_request(Close::Reset, Shutdown::Transport).await;
    }

    #[tokio::test]
    async fn eof_drains_queued_request_before_connection_completion() {
        drain_queued_request(Close::Eof, Shutdown::Transport).await;
    }

    #[tokio::test]
    async fn eof_without_shutdown_drains_queued_request() {
        drain_queued_request(Close::Eof, Shutdown::KeepOpen).await;
    }

    #[tokio::test]
    async fn reset_without_shutdown_drains_queued_request() {
        drain_queued_request(Close::Reset, Shutdown::KeepOpen).await;
    }

    #[tokio::test]
    async fn upgrade_preserves_sender_readiness_until_connection_is_reclaimed() {
        for shutdown in [Shutdown::Transport, Shutdown::KeepOpen] {
            let control = control();
            let (mut sender, mut connection) =
                http1::handshake(TokioIo::new(ScriptedIo(Rc::clone(&control))))
                    .await
                    .unwrap();
            let mut cx = Context::from_waker(Waker::noop());
            let mut upgrade = request("/upgrade");
            upgrade
                .headers_mut()
                .insert("connection", "upgrade".parse().unwrap());
            upgrade
                .headers_mut()
                .insert("upgrade", "test-stream".parse().unwrap());
            let mut response = Box::pin(sender.try_send_request(upgrade));
            assert!(poll_connection(&mut connection, shutdown, &mut cx).is_pending());
            assert!(sender.poll_ready(&mut cx).is_pending());
            let readiness_before_upgrade = sender.is_ready();
            control.read.set(ReadStep::Bytes(UPGRADE));
            assert!(matches!(
                poll_connection(&mut connection, shutdown, &mut cx),
                Poll::Ready(Ok(()))
            ));
            let Poll::Ready(Ok(response)) = response.as_mut().poll(&mut cx) else {
                panic!("upgrade response must be delivered")
            };
            assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
            assert!(
                !sender.is_closed(),
                "upgrade is not a request-channel shutdown"
            );
            // The upgrade is not reusable HTTP capacity. Preserve pending
            // readiness, rather than converting it to a closed-channel error.
            assert_eq!(sender.is_ready(), readiness_before_upgrade);
            assert!(sender.poll_ready(&mut cx).is_pending());
            assert_eq!(control.shutdowns.get(), 0);
            let parts = connection.into_parts();
            assert_eq!(parts.read_buf.as_ref(), b"early");
            assert!(
                sender.is_closed(),
                "reclaiming the connection drops its dispatcher"
            );
        }
    }
}
