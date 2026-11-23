use super::*;

#[test]
fn only_explicit_source_and_three_current_roles_are_accepted() {
    let good = ["/old", "/zxs/disks/system", "/zxs/disks/data", "/zxs/disks/recycle", "2010", "2020"];
    let q = Preflight::parse(good.map(OsString::from).into_iter()).unwrap();
    assert_eq!(q.source, Path::new("/old"));
    assert_eq!(q.roles[1], Path::new("/zxs/disks/data"));
    for wrong in ["/", "../source", "/x/../source", "/x//source", "/x/"] {
        let mut args = good;
        args[0] = wrong;
        assert!(Preflight::parse(args.map(OsString::from).into_iter()).is_err());
    }
    let appended = good.into_iter().chain(["--execute"]).map(OsString::from);
    assert!(Preflight::parse(appended).is_err());
}
