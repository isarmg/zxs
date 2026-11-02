// Public select! pattern regressions; every input future is immediately ready.
use std::future::{Future, ready};
use std::pin::pin;
use std::task::{Context, Poll, Waker};

fn ready_result<T>(future: impl Future<Output = T>) -> T {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("fixture select contains only immediately ready inputs"),
    }
}

#[test]
fn unparenthesized_mut_binding_is_a_control() {
    let value = ready_result(async {
        tokio::select! {
            Some(mut value) = ready(Some(String::from("left"))) => {
                value.push_str("-right");
                value
            }
        }
    });
    assert_eq!(value, "left-right");
}

#[test]
#[allow(
    unused_parens,
    reason = "Nested parentheses exercise Pat::Paren traversal in the pattern precheck."
)]
fn parenthesized_mut_binding_keeps_the_owned_output() {
    let value = ready_result(async {
        tokio::select! {
            ((Some(mut value))) = ready(Some(String::from("left"))) => {
                value.push_str("-right");
                value
            }
        }
    });
    assert_eq!(value, "left-right");
}

#[test]
fn unparenthesized_ref_mut_binding_is_a_control() {
    let value = ready_result(async {
        tokio::select! {
            Some(ref mut value) = ready(Some(String::from("left"))) => {
                value.push_str("-right");
                value.clone()
            }
        }
    });
    assert_eq!(value, "left-right");
}

#[test]
#[allow(
    unused_parens,
    reason = "Nested parentheses exercise Pat::Paren traversal in the pattern precheck."
)]
fn parenthesized_ref_mut_binding_mutates_only_the_owned_output() {
    let value = ready_result(async {
        tokio::select! {
            ((Some(ref mut value))) = ready(Some(String::from("left"))) => {
                value.push_str("-right");
                value.clone()
            }
        }
    });
    assert_eq!(value, "left-right");
}

#[test]
#[allow(
    unused_parens,
    reason = "The parenthesized pattern must still disable a nonmatching select branch."
)]
fn parenthesized_nonmatching_pattern_keeps_else_behavior() {
    let value = ready_result(async {
        tokio::select! {
            ((Some(mut value))) = ready(None::<String>) => {
                value.push_str("must-not-run");
                value
            },
            else => String::from("no-match"),
        }
    });
    assert_eq!(value, "no-match");
}
