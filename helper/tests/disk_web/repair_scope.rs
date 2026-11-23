use super::*;
#[test]fn only_the_fixed_format_and_bounded_size_from_the_same_scoped_offer(){
    let raw=|format:u8,bytes:u64|[TAG,&[7;32],&[format],&bytes.to_be_bytes()].concat();
    for format in [RepairFormat::Ext4,RepairFormat::Exfat,RepairFormat::Fat32]{
        assert_eq!(raw_scope(&raw(format.scope_code(),i64::MAX as u64)).unwrap(),(format,i64::MAX as u64));
    }
    for n in [0,u64::MAX]{assert!(raw_scope(&raw(1,n)).is_err());}
    for format in [0,4,u8::MAX]{assert!(raw_scope(&raw(format,1)).is_err());}
    let mut payload=raw(1,1);payload.push(0);assert!(raw_scope(&payload).is_err());
    assert!(raw_scope(&[TAG,&[0;32],&[1],&1u64.to_be_bytes()].concat()).is_err());
}
