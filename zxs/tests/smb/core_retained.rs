use super::*;

#[test]
fn retained_round_spelling_keeps_the_complete_sql_cursor_in_order(){
    let mut first=[0x7f;16];first[0]=0x00;
    let mut next=first;next[15]=0x80;
    let a=OperationId(first);let b=OperationId(next);
    let an=a.to_string();let bn=b.to_string();
    assert!(an.as_bytes()<bn.as_bytes());
    assert_eq!(delete_id(an.as_bytes()).unwrap(),a);
    assert_eq!(delete_id(bn.as_bytes()).unwrap(),b);
    for invalid in ["0".repeat(32),"f".repeat(32),"a".repeat(31),an.to_uppercase(),format!("{}\\x",bn)]{
        assert!(delete_id(invalid.as_bytes()).is_err());
    }
}
