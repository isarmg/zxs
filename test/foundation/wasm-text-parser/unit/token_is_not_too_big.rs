use super::*;

#[test]
fn token_is_not_too_big() {
    assert!(std::mem::size_of::<Token>() <= std::mem::size_of::<u64>() * 2);
}
