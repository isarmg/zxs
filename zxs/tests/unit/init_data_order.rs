use super::*;

fn raw(path: &[u8]) -> Vec<u8> {
    let mut raw = b"ZXINITD1".to_vec();
    raw.extend_from_slice(&[1; 32]); // exact existing descriptor
    raw.extend_from_slice(&[2; 32]); // parent session nonce
    raw.extend_from_slice(&[3; 16]);
    raw.extend_from_slice(&[4; 32]);
    raw.push(3);
    for item in [11_u64, 12, 13, 21, 22, 23, 31, 32, 33] {
        raw.extend_from_slice(&item.to_be_bytes());
    }
    raw.extend_from_slice(&(path.len() as u16).to_be_bytes());
    raw.extend_from_slice(path);
    raw
}

#[test]
fn init_order_is_not_an_import_ticket_or_an_unbound_target_name() {
    let valid = raw(b"/data/second");
    let InitSelection {
        scope,
        pool,
        token,
        roles,
        path,
    } = selection(&valid, [1; 32]).unwrap();
    assert_eq!(roles, [[11, 12, 13], [21, 22, 23], [31, 32, 33]]);
    for field in [120..121, 129..137, 137..145, 185..193] {
        let mut zero = valid.clone();
        zero[field].fill(0);
        assert!(selection(&zero, [1; 32]).is_err());
    }
    assert_eq!(
        (scope, pool, token, path),
        ([2; 32], [3; 16], [4; 32], b"/data/second".to_vec())
    );
    assert!(selection(&valid, [7; 32]).is_err());
    for cut in 0..valid.len() {
        assert!(selection(&valid[..cut], [1; 32]).is_err());
    }
    let mut trailing = valid.clone();
    trailing.push(0);
    assert!(selection(&trailing, [1; 32]).is_err());
    for range in [40..72, 72..88, 88..120] {
        let mut zero = valid.clone();
        zero[range].fill(0);
        assert!(selection(&zero, [1; 32]).is_err());
    }
    for path in [
        b"relative".as_slice(),
        b"/",
        b"/a/",
        b"/a/../b",
        b"/a//b",
        b"/a\0b",
    ] {
        assert!(selection(&raw(path), [1; 32]).is_err());
    }
}
