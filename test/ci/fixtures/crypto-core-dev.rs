#[cfg(feature = "dev")]
#[test]
fn supported_dev_helpers_validate_fixed_hash() {
    let vector = cryptographic_core::dev::TestVector {
        input: b"abc",
        output: b"abc\0\0\0\0\0",
    };
    assert_eq!(
        cryptographic_core::dev::fixed_test::<FixedHashWithSer>(&vector),
        Ok(())
    );
    assert_eq!(
        cryptographic_core::dev::fixed_reset_test::<FixedHashWithSer>(&vector),
        Ok(())
    );
}

#[test]
fn u128_array_state_uses_full_element_width() {
    use cryptographic_core::common_types::hazmat::SerializableState;
    macro_rules! check {
        ($length:expr) => {{
            type State = [u128; $length];
            let values: State = core::array::from_fn(|index| {
                0xfedc_ba98_7654_3210_0123_4567_89ab_cdef_u128 ^ index as u128
            });
            let serialized = values.serialize();
            assert_eq!(serialized.len(), 16 * $length);
            assert_eq!(
                &serialized[..16],
                &[0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01,
                  0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe]
            );
            for (value, chunk) in values.iter().zip(serialized.chunks_exact(16)) {
                assert_eq!(chunk, value.to_le_bytes());
            }
            assert_eq!(State::deserialize(&serialized).unwrap(), values);
        }};
    }
    check!(1);
    check!(2);
    check!(3);
    check!(4);
    check!(6);
    check!(8);
    check!(12);
    check!(16);
    check!(24);
    check!(32);
    check!(48);
    check!(64);
    check!(128);
    check!(256);
    check!(512);
}

#[test]
fn u128_array_state_decodes_independent_little_endian_bytes() {
    use cryptographic_core::common_types::hazmat::{SerializableState, SerializedState};
    let mut serialized = SerializedState::<[u128; 2]>::default();
    for (index, byte) in serialized.iter_mut().enumerate() {
        *byte = index as u8;
    }
    assert_eq!(
        <[u128; 2]>::deserialize(&serialized).unwrap(),
        [
            0x0f0e_0d0c_0b0a_0908_0706_0504_0302_0100,
            0x1f1e_1d1c_1b1a_1918_1716_1514_1312_1110,
        ]
    );
}
