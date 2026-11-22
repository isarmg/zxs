use super::tag_equal_choice;

#[test]
fn tag_choice_keeps_equal_bytes_and_rejects_public_differences() {
    let input = core::array::from_fn::<_, 24, _>(|i| u8::try_from(i).unwrap());
    for size in 0..=input.len() {
        let prefix = &input[..size];
        assert_eq!(tag_equal_choice(prefix, prefix), 1);
        if size < input.len() {
            assert_eq!(tag_equal_choice(prefix, &input[..=size]), 0);
        }
        for offset in 0..size {
            let mut changed = input;
            changed[offset] ^= 1;
            assert_eq!(tag_equal_choice(prefix, &changed[..size]), 0);
        }
    }
}
