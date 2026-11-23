use super::*;

#[test]
fn only_explicit_source_and_three_current_roles_are_accepted() {
    let good = [
        "/old",
        "/zxs/disks/system",
        "/zxs/disks/data",
        "/zxs/disks/recycle",
    ];
    let q = Preflight::parse(good.map(OsString::from).into_iter()).unwrap();
    assert_eq!(q.source, Path::new("/old"));
    assert_eq!(q.roles, [good[1], good[2], good[3]].map(PathBuf::from));
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
