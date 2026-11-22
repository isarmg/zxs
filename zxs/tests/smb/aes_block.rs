use super::*;

#[test]
fn fips_197_aes128_single_and_batch_known_answer() {
    // FIPS PUB 197 (2001), Appendix C.1, a public known-answer vector.
    // https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.197.pdf
    let key: [u8; 16] = core::array::from_fn(|i| u8::try_from(i).unwrap());
    let input = Block::from(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff_u128.to_be_bytes());
    let output = Block::from(0x69c4_e0d8_6a7b_0430_d8cd_b780_70b4_c55a_u128.to_be_bytes());
    let cipher = Aes128::new(&Array::from(key));
    fn require_erasure<T: zeroize::ZeroizeOnDrop>(_: &T) {}
    require_erasure(&cipher);

    let mut single = input;
    cipher.encrypt_block(&mut single);
    assert_eq!(single, output);
    cipher.decrypt_block(&mut single);
    assert_eq!(single, input);

    // 64 blocks cover at least one batch on every existing AES backend.
    let mut batch = [input; 64];
    cipher.encrypt_blocks(&mut batch);
    assert_eq!(batch, [output; 64]);
    cipher.decrypt_blocks(&mut batch);
    assert_eq!(batch, [input; 64]);
}

#[test]
fn aes128_batch_matches_single_blocks_at_backend_boundaries() {
    let cipher = Aes128::new(&Array::from([0x35; 16]));
    let input: [Block; 129] = core::array::from_fn(|index| {
        let mut bytes = [0; 16];
        bytes[0] = u8::try_from(index).unwrap();
        Block::from(bytes)
    });
    let mut expected = input;
    for block in &mut expected {
        cipher.encrypt_block(block);
    }

    // Cover parallel groups of up to 64 blocks, with and without a tail.
    for len in [0, 1, 2, 4, 5, 7, 8, 9, 29, 30, 31, 63, 64, 65, 129] {
        let mut batch = input;
        cipher.encrypt_blocks(&mut batch[..len]);
        assert_eq!(batch[..len], expected[..len]);
        assert_eq!(batch[len..], input[len..]);
        cipher.decrypt_blocks(&mut batch[..len]);
        assert_eq!(batch, input);
    }
}

#[test]
fn fixed_keys_keep_size_checks_and_array_erasure() {
    use crate::smb::session::packet_mac::Cmac;

    let mut key: Key<Aes128> = Array::from([0x35; 16]);
    assert!(Aes128::new_from_slice(&key[..15]).is_err());
    assert!(Aes128::new_from_slice(&[0x35; 17]).is_err());
    assert!(Aes128::new_from_slice(&key).is_ok());
    assert!(Cmac::<Aes128>::new_from_slice(&key[..15]).is_err());
    assert!(Cmac::<Aes128>::new_from_slice(&key).is_ok());

    zeroize::Zeroize::zeroize(&mut key);
    assert_eq!(&key[..], &[0; 16]);
}
