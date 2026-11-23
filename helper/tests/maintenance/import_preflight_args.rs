use super::*;

#[test]
fn only_explicit_source_and_ordered_current_roles_are_accepted() {
    let good = [
        "/old",
        "/zxs/disks/system",
        "/zxs/disks/recycle",
        "/zxs/disks/data-a",
        "/zxs/disks/data-b",
    ];
    let q = Preflight::parse(good.map(OsString::from).into_iter()).unwrap();
    assert_eq!(q.source, Path::new("/old"));
    assert_eq!(
        q.roles,
        good[1..]
            .iter()
            .copied()
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    );
    assert!(Preflight::parse(good[..3].iter().copied().map(OsString::from)).is_err());
    assert!(Preflight::parse(good[..4].iter().copied().map(OsString::from)).is_ok());
    for wrong in ["/", "../source", "/x/../source", "/x//source", "/x/"] {
        let mut args = good;
        args[0] = wrong;
        assert!(Preflight::parse(args.map(OsString::from).into_iter()).is_err());
    }
    let obsolete_proxy_ids = good.into_iter().chain(["2010", "2020"]).map(OsString::from);
    assert!(Preflight::parse(obsolete_proxy_ids).is_err());

    let appended = good.into_iter().chain(["--execute"]).map(OsString::from);
    assert!(Preflight::parse(appended).is_err());
}

#[test]
fn data_roster_length_is_bounded() {
    let args = |count| {
        ["/old", "/system", "/recycle"]
            .into_iter()
            .map(OsString::from)
            .chain((0..count).map(|i| OsString::from(format!("/data-{i}"))))
    };
    assert!(Preflight::parse(args(62)).is_ok());
    assert!(Preflight::parse(args(63)).is_err());
}
