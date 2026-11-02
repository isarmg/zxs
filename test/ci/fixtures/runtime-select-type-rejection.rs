// The owned handler must reject a String-only operation on a u8 payload.
#[allow(
    unused_parens,
    reason = "Parentheses exercise the same pattern precheck as the positive regression."
)]
fn main() {
    let _future = async {
        tokio::select! {
            ((Some(mut value))) = std::future::ready(Some(7u8)) => {
                value.push_str("wrong-type");
            }
        }
    };
}
