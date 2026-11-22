use super::*;
#[test]
fn one_fixed_deployment_selects_only_nonzero_original_instance_and_complete_record(){
    let mut bytes=[0;SIZE];bytes[..16].copy_from_slice(MAGIC);
    assert!(Selected::decode(&bytes).is_err());
    bytes[16..32].fill(3);
    assert!(Selected::decode(&bytes).is_err());
    bytes[32..].fill(4);
    assert_eq!(Selected::decode(&bytes).unwrap(),Selected{instance:[3;16],record:[4;32]});
    assert!(Selected::decode(&bytes[..SIZE-1]).is_err());
    assert!(Selected::decode(&[bytes.as_slice(),&[0]].concat()).is_err());
    bytes[0]=0;assert!(Selected::decode(&bytes).is_err());
}
