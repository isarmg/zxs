use super::*;
#[test]
fn standalone_never_converts_legacy_site_input_into_another_start(){
    assert!(reject_legacy_selection(|_|false).is_ok());
    for name in LEGACY_ENV{
        assert_eq!(reject_legacy_selection(|key|key==name).unwrap_err().kind(),io::ErrorKind::Unsupported);
    }
}
