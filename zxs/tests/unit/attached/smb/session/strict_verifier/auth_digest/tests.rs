use super::Md5;
use crate::smb::fixed_output::Digest;

#[test]
fn ntlm_digest_matches_rfc1321_at_every_buffer_split() {
    // RFC 1321 Appendix A.5: empty, short, long-padding and full-block inputs.
    let examples: [(&[u8], &str); 4] = [
        (b"", "d41d8cd98f00b204e9800998ecf8427e"),
        (b"abc", "900150983cd24fb0d6963f7d28e17f72"),
        (b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", "d174ab98d277d9f5a5611c2c9f419d9f"),
        (b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
            "57edf4a22be3c955ac49da2e2107b67a"),
    ];
    for (input, hex) in examples {
        let tag: [u8; 16] = std::array::from_fn(|i| u8::from_str_radix(&hex[2*i..2*i+2], 16).unwrap());
        for split in 0..=input.len() {
            let mut state = Md5::new();
            state.update(&input[..split]);
            state.update(&input[split..]);
            assert_eq!(state.finalize().as_slice(), &tag, "{}/{split}",input.len());
        }
    }
}
