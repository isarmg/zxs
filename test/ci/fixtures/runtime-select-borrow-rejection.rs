// A mutable handler binding must not escape the macro-local owned output.
#[allow(
    unused_parens,
    reason = "Parentheses exercise the same pattern precheck as the positive regression."
)]
fn main() {
    let _future = async {
        let escaped = tokio::select! {
            ((Some(ref mut value))) = std::future::ready(Some(String::from("owned"))) => value,
        };
        escaped.push_str("must-not-escape");
    };
}
