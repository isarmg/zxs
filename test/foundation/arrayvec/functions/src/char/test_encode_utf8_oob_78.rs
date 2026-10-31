#[test]
fn test_encode_utf8_oob() {
    // test that we report oob if the buffer is too short
    let mut data = [0u8; 16];
    let chars = ['a', 'α', '�', '𐍈'];
    for (len, &ch) in (1..=4).zip(&chars) {
        assert_eq!(len, ch.len_utf8(), "Len of ch={}", ch);
        let ptr = data.as_mut_ptr();
        unsafe {
            assert!(matches!(encode_utf8(ch, ptr, len - 1), Err(_)));
            assert!(matches!(encode_utf8(ch, ptr, len), Ok(_)));
        }
    }
}
