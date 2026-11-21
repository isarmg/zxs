use super::*;
use std::os::unix::ffi::OsStringExt;

#[test]
fn only_exact_argv0_selects_the_gated_main_binary_role() {
    let check = |args: &[&str]| selected(&mut args.iter().copied().map(OsString::from));
    assert_eq!(check(&["zxs:internal-smb-311"]), Some(Ok(Role::Server)));
    assert_eq!(check(&["zxs:internal-smb-311", ""]), Some(Err(Fault::Shape)));
    assert_eq!(check(&["zxs:internal-smb-preflight-311"]), Some(Ok(Role::Preflight)));
    assert_eq!(check(&["zxs:internal-smb-preflight-311", ""]), Some(Err(Fault::Shape)));
    assert_eq!(check(&["zxs", "zxs:internal-smb-preflight-311"]), None);
    assert_eq!(check(&["/path/zxs:internal-smb-preflight-311"]), None);
    assert_eq!(check(&["zxs:internal-web-1"]), Some(Ok(Role::Web)));
    assert_eq!(check(&["zxs:internal-web-1", ""]), Some(Err(Fault::Shape)));
    assert_eq!(check(&["zxs", "zxs:internal-web-1"]), None);
    assert_eq!(check(&["/usr/bin/zxs"]), None);
    assert_eq!(check(&["zxs", "zxs:internal-smb-311"]), None);
    assert_eq!(check(&["zxs-rust-smb"]), None);
    assert_eq!(check(&["/path/zxs:internal-smb-311"]), None);
    assert_eq!(selected(&mut [OsString::from_vec(vec![0xff])].into_iter()), None);
}
