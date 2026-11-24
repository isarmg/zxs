use super::*;

fn raw(data: &[u8], recycle: &[u8]) -> Vec<u8> {
    raw_roots([b"/zxs/disks/system", data, recycle])
}
fn raw_roots([system, data, recycle]: [&[u8]; 3]) -> Vec<u8> {
    raw_roster(system, recycle, &[data])
}

fn raw_roster(system: &[u8], recycle: &[u8], data: &[&[u8]]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend([1; 16]);
    out.extend([2; 16]);
    out.extend([3; 16]);
    out.extend([4; 32]);
    out.push(4);
    out.extend(b"user");
    out.push(u8::try_from(data.len()).unwrap());
    for (i, root) in [system, recycle]
        .into_iter()
        .chain(data.iter().copied())
        .enumerate()
    {
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
    assert_eq!(roots.data.len(), 1);
    assert_eq!(roots.data[0].path(), Path::new("/zxs/disks/data/opaque"));
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
fn descriptor_requires_exact_ordered_nonempty_data_roster() {
    let raw = raw_roster(b"/system", b"/recycle", &[b"/disk-b", b"/disk-a"]);
    let (roots, _) = Descriptor::parse(&raw).unwrap().into_parts();
    assert_eq!(roots.system.path(), Path::new("/system"));
    assert_eq!(roots.recycle.path(), Path::new("/recycle"));
    assert_eq!(roots.data.len(), 2);
    assert_eq!(roots.data[0].path(), Path::new("/disk-b"));
    assert_eq!(roots.data[0].token, [9; 32]);
    assert_eq!(roots.data[1].path(), Path::new("/disk-a"));
    assert_eq!(roots.data[1].token, [10; 32]);

    let count = MAGIC.len() + 16 + 16 + 16 + 32 + 1 + b"user".len();
    for bad in [0, 1, 3, 63, 255] {
        let mut changed = raw.clone();
        changed[count] = bad;
        assert!(Descriptor::parse(&changed).is_err());
    }
    let mut singleton_magic = raw;
    drop(singleton_magic.splice(..MAGIC.len(), b"ZXS-HTTP\0".iter().copied()));
    assert!(Descriptor::parse(&singleton_magic).is_err());

    let maximum = (0..MAX_DATA_ROOTS)
        .map(|i| format!("/data-{i}"))
        .collect::<Vec<_>>();
    let paths = maximum.iter().map(String::as_bytes).collect::<Vec<_>>();
    assert!(Descriptor::parse(&raw_roster(b"/system", b"/recycle", &paths)).is_ok());
    assert!(Descriptor::parse(&raw_roster(b"/system", b"/recycle", &[])).is_err());
}

#[test]
fn descriptor_rejects_data_paths_or_tokens_aliasing_any_role() {
    for paths in [
        [b"/data".as_slice(), b"/data".as_slice()],
        [b"/data".as_slice(), b"/data/sub".as_slice()],
        [b"/data".as_slice(), b"/system/sub".as_slice()],
        [b"/data".as_slice(), b"/recycle".as_slice()],
    ] {
        assert!(Descriptor::parse(&raw_roster(b"/system", b"/recycle", &paths)).is_err());
    }
    for duplicate in [7, 8, 9] {
        let mut raw = raw_roster(b"/system", b"/recycle", &[b"/data1", b"/data2"]);
        let token = raw.len() - 32;
        raw[token..].fill(duplicate);
        assert!(Descriptor::parse(&raw).is_err());
    }
}

pub(crate) const FIXTURE_BASIC: &str = "Basic dXNlcjpmcmVzaCB3b3Jrc3BhY2UgdGVzdCBzZWNyZXQ=";

pub(crate) fn fixture_account() -> Account {
    // The same independent Argon2id vector used in the password fixtures.
    let salt = b"\x1a\x2b\x3c\x4d\x5e\x6f\x70\x81\x92\xa3\xb4\xc5\xd6\xe7\xf8\x09";
    let tag = b"\xcf\x7e\x98\x79\xa1\x40\x23\x3c\x9a\x38\xa1\x60\x19\xd0\x3d\xfc\xd1\x27\xd1\x85\x53\x86\xde\xfe\x40\xfa\x2f\x39\xab\x83\xe8\xc2";
    let mut bytes = raw(b"/data", b"/recycle");
    let offset = MAGIC.len() + 32;
    bytes[offset..offset + 16].copy_from_slice(salt);
    bytes[offset + 16..offset + 48].copy_from_slice(tag);
    Descriptor::parse(&bytes).unwrap().into_parts().1
}

#[test]
fn direct_request_authenticates_the_original_live_account() {
    let account = fixture_account();
    let another = fixture_account();
    let exact = [("Authorization", FIXTURE_BASIC)];
    let other_login = [(
        "Authorization",
        "Basic b3RoZXI6ZnJlc2ggd29ya3NwYWNlIHRlc3Qgc2VjcmV0",
    )];
    assert_eq!(
        account.authenticate(&other_login, &|| false).err(),
        Some(AuthError::Invalid)
    );
    assert_eq!(
        account
            .authenticate(&exact, &|| false)
            .unwrap()
            .with_admission(&another, |_| ()),
        Err(AuthError::Unavailable)
    );
    account
        .authenticate(&exact, &|| false)
        .unwrap()
        .with_admission(&account, |allowed| {
            assert!(allowed());
            account.stop_admission();
            assert!(!allowed());
        })
        .unwrap();
    assert_eq!(
        account.authenticate(&exact, &|| false).err(),
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
