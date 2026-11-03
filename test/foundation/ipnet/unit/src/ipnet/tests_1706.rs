#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! make_ipnet_vec {
        ($($x:expr),*) => ( vec![$($x.parse::<IpNet>().unwrap(),)*] );
        ($($x:expr,)*) => ( make_ipnet_vec![$($x),*] );
    }

    #[test]
    fn test_make_ipnet_vec() {
        assert_eq!(
            make_ipnet_vec![
                "10.1.1.1/32", "10.2.2.2/24", "10.3.3.3/16",
                "fd00::1/128", "fd00::2/127", "fd00::3/126",
            ],
            vec![
                "10.1.1.1/32".parse().unwrap(),
                "10.2.2.2/24".parse().unwrap(),
                "10.3.3.3/16".parse().unwrap(),
                "fd00::1/128".parse().unwrap(),
                "fd00::2/127".parse().unwrap(),
                "fd00::3/126".parse().unwrap(),
            ]
        );
    }

    macro_rules! make_ipv4_subnets_test {
        ($name:ident, $start:expr, $end:expr, $min_prefix_len:expr, $($x:expr),*) => (
            #[test]
            fn $name() {
                let subnets = IpSubnets::from(Ipv4Subnets::new(
                    $start.parse().unwrap(),
                    $end.parse().unwrap(),
                    $min_prefix_len,
                ).unwrap());
                let results = make_ipnet_vec![$($x),*];
                assert_eq!(subnets.collect::<Vec<IpNet>>(), results);
            }
        );
        ($name:ident, $start:expr, $end:expr, $min_prefix_len:expr, $($x:expr,)*) => (
            make_ipv4_subnets_test!($name, $start, $end, $min_prefix_len, $($x),*);
        );
    }

    macro_rules! make_ipv6_subnets_test {
        ($name:ident, $start:expr, $end:expr, $min_prefix_len:expr, $($x:expr),*) => (
            #[test]
            fn $name() {
                let subnets = IpSubnets::from(Ipv6Subnets::new(
                    $start.parse().unwrap(),
                    $end.parse().unwrap(),
                    $min_prefix_len,
                ).unwrap());
                let results = make_ipnet_vec![$($x),*];
                assert_eq!(subnets.collect::<Vec<IpNet>>(), results);
            }
        );
        ($name:ident, $start:expr, $end:expr, $min_prefix_len:expr, $($x:expr,)*) => (
            make_ipv6_subnets_test!($name, $start, $end, $min_prefix_len, $($x),*);
        );
    }

    make_ipv4_subnets_test!(
        test_ipv4_subnets_zero_zero,
        "0.0.0.0", "0.0.0.0", 0,
        "0.0.0.0/32",
    );

    make_ipv4_subnets_test!(
        test_ipv4_subnets_zero_max,
        "0.0.0.0", "255.255.255.255", 0,
        "0.0.0.0/0",
    );

    make_ipv4_subnets_test!(
        test_ipv4_subnets_max_max,
        "255.255.255.255", "255.255.255.255", 0,
        "255.255.255.255/32",
    );

    make_ipv4_subnets_test!(
        test_ipv4_subnets_none,
        "0.0.0.1", "0.0.0.0", 0,
    );

    make_ipv4_subnets_test!(
        test_ipv4_subnets_one,
        "0.0.0.0", "0.0.0.1", 0,
        "0.0.0.0/31",
    );

    make_ipv4_subnets_test!(
        test_ipv4_subnets_two,
        "0.0.0.0", "0.0.0.2", 0,
        "0.0.0.0/31",
        "0.0.0.2/32",
    );

    make_ipv4_subnets_test!(
        test_ipv4_subnets_taper,
        "0.0.0.0", "0.0.0.10", 30,
        "0.0.0.0/30",
        "0.0.0.4/30",
        "0.0.0.8/31",
        "0.0.0.10/32",
    );

    make_ipv6_subnets_test!(
        test_ipv6_subnets_zero_zero,
        "::", "::", 0,
        "::/128",
    );

    make_ipv6_subnets_test!(
        test_ipv6_subnets_zero_max,
        "::", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", 0,
        "::/0",
    );

    make_ipv6_subnets_test!(
        test_ipv6_subnets_max_max,
        "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", 0,
        "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff/128",
    );

    make_ipv6_subnets_test!(
        test_ipv6_subnets_none,
        "::1", "::", 0,
    );

    make_ipv6_subnets_test!(
        test_ipv6_subnets_one,
        "::", "::1", 0,
        "::/127",
    );

    make_ipv6_subnets_test!(
        test_ipv6_subnets_two,
        "::", "::2", 0,
        "::/127",
        "::2/128",
    );

    make_ipv6_subnets_test!(
        test_ipv6_subnets_taper,
        "::", "::a", 126,
        "::/126",
        "::4/126",
        "::8/127",
        "::a/128",
    );

    // Issue #70
    #[test]
    fn test_ipv4_subnets_zero_max_minus_one() {
        let subnets: Vec<Ipv4Net> = Ipv4Subnets::new(
            Ipv4Addr::from(0u32),
            Ipv4Addr::from(u32::MAX - 1),
            0,
        )
        .unwrap()
        .collect();
        assert!(!subnets[0].contains(&Ipv4Addr::from(u32::MAX)));
    }

    // Issue #70
    #[test]
    fn test_ipv6_subnets_zero_max_minus_one() {
        let subnets: Vec<Ipv6Net> = Ipv6Subnets::new(
            Ipv6Addr::from(0u128),
            Ipv6Addr::from(u128::MAX - 1),
            0,
        )
        .unwrap()
        .collect();
        assert!(!subnets[0].contains(&Ipv6Addr::from(u128::MAX)));
    }

    #[test]
    fn ipnet_aggregate() {
        let ip_nets = make_ipnet_vec![
            "10.0.0.0/24", "10.0.1.0/24", "10.0.1.1/24", "10.0.1.2/24",
            "10.0.2.0/24",
            "10.1.0.0/24", "10.1.1.0/24",
            "192.168.0.0/24", "192.168.1.0/24", "192.168.2.0/24", "192.168.3.0/24",
            "fd00::/32", "fd00:1::/32",
            "fd00:2::/32",
        ];

        let ip_aggs = make_ipnet_vec![
            "10.0.0.0/23",
            "10.0.2.0/24",
            "10.1.0.0/23",
            "192.168.0.0/22",
            "fd00::/31",
            "fd00:2::/32",
        ];

        assert_eq!(IpNet::aggregate(&ip_nets), ip_aggs);

        // Issue #44
        assert_eq!(IpNet::aggregate(&make_ipnet_vec!["128.0.0.0/1"]), make_ipnet_vec!["128.0.0.0/1"]);
        assert_eq!(IpNet::aggregate(&make_ipnet_vec!["0.0.0.0/1", "128.0.0.0/1"]), make_ipnet_vec!["0.0.0.0/0"]);
        assert_eq!(IpNet::aggregate(&make_ipnet_vec!["8000::/1"]), make_ipnet_vec!["8000::/1"]);
        assert_eq!(IpNet::aggregate(&make_ipnet_vec!["::/1", "8000::/1"]), make_ipnet_vec!["::/0"]);

        // Issue #71
        assert_eq!(IpNet::aggregate(&make_ipnet_vec!["255.255.255.254/32"]), make_ipnet_vec!["255.255.255.254/32"]);
        assert_eq!(IpNet::aggregate(&make_ipnet_vec!["255.255.255.255/32"]), make_ipnet_vec!["255.255.255.255/32"]);
        assert_eq!(IpNet::aggregate(&make_ipnet_vec!["255.255.255.252/31", "255.255.255.254/32"]), make_ipnet_vec!["255.255.255.252/31", "255.255.255.254/32"]);
        assert_eq!(IpNet::aggregate(&make_ipnet_vec!["ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe/128"]), make_ipnet_vec!["ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe/128"]);
        assert_eq!(IpNet::aggregate(&make_ipnet_vec!["ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff/128"]), make_ipnet_vec!["ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff/128"]);
        assert_eq!(IpNet::aggregate(&make_ipnet_vec!["ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffc/127", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe/128"]), make_ipnet_vec!["ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffc/127", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe/128"]);
    }

    #[test]
    fn ipnet_default() {
        let ipnet: IpNet = "0.0.0.0/0".parse().unwrap();
        assert_eq!(ipnet, IpNet::default());
    }

    #[test]
    fn ipv4net_default() {
        let ipnet: Ipv4Net = "0.0.0.0/0".parse().unwrap();
        assert_eq!(ipnet, Ipv4Net::default());
    }

    #[test]
    fn ipv6net_default() {
        let ipnet: Ipv6Net = "::/0".parse().unwrap();
        assert_eq!(ipnet, Ipv6Net::default());
    }

    #[test]
    fn new_assert() {
        const _: Ipv4Net = Ipv4Net::new_assert(Ipv4Addr::new(0, 0, 0, 0), 0);
        const _: Ipv4Net = Ipv4Net::new_assert(Ipv4Addr::new(0, 0, 0, 0), 32);
        const _: Ipv6Net = Ipv6Net::new_assert(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0), 0);
        const _: Ipv6Net = Ipv6Net::new_assert(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0), 128);

        let _ = Ipv4Net::new_assert(Ipv4Addr::new(0, 0, 0, 0), 0);
        let _ = Ipv4Net::new_assert(Ipv4Addr::new(0, 0, 0, 0), 32);
        let _ = Ipv6Net::new_assert(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0), 0);
        let _ = Ipv6Net::new_assert(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0), 128);
    }

    #[test]
    #[should_panic]
    fn ipv4net_new_assert_panics() {
        let _ = Ipv4Net::new_assert(Ipv4Addr::new(0, 0, 0, 0), 33);
    }

    #[test]
    #[should_panic]
    fn ipv6net_new_assert_panics() {
        let _ = Ipv6Net::new_assert(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0), 129);
    }
}
