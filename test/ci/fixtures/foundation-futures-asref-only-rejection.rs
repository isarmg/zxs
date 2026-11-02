use futures::io::AsyncWriteExt;
struct ChangingView;
impl AsRef<[u8]> for ChangingView {
    fn as_ref(&self) -> &[u8] {
        b"bytes"
    }
}
fn main() {
    let _ = Vec::<u8>::new().into_sink::<ChangingView>();
}
