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
