use super::*;

#[test]
fn preserved_chain_depth_and_total_physical_roots_have_distinct_limits() {
    assert!(require_count(1, 4).is_ok());
    assert!(require_count(29, 3).is_ok());
    assert!(require_count(1, 31).is_ok());
    for (current, added) in [(0, 32), (1, 32), (30, 3), (2, 1), (usize::MAX, 2)] {
        assert_eq!(require_count(current, added).unwrap_err().code(), ErrorCode::ResourceLimit);
    }
}
