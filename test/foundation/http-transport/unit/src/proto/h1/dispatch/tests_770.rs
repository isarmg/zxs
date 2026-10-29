#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::io::Compat;
    use crate::proto::h1::ClientTransaction;
    use std::time::Duration;

    #[cfg(not(miri))]
    #[tokio::test]
    async fn client_close_drains_pending_requests_on_reset() {
        client_close_drains_pending_requests(true, true).await;
    }

    #[cfg(not(miri))]
    #[tokio::test]
    async fn client_close_drains_pending_requests_on_eof() {
        client_close_drains_pending_requests(false, true).await;
    }

    #[cfg(not(miri))]
    #[tokio::test]
    async fn client_close_drains_pending_requests_without_shutdown() {
        client_close_drains_pending_requests(false, false).await;
    }

    #[cfg(not(miri))]
    async fn client_close_drains_pending_requests(reset: bool, should_shutdown: bool) {
        use std::{cell::Cell, io, rc::Rc};

        #[derive(Clone, Copy)]
        enum ReadState {
            Pending,
            Response,
            Close,
            Done,
        }

        // Unlike Tokio IO, this reader does not consume cooperative budget, so
        // the close is observed while the dispatch channel is forced to yield.
        struct ClosingReader {
            state: Rc<Cell<ReadState>>,
            reset: bool,
        }

        impl tokio::io::AsyncRead for ClosingReader {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                match self.state.get() {
                    ReadState::Pending => Poll::Pending,
                    ReadState::Response => {
                        buf.put_slice(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n");
                        self.state.set(ReadState::Pending);
                        Poll::Ready(Ok(()))
                    }
                    ReadState::Close => {
                        self.state.set(ReadState::Done);
                        if self.reset {
                            Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::ConnectionReset,
                                "reset",
                            )))
                        } else {
                            Poll::Ready(Ok(()))
                        }
                    }
                    ReadState::Done => panic!("closed IO must not be polled again"),
                }
            }
        }

        let state = Rc::new(Cell::new(ReadState::Pending));
        let io = Compat::new(tokio::io::join(
            ClosingReader {
                state: state.clone(),
                reset,
            },
            Vec::<u8>::new(),
        ));
        let (mut tx, rx) = crate::client::dispatch::channel();
        let conn = Conn::<_, bytes::Bytes, ClientTransaction>::new(io);
        let mut dispatcher = Dispatcher::new(Client::new(rx), conn);

        // Complete an exchange to put the connection into idle keep-alive.
        let mut first = tokio_test::task::spawn(
            tx.try_send(crate::Request::new(IncomingBody::empty()))
                .unwrap(),
        );
        tokio_test::task::spawn(()).enter(|cx, _| {
            assert!(Pin::new(&mut dispatcher).poll(cx).is_pending());
            state.set(ReadState::Response);
            assert!(Pin::new(&mut dispatcher).poll(cx).is_pending());
        });
        tokio_test::assert_ready_ok!(first.poll()).unwrap();
        assert!(tx.is_ready());
        state.set(ReadState::Close);
        let req = crate::Request::builder()
            .uri("/not-sent")
            .body(IncomingBody::empty())
            .unwrap();
        let promise = tx.try_send(req).unwrap();
        let mut dispatcher = tokio_test::task::spawn(futures::future::poll_fn(|cx| {
            dispatcher.poll_catch(cx, should_shutdown)
        }));

        // Exhaust Tokio's cooperative budget so even a closed channel returns
        // Pending when polled. This deterministically exercises the same drain
        // path as a send that has reserved a slot but not yet published it.
        let (budget_tx, mut budget_rx) = tokio::sync::mpsc::unbounded_channel();
        loop {
            budget_tx.send(()).unwrap();
            if task::now_or_never(budget_rx.recv()).is_none() {
                break;
            }
        }

        assert!(
            dispatcher.poll().is_pending(),
            "must finish draining before exiting"
        );
        assert!(tx.is_closed(), "must reject new requests while draining");

        // The dispatcher must retain its original result while the drain is
        // pending, rather than polling the closed connection again.
        tokio::task::yield_now().await;
        assert!(dispatcher.is_woken());
        let result = tokio_test::assert_ready!(dispatcher.poll());
        if reset {
            let err = result.err().expect("connection reset error");
            assert_eq!(
                err.find_source::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::ConnectionReset,
            );
        } else {
            assert!(matches!(result, Ok(Dispatched::Shutdown)));
        }

        // Keep both the sender and dispatcher alive: cancellation must not
        // depend on either one's destructor running.
        let mut promise = tokio_test::task::spawn(promise);
        let mut err = tokio_test::assert_ready_ok!(promise.poll()).unwrap_err();
        assert!(err.error().is_canceled());
        assert_eq!(err.take_message().unwrap().uri(), "/not-sent");
        assert!(tx.is_closed());
    }

    #[test]
    fn client_read_bytes_before_writing_request() {
        let _ = pretty_env_logger::try_init();

        tokio_test::task::spawn(()).enter(|cx, _| {
            let (io, mut handle) = tokio_test::io::Builder::new().build_with_handle();

            // Block at 0 for now, but we will release this response before
            // the request is ready to write later...
            let (mut tx, rx) = crate::client::dispatch::channel();
            let conn = Conn::<_, bytes::Bytes, ClientTransaction>::new(Compat::new(io));
            let mut dispatcher = Dispatcher::new(Client::new(rx), conn);

            // First poll is needed to allow tx to send...
            assert!(Pin::new(&mut dispatcher).poll(cx).is_pending());

            // Unblock our IO, which has a response before we've sent request!
            //
            handle.read(b"HTTP/1.1 200 OK\r\n\r\n");

            let mut res_rx = tx
                .try_send(crate::Request::new(IncomingBody::empty()))
                .unwrap();

            tokio_test::assert_ready_ok!(Pin::new(&mut dispatcher).poll(cx));
            let err = tokio_test::assert_ready_ok!(Pin::new(&mut res_rx).poll(cx))
                .expect_err("callback should send error");

            match (err.error.is_canceled(), err.message.as_ref()) {
                (true, Some(_)) => (),
                _ => panic!("expected Canceled, got {:?}", err),
            }
        });
    }

    #[cfg(not(miri))]
    #[tokio::test]
    async fn client_flushing_is_not_ready_for_next_request() {
        let _ = pretty_env_logger::try_init();

        let (io, _handle) = tokio_test::io::Builder::new()
            .write(b"POST / HTTP/1.1\r\ncontent-length: 4\r\n\r\n")
            .read(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
            .wait(std::time::Duration::from_secs(2))
            .build_with_handle();

        let (mut tx, rx) = crate::client::dispatch::channel();
        let mut conn = Conn::<_, bytes::Bytes, ClientTransaction>::new(Compat::new(io));
        conn.set_write_strategy_queue();

        let dispatcher = Dispatcher::new(Client::new(rx), conn);
        let _dispatcher = tokio::spawn(async move { dispatcher.await });

        let body = {
            let (mut tx, body) = IncomingBody::new_channel(DecodedLength::new(4), false);
            tx.try_send_data("reee".into()).unwrap();
            body
        };

        let req = crate::Request::builder().method("POST").body(body).unwrap();

        let res = tx.try_send(req).unwrap().await.expect("response");
        drop(res);

        assert!(!tx.is_ready());
    }

    #[cfg(not(miri))]
    #[tokio::test]
    async fn body_empty_chunks_ignored() {
        let _ = pretty_env_logger::try_init();

        let io = tokio_test::io::Builder::new()
            // no reading or writing, just be blocked for the test...
            .wait(Duration::from_secs(5))
            .build();

        let (mut tx, rx) = crate::client::dispatch::channel();
        let conn = Conn::<_, bytes::Bytes, ClientTransaction>::new(Compat::new(io));
        let mut dispatcher = tokio_test::task::spawn(Dispatcher::new(Client::new(rx), conn));

        // First poll is needed to allow tx to send...
        assert!(dispatcher.poll().is_pending());

        let body = {
            let (mut tx, body) = IncomingBody::channel();
            tx.try_send_data("".into()).unwrap();
            body
        };

        let _res_rx = tx.try_send(crate::Request::new(body)).unwrap();

        // Ensure conn.write_body wasn't called with the empty chunk.
        // If it is, it will trigger an assertion.
        assert!(dispatcher.poll().is_pending());
    }
}
