#[test]
fn test_parse_iterator_all_invalid() {
    let mut iter = MimeIter::new("invalid, another-invalid");
    assert_eq!(iter.next().unwrap().unwrap_err(), "invalid");
    assert_eq!(iter.next(), None);
}
