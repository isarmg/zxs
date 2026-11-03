#[cfg(test)]
mod subnets_contract {
    use super::*;

    #[test]
    fn ipv4_subnets_rejects_invalid_prefix() {
        for prefix in 33..=u8::MAX {
            for (start, end) in [(0, 0), (0, u32::MAX), (1, 0)] {
                assert_eq!(
                    Ipv4Subnets::new(Ipv4Addr::from(start), Ipv4Addr::from(end), prefix),
                    Err(PrefixLenError),
                );
            }
        }
    }

    #[test]
    fn ipv6_subnets_rejects_invalid_prefix() {
        for prefix in 129..=u8::MAX {
            for (start, end) in [(0, 0), (0, u128::MAX), (1, 0)] {
                assert_eq!(
                    Ipv6Subnets::new(Ipv6Addr::from(start), Ipv6Addr::from(end), prefix),
                    Err(PrefixLenError),
                );
            }
        }
    }

    #[test]
    fn ipv4_subnets_accepts_valid_prefixes_and_preserves_empty_ranges() {
        for prefix in 0..=32 {
            for value in [0, 1, u32::MAX] {
                let addr = Ipv4Addr::from(value);
                let mut subnets = Ipv4Subnets::new(addr, addr, prefix).unwrap();
                assert_eq!(subnets.next(), Some(Ipv4Net::new(addr, 32).unwrap()));
                assert_eq!(subnets.next(), None);
                assert_eq!(subnets.next(), None);
            }
            let mut empty = Ipv4Subnets::new(Ipv4Addr::from(1), Ipv4Addr::from(0), prefix).unwrap();
            assert_eq!(empty.next(), None);
            assert_eq!(empty.next(), None);
        }
    }

    #[test]
    fn ipv6_subnets_accepts_valid_prefixes_and_preserves_empty_ranges() {
        for prefix in 0..=128 {
            for value in [0, 1, u128::MAX] {
                let addr = Ipv6Addr::from(value);
                let mut subnets = Ipv6Subnets::new(addr, addr, prefix).unwrap();
                assert_eq!(subnets.next(), Some(Ipv6Net::new(addr, 128).unwrap()));
                assert_eq!(subnets.next(), None);
                assert_eq!(subnets.next(), None);
            }
            let mut empty = Ipv6Subnets::new(Ipv6Addr::from(1), Ipv6Addr::from(0), prefix).unwrap();
            assert_eq!(empty.next(), None);
            assert_eq!(empty.next(), None);
        }
    }
}
