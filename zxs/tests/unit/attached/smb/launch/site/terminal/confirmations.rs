use super::*;
#[test]
fn same_foreground_confirmation_accepts_only_the_original_line(){
    let command=format!("{RUN_PREFIX}{}","42".repeat(16));
    let accepted=format!("{command}\n");
    assert_eq!(accepted.len(),CONFIRM_MAX);
    assert!(confirmation(accepted.as_bytes(),&command).is_ok());
    for invalid in [command.clone(),format!("{command}\r\n"),format!("{command} \n"),
        format!("{command} WEB-HTTPS https://admin.example sha256:{}\n","35".repeat(32)),
        format!("{command}\nextra\n"),format!("RUN-ZXS {}\n","43".repeat(16))]{
        assert!(confirmation(invalid.as_bytes(),&command).is_err());
    }
    assert!(confirmation(b"\xff\n",&command).is_err());
}
