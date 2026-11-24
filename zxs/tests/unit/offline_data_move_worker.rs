use super::*;

fn hex(raw: [u8; 32]) -> String {
    use std::fmt::Write as _;
    raw.iter().fold(String::new(), |mut out, byte| {
        write!(&mut out, "{byte:02x}").unwrap();
        out
    })
}

fn example() -> Vec<u8> {
    let nodes = [
        [
            0x0102_0304_0506_0708,
            0x1112_1314_1516_1718,
            0x2122_2324_2526_2728,
        ],
        [0x31, 0x32, 0x33],
        [0x41, 0x42, 0x43],
        [0x51, 0x52, 0x53],
    ];
    let binding = role_binding(ROLE_DOMAIN, [0x42; 32], &nodes).unwrap();
    assert_eq!(
        hex(binding),
        "9e9677fc4c09673a235b3bc2a88197afc6c588a1c4086ba7750c0ad8aedff98c"
    );
    let parse_digest = |typed: &str| {
        <[u8; 32]>::try_from(
            typed
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|candidate| {
                    u8::from_str_radix(std::str::from_utf8(candidate).unwrap(), 16).unwrap()
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    };
    let mut out = Vec::from(*b"ZXDMV1\0\0");
    out.extend(binding);
    out.extend([0x24; 32]);
    out.extend([0x31; 16]);
    out.extend([0x32; 16]);
    out.extend(parse_digest(
        "782d96da9a7c4516df1b1a72dc4982faaa05befc9c5dd71469047253ebb48434",
    ));
    out.extend(parse_digest(
        "f545a79a125aaa6c1c9d8f4bc81a0d7ff3ae8d521b9a726a1a179d73bf1ee0a6",
    ));
    let mut h = Sha256::new();
    h.update(&out).unwrap();
    assert_eq!(
        hex(h.finish()),
        "bdbd276eb6e0aee8b13a896d9e490d847ba61177e51bc9e321387e443eb589d6"
    );
    out
}

#[test]
fn move_order_bounds_and_terminal_distinguish_new_from_history() {
    let sample = example();
    let order = Order::decode(&sample).unwrap();
    for (cut, class) in [
        (40..72, 1),
        (72..88, 2),
        (88..104, 3),
        (104..136, 4),
        (136..168, 5),
    ] {
        let mut changed = sample.clone();
        changed[cut].fill(0);
        assert!(Order::decode(&changed).is_err(), "{class}");
    }
    let mut changed = sample.clone();
    changed.copy_within(104..136, 136);
    assert!(Order::decode(&changed).is_err());
    assert!(Order::decode(&sample[..167]).is_err());
    changed.push(0);
    assert!(Order::decode(&changed).is_err());
    let fresh = terminal(order.request, MoveCompletion::Completed([7; 32]));
    let past = terminal(order.request, MoveCompletion::Historical([7; 32]));
    assert_eq!(fresh[0], 1);
    assert_eq!(past[0], 2);
    assert_eq!(&fresh[1..17], &[0x31; 16]);
    assert_eq!(&fresh[17..33], &[0x32; 16]);
    assert_eq!(&fresh[33..], &[7; 32]);
    assert_eq!(fresh[1..], past[1..]);
}
