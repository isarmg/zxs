// ZXS adaptation of RustCrypto AES; see block_cipher/ORIGIN.md.
#[cfg(target_arch = "x86")]
use core::arch::x86::*;
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

pub(crate) type RoundKeys<const RK: usize> = [__m128i; RK];

#[inline]
#[target_feature(enable = "aes")]
pub(super) fn aes128_expand_key(key: &[u8; 16]) -> RoundKeys<11> {
    #[target_feature(enable = "aes")]
    fn expand_round<const R: i32>(keys: &mut RoundKeys<11>, pos: usize) {
        let mut t1 = keys[pos - 1];
        let mut t2;
        let mut t3;

        t2 = _mm_aeskeygenassist_si128(t1, R);
        t2 = _mm_shuffle_epi32(t2, 0xff);
        t3 = _mm_slli_si128(t1, 0x4);
        t1 = _mm_xor_si128(t1, t3);
        t3 = _mm_slli_si128(t3, 0x4);
        t1 = _mm_xor_si128(t1, t3);
        t3 = _mm_slli_si128(t3, 0x4);
        t1 = _mm_xor_si128(t1, t3);
        t1 = _mm_xor_si128(t1, t2);

        keys[pos] = t1;
    }

    let mut keys = [_mm_setzero_si128(); 11];
    keys[0] = load(key);

    let kr = &mut keys;
    expand_round::<0x01>(kr, 1);
    expand_round::<0x02>(kr, 2);
    expand_round::<0x04>(kr, 3);
    expand_round::<0x08>(kr, 4);
    expand_round::<0x10>(kr, 5);
    expand_round::<0x20>(kr, 6);
    expand_round::<0x40>(kr, 7);
    expand_round::<0x80>(kr, 8);
    expand_round::<0x1B>(kr, 9);
    expand_round::<0x36>(kr, 10);

    keys
}

#[inline]
#[target_feature(enable = "aes")]
pub(super) fn inv_expanded_keys<const N: usize>(keys: &[__m128i; N]) -> [__m128i; N] {
    let mut inv_keys: [__m128i; N] = [_mm_setzero_si128(); N];
    inv_keys[0] = keys[N - 1];
    for i in 1..N - 1 {
        inv_keys[i] = _mm_aesimc_si128(keys[N - 1 - i]);
    }
    inv_keys[N - 1] = keys[0];
    inv_keys
}

#[target_feature(enable = "sse2")]
fn load(bytes: &[u8]) -> __m128i {
    assert!(size_of_val(bytes) <= size_of::<__m128i>());
    let mut t = crate::smb::session::block_cipher::Block::default();
    t[..bytes.len()].copy_from_slice(bytes);
    super::utils::load_block(&t)
}
