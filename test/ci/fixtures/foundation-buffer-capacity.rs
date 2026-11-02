use futures::{Stream, StreamExt, TryStreamExt, stream};
use std::{
    future::{Future, Ready, ready},
    num::NonZeroUsize,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

fn plain<F>(items: Vec<F>) -> impl Stream<Item = F> + Unpin {
    stream::iter(items)
}
fn fallible<F>(items: Vec<F>) -> impl Stream<Item = Result<F, &'static str>> + Unpin {
    stream::iter(items.into_iter().map(Ok))
}
struct PendingFuture(Arc<AtomicUsize>);
impl Future for PendingFuture {
    type Output = Result<u8, &'static str>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}
impl Drop for PendingFuture {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

macro_rules! capacity_cases {
    ($positive:ident, $bounded:ident, $source:ident, $method:ident) => {
        #[test]
        fn $positive() {
            let capacity = NonZeroUsize::new(1).unwrap();
            let mut stream =
                $source(vec![ready(Ok(1)), ready(Err("original")), ready(Ok(3))]).$method(capacity);
            for value in [Ok(1), Err("original"), Ok(3)] {
                assert_eq!(
                    super::poll_next(Pin::new(&mut stream)),
                    Poll::Ready(Some(value))
                );
            }
            assert_eq!(super::poll_next(Pin::new(&mut stream)), Poll::Ready(None));
            let mut empty =
                $source(Vec::<Ready<Result<u8, &'static str>>>::new()).$method(capacity);
            assert_eq!(super::poll_next(Pin::new(&mut empty)), Poll::Ready(None));
        }
        #[test]
        fn $bounded() {
            let dropped = Arc::new(AtomicUsize::new(0));
            let admitted = Arc::new(AtomicUsize::new(0));
            let count = Arc::clone(&admitted);
            let source = $source(
                (0..3)
                    .map(|_| PendingFuture(Arc::clone(&dropped)))
                    .collect(),
            )
            .inspect(move |_| {
                count.fetch_add(1, Ordering::SeqCst);
            });
            let mut stream = source.$method(NonZeroUsize::new(2).unwrap());
            assert_eq!(super::poll_next(Pin::new(&mut stream)), Poll::Pending);
            assert_eq!(admitted.load(Ordering::SeqCst), 2);
            assert_eq!(super::poll_next(Pin::new(&mut stream)), Poll::Pending);
            assert_eq!(admitted.load(Ordering::SeqCst), 2);
            drop(stream);
            assert_eq!(dropped.load(Ordering::SeqCst), 3);
        }
    };
}
capacity_cases!(buffered_positive, buffered_bounds_and_drop, plain, buffered);
capacity_cases!(
    unordered_positive,
    unordered_bounds_and_drop,
    plain,
    buffer_unordered
);
capacity_cases!(
    try_buffered_positive,
    try_buffered_bounds_and_drop,
    fallible,
    try_buffered
);
capacity_cases!(
    try_unordered_positive,
    try_unordered_bounds_and_drop,
    fallible,
    try_buffer_unordered
);

#[test]
fn try_buffers_preserve_source_error_and_continue() {
    macro_rules! check {
        ($method:ident) => {{
            let mut source =
                stream::iter([Ok(ready(Ok(1))), Err("source error"), Ok(ready(Ok(2)))])
                    .$method(NonZeroUsize::new(1).unwrap());
            for expected in [Ok(1), Err("source error"), Ok(2)] {
                assert_eq!(
                    super::poll_next(Pin::new(&mut source)),
                    Poll::Ready(Some(expected))
                );
            }
            assert_eq!(super::poll_next(Pin::new(&mut source)), Poll::Ready(None));
        }};
    }
    check!(try_buffered);
    check!(try_buffer_unordered);
}
