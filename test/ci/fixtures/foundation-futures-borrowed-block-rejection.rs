use futures::Sink;
use futures::io::AsyncWriteExt;
use std::pin::Pin;
use std::task::{Context, Waker};
fn main() {
    let mut sink = Vec::<u8>::new().into_sink::<&[u8]>();
    {
        let bytes = vec![1, 2, 3];
        Pin::new(&mut sink).start_send(bytes.as_slice()).unwrap();
    }
    let mut cx = Context::from_waker(Waker::noop());
    let _ = Pin::new(&mut sink).poll_flush(&mut cx);
}
