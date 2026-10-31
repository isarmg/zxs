#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::Encoder;

    /// OID `1.2.840.10045.2.1` encoded as ASN.1 BER/DER
    const EXAMPLE_OID_BER: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];

    #[test]
    fn base128_byte() {
        let example_arc = 0x44332211;
        assert_eq!(super::base128_len(example_arc), 5);
        assert_eq!(super::base128_byte(example_arc, 0, 5).unwrap(), 0b10000100);
        assert_eq!(super::base128_byte(example_arc, 1, 5).unwrap(), 0b10100001);
        assert_eq!(super::base128_byte(example_arc, 2, 5).unwrap(), 0b11001100);
        assert_eq!(super::base128_byte(example_arc, 3, 5).unwrap(), 0b11000100);
        assert_eq!(super::base128_byte(example_arc, 4, 5).unwrap(), 0b10001);
    }

    #[test]
    fn encode() {
        let encoder = Encoder::<7>::new();
        let encoder = encoder.arc(1).unwrap();
        let encoder = encoder.arc(2).unwrap();
        let encoder = encoder.arc(840).unwrap();
        let encoder = encoder.arc(10045).unwrap();
        let encoder = encoder.arc(2).unwrap();
        let encoder = encoder.arc(1).unwrap();
        assert_eq!(&encoder.bytes[..encoder.cursor], EXAMPLE_OID_BER);
    }
}
