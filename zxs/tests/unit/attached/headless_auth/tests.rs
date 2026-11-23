use super::*;

#[test]
fn native_peer_credentials_observe_both_ends_of_a_private_socket_pair() {
    let (left, right) = UnixStream::pair().unwrap();
    let expected = crate::fs::local_uid();
    assert_eq!(peer_uid(&left).unwrap(), expected);
    assert_eq!(peer_uid(&right).unwrap(), expected);
}

fn raw(data: &[u8], recycle: &[u8]) -> Vec<u8> {
    raw_roots([b"/zxs/disks/system", data, recycle])
}
fn raw_roots(roots: [&[u8]; 3]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend([1; 16]);
    out.extend([2; 16]);
    out.extend(4020u32.to_be_bytes());
    out.extend([3; 16]);
    out.extend([4; 32]);
    out.push(4);
    out.extend(b"user");
    for (i, root) in roots.into_iter().enumerate() {
        out.extend(u16::try_from(root.len()).unwrap().to_be_bytes());
        out.extend(root);
        out.extend([u8::try_from(i).unwrap() + 7; 32]);
    }
    out
}

#[test]
fn fresh_descriptor_rejects_extra_wrong_or_ambiguous_fields() {
    let first = raw(b"/zxs/disks/data/opaque", b"/zxs/disks/recycle/opaque");
    let selected = Descriptor::parse(&first).unwrap();
    let (roots, _) = selected.into_parts();
    assert_eq!(roots.data.path(), Path::new("/zxs/disks/data/opaque"));
    assert_eq!(roots.recycle.path(), Path::new("/zxs/disks/recycle/opaque"));
    for cut in 0..first.len() {
        assert!(Descriptor::parse(&first[..cut]).is_err());
    }
    let mut new = first.clone();
    new.push(0);
    assert!(Descriptor::parse(&new).is_err());
    new = first;
    new[MAGIC.len() + 32..MAGIC.len() + 36].fill(0);
    assert!(Descriptor::parse(&new).is_err());
    for path in [
        b"relative".as_slice(),
        b"/",
        b"/data/",
        b"/data//file",
        b"/data/./file",
        b"/data/../file",
        b"/data/\x00",
    ] {
        assert!(Descriptor::parse(&raw(path, b"/recycle")).is_err());
        assert!(Descriptor::parse(&raw(b"/data", path)).is_err());
    }
    assert_eq!(roots.system.path(), Path::new("/zxs/disks/system"));
    assert!(Descriptor::parse(&raw_roots([b"/system", b"/system/alias", b"/other"])).is_err());
    let mut duplicate = raw(b"/data", b"/recycle");
    let start = duplicate.len() - 32;
    duplicate[start..].fill(8);
    assert!(Descriptor::parse(&duplicate).is_err());
    for (data, recycle) in [
        (b"/same".as_slice(), b"/same".as_slice()),
        (b"/one".as_slice(), b"/one/child".as_slice()),
        (b"/one/child".as_slice(), b"/one".as_slice()),
    ] {
        assert!(Descriptor::parse(&raw(data, recycle)).is_err());
    }
}

#[test]
fn exact_basic_header_and_canonical_padding_only() {
    let exact = ("Authorization", "bAsIc dXNlcjpwYXNz");
    assert_eq!(
        credentials(&[exact]).unwrap().decoded.as_bytes(),
        b"user:pass"
    );
    assert_eq!(
        credentials(&[("Authorization", "Basic YTpiYw==")])
            .unwrap()
            .decoded
            .as_bytes(),
        b"a:bc"
    );
    assert_eq!(
        credentials(&[("Authorization", "Basic YTpiY2Q=")])
            .unwrap()
            .decoded
            .as_bytes(),
        b"a:bcd"
    );
    assert_eq!(
        credentials(&[("Authorization", "Basic YTpiYzp4")])
            .unwrap()
            .decoded
            .as_bytes(),
        b"a:bc:x"
    );
    assert_eq!(
        credentials(&[("x-forwarded-user", "user")]).err(),
        Some(AuthError::Invalid)
    );
    assert_eq!(
        credentials(&[exact, ("authorization", exact.1)]).err(),
        Some(AuthError::Invalid)
    );
    for value in [
        "Bearer dXNlcjpwYXNz",
        "Basic  dXNlcjpwYXNz",
        "Basic YTpiYx==",
        "Basic YTpiY2R=",
        "Basic YTpiYw==YQ==",
        "Basic YTp9_",
        "Basic YToA",
        "Basic ",
    ] {
        assert_eq!(
            credentials(&[("Authorization", value)]).err(),
            Some(AuthError::Invalid),
            "{value}"
        );
    }
}
