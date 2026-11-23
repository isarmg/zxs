use super::*;

fn raw(data: &[u8], recycle: &[u8]) -> Vec<u8> {
    raw_roots([b"/zxs/disks/system", data, recycle])
}
fn raw_roots(roots: [&[u8]; 3]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend([1; 16]);
    out.extend([2; 16]);
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
    let deployment_fields = MAGIC.len() + 32;
    let mut with_uid = first.clone();
    drop(with_uid.splice(deployment_fields..deployment_fields, 4020u32.to_be_bytes()));
    assert!(Descriptor::parse(&with_uid).is_err());
    drop(with_uid.splice(..MAGIC.len(), b"ZXS-HEADLESS-1\0".iter().copied()));
    assert!(Descriptor::parse(&with_uid).is_err());
    new = first;
    new[deployment_fields..deployment_fields + 16].fill(0);
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
fn direct_request_authenticates_the_original_live_account() {
    // The same independent Argon2id vector used in the password fixtures.
    let salt = b"\x1a\x2b\x3c\x4d\x5e\x6f\x70\x81\x92\xa3\xb4\xc5\xd6\xe7\xf8\x09";
    let tag = b"\xcf\x7e\x98\x79\xa1\x40\x23\x3c\x9a\x38\xa1\x60\x19\xd0\x3d\xfc\xd1\x27\xd1\x85\x53\x86\xde\xfe\x40\xfa\x2f\x39\xab\x83\xe8\xc2";
    let mut bytes = raw(b"/data", b"/recycle");
    let offset = MAGIC.len() + 32;
    bytes[offset..offset + 16].copy_from_slice(salt);
    bytes[offset + 16..offset + 48].copy_from_slice(tag);
    let (_, account) = Descriptor::parse(&bytes).unwrap().into_parts();
    let (_, another) = Descriptor::parse(&bytes).unwrap().into_parts();
    let exact = [(
        "Authorization",
        "Basic dXNlcjpmcmVzaCB3b3Jrc3BhY2UgdGVzdCBzZWNyZXQ=",
    )];
    let other_login = [(
        "Authorization",
        "Basic b3RoZXI6ZnJlc2ggd29ya3NwYWNlIHRlc3Qgc2VjcmV0",
    )];
    assert_eq!(
        account.authenticate(&other_login).err(),
        Some(AuthError::Invalid)
    );
    assert_eq!(
        account
            .authenticate(&exact)
            .unwrap()
            .with_admission(&another, |_| ()),
        Err(AuthError::Unavailable)
    );
    account
        .authenticate(&exact)
        .unwrap()
        .with_admission(&account, |allowed| {
            assert!(allowed());
            account.stop_admission();
            assert!(!allowed());
        })
        .unwrap();
    assert_eq!(
        account.authenticate(&exact).err(),
        Some(AuthError::Unavailable)
    );
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
