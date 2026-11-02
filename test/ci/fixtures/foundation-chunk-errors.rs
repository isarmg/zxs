use futures::{TryStreamExt, stream};
use std::{error::Error, fmt, pin::Pin, task::Poll};

#[derive(Debug)]
struct Cause;
impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("original cause")
    }
}
impl Error for Cause {}

#[derive(Debug)]
struct Failure(Cause);
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("stream failure")
    }
}
impl Error for Failure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

macro_rules! chunk_contracts {
    ($source_test:ident, $value_test:ident, $method:ident) => {
        #[test]
        fn $source_test() {
            let mut chunks = stream::iter([Ok(1), Err(Failure(Cause)), Ok(2)]).$method(2);
            let Poll::Ready(Some(Err(error))) = super::poll_next(Pin::new(&mut chunks)) else {
                panic!("expected the partial chunk and original stream error");
            };
            assert_eq!(error.0, [1]);
            assert_eq!(error.to_string(), "stream failure");
            let source = error.source().expect("the wrapped error is the source");
            assert!(std::ptr::eq(
                source.downcast_ref::<Failure>().unwrap(),
                &error.1
            ));
            assert!(std::ptr::eq(
                source.source().unwrap().downcast_ref::<Cause>().unwrap(),
                &error.1.0
            ));
            assert_eq!(
                super::poll_next(Pin::new(&mut chunks)).map(|v| v.map(|r| r.unwrap())),
                Poll::Ready(Some(vec![2]))
            );
            assert!(matches!(
                super::poll_next(Pin::new(&mut chunks)),
                Poll::Ready(None)
            ));
        }

        #[test]
        fn $value_test() {
            let message = String::from("borrowed failure");
            let mut chunks = stream::iter([Ok(1), Err(message.as_str()), Ok(2)]).$method(2);
            let Poll::Ready(Some(Err(error))) = super::poll_next(Pin::new(&mut chunks)) else {
                panic!("expected a borrowed non-Error payload");
            };
            assert_eq!(error.0, [1]);
            assert_eq!(error.1, message.as_str());
            assert_eq!(error.to_string(), message);
            assert!(format!("{error:?}").contains(&message));
            let mut numeric = stream::iter([Ok(3), Err(4)]).$method(2);
            let Poll::Ready(Some(Err(error))) = super::poll_next(Pin::new(&mut numeric)) else {
                panic!("expected a numeric non-Error payload");
            };
            assert_eq!(error.0, [3]);
            assert_eq!(error.1, 4);
        }
    };
}

chunk_contracts!(
    chunks_preserves_source_chain,
    chunks_accepts_non_error_payloads,
    try_chunks
);
chunk_contracts!(
    ready_chunks_preserves_source_chain,
    ready_chunks_accepts_non_error_payloads,
    try_ready_chunks
);
