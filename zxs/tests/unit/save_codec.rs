//! Fixed OC2 bytes, independent of the current encoder and decoder.
use super::*;

const FIRST: &str = concat!(
    "4f433200",
    "11111111111111111111111111111111",
    "22222222222222222222222222222222",
    "000000000000000101",
    "00046d61696e00",
    "33333333333333333333333333333333",
    "0102030405060708",
    "6666666666666666666666666666666666666666666666666666666666666666",
    "0106e6a0b7e69cac",
);
const APPEND: &str = concat!(
    "4f433200",
    "11111111111111111111111111111111",
    "22222222222222222222222222222222",
    "000000000000000201",
    "0004736964650144444444444444444444444444444444",
    "33333333333333333333333333333333",
    "0102030405060708",
    "6666666666666666666666666666666666666666666666666666666666666666",
    "00",
);
const FORK: &str = concat!(
    "4f433200",
    "11111111111111111111111111111111",
    "22222222222222222222222222222222",
    "000000000000000301",
    "01036e657755555555555555555555555555555555",
    "33333333333333333333333333333333",
    "0102030405060708",
    "6666666666666666666666666666666666666666666666666666666666666666",
    "00",
);

fn octets(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn exact_oc2_bytes_round_trip_all_three_saved_targets() {
    let vectors = [
        (
            FIRST,
            1,
            Target::FirstMain {
                name: "样本".into(),
            },
        ),
        (
            APPEND,
            2,
            Target::Branch {
                name: "side".into(),
                expected_head: [0x44; 16],
            },
        ),
        (
            FORK,
            3,
            Target::HistoricalMain {
                new_branch: "new".into(),
                original: [0x55; 16],
            },
        ),
    ];
    for (raw, sequence, target) in vectors {
        let original = octets(raw);
        let expected = Receipt {
            mutation: Mutation {
                container: [0x11; 16],
                request: [0x22; 16],
            },
            sequence,
            change: Change::Save {
                target,
                node: [0x33; 16],
                body: Body {
                    length: 0x0102_0304_0506_0708,
                    sha256: [0x66; 32],
                },
            },
        };
        assert_eq!(decode(&original).unwrap(), expected);
        assert_eq!(encode(&expected).unwrap(), original);
    }
}

#[test]
fn contradictory_first_names_and_bases_do_not_enter_the_model() {
    // A first display name is mandatory, legal only with an absent base on main.
    let mut first = octets(FIRST);
    first.truncate(first.len() - (2 + "样本".len()));
    first.push(0);
    assert!(decode(&first).is_err());
    let mut named_root = octets(FIRST);
    let branch = named_root
        .windows(4)
        .position(|part| part == b"main")
        .unwrap();
    named_root[branch..branch + 4].copy_from_slice(b"side");
    assert!(decode(&named_root).is_err());
    for base in [APPEND, FORK] {
        let mut extra = octets(base);
        assert_eq!(extra.pop(), Some(0));
        extra.extend_from_slice(&[1, 1, b'x']);
        assert!(decode(&extra).is_err());
    }
}
