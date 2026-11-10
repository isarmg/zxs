use super::*;
#[test]
fn account_selection_is_local_nonroot_unique_uid_and_does_not_return_password_fields() {
    let name = Name::new("zxs".into()).unwrap();
    let source =
        "root:x:0:0:root:/root:/bin/sh\nzxs:x:980:981:service:/zxs/home:/usr/sbin/nologin\n";
    let record = local_record(source, &name).unwrap().unwrap();
    assert_eq!((record.uid, record.gid), (980, 981));
    assert_eq!(record.home, "/zxs/home");
    assert_eq!(record.shell, "/usr/sbin/nologin");
    for source in [
        "zxs:x:0:981::/:/bin/false\n",
        "zxs:x:980:0::/:/bin/false\n",
        "zxs:x:980:981::/:/bin/false\nother:x:980:981::/:/bin/false\n",
        "zxs:x:980:981::/:/bin/false\nzxs:x:982:981::/:/bin/false\n",
        "zxs:malformed\n",
    ] {
        assert!(local_record(source, &name).is_err());
    }
    for value in ["root", "bad\nname", "%i", "name with space", "0abc"] {
        assert!(Name::new(value.into()).is_err());
    }
}

#[test]
fn configuration_group_selection_is_unique_local_and_nonroot() {
    let group = Name::new("zxs".into()).unwrap();
    assert_eq!(
        local_group_record("root:x:0:\nzxs:x:981:\n", &group).unwrap(),
        Some(981)
    );
    for source in [
        "zxs:x:0:\n",
        "zxs:x:981:\nzxs:x:982:\n",
        "zxs:x:981:\nother:x:981:\n",
        "zxs:malformed\n",
        "zxs:x:invalid:\n",
    ] {
        assert!(local_group_record(source, &group).is_err());
    }
}

#[test]
fn missing_local_records_are_not_malformed_records() {
    let name = Name::new("zxs".into()).unwrap();
    assert!(
        local_record("root:x:0:0:root:/root:/bin/sh\n", &name)
            .unwrap()
            .is_none()
    );
    assert_eq!(local_group_record("root:x:0:\n", &name).unwrap(), None);
    assert!(local_record("broken", &name).is_err());
    assert!(local_group_record("broken", &name).is_err());
}
