use super::*;
#[test]
fn exact_pair_body_and_exact_seal_transitions(){
    let nonce=[1;32];let cohort=[2;32];let expected=body(nonce,cohort).unwrap();
    assert_eq!(&expected[..16],b"ZXS-ADMIT-PAIR1\0");
    assert_eq!(&expected[16..48],&nonce);
    assert_eq!(&expected[48..80],&cohort);
    let sha=digest(&expected).unwrap();
    assert!(matches_body(&expected,&expected,sha).is_ok());
    assert!(matches_body(&expected,&expected,[0;32]).is_err());
    for ix in [0usize,15,16,47,48,79]{
        let mut foreign=expected;foreign[ix]^=1;
        assert!(matches_body(&foreign,&expected,digest(&foreign).unwrap()).is_err());
    }
    assert!(body([0;32],cohort).is_err());
    assert!(body(nonce,[0;32]).is_err());
    assert_eq!(seal_state(HELD),Ok(SealState::Held));
    assert_eq!(seal_state(RELEASED),Ok(SealState::Released));
    for malformed in [0,libc::F_SEAL_EXEC,libc::F_SEAL_SEAL,HELD&!libc::F_SEAL_WRITE,RELEASED|libc::F_SEAL_FUTURE_WRITE]{
        assert_eq!(seal_state(malformed),Err(Fault::Handoff));
    }
}
