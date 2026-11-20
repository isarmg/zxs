use super::*;
use sha2::{Digest,Sha256};

fn bytes(v:&str)->Vec<u8>{assert!(v.len().is_multiple_of(2));(0..v.len()).step_by(2).map(|i|u8::from_str_radix(&v[i..i+2],16).unwrap()).collect()}
const SID:&str="010400000000000515000000010000000200000003000000";
fn sample()->Vec<u8>{
    let mut b=b"ZXS-SAM/BOUND-1\0".to_vec();
    b.extend([0x11;16]);b.extend([0x22;16]);b.extend([0x33;16]);b.extend([0x44;32]);
    b.extend(b"ZXSLOCAL");b.extend(bytes(SID));b
}
fn match_install(b:&[u8])->io::Result<LocalDomain>{LocalDomain::expected_binding(b,&[0x11;16],&[0x22;16],&[0x33;16],&[0x44;32])}
#[test]
fn guest_and_explicit_installation_bind_the_same_nonsecret_domain_not_an_account_grant(){
    let mut guest=b"ZXS-SAM/NEW-1\0ZXSLOCAL".to_vec();guest.extend(bytes(SID));
    let a=LocalDomain::guest_bytes(&guest).unwrap();let b=match_install(&sample()).unwrap();
    assert!(a==b);assert_eq!(b.expected_smb_sid().as_slice(),bytes("010500000000000515000000010000000200000003000000e8030000"));
    assert!(match_install(&guest).is_err());assert!(LocalDomain::guest_bytes(&sample()).is_err());
}
#[test]
fn unknown_domains_trailing_data_and_each_different_host_selection_are_rejected(){
    for off in [0,BOUND.len(),BOUND.len()+16,BOUND.len()+32,BOUND.len()+48,BOUND.len()+80,BOUND.len()+80+NAME.len()+1]{
        let mut b=sample();b[off]^=1;assert!(match_install(&b).is_err());
    }
    let mut b=sample();let n=b.len();assert!(match_install(&b[..n-1]).is_err());
    b.extend([0]);assert!(match_install(&b).is_err());let mut b=sample();b[n-12..].fill(0);assert!(match_install(&b).is_err());
    let mut b=sample();b[n-23]=5;b.extend(1000u32.to_le_bytes());assert!(match_install(&b).is_err());
}
#[test]
fn exact_guest_boot_profile_is_not_a_live_service_or_a_systemwide_configuration(){
    let data=include_bytes!("../../resources/samba-domain/domain-bootstrap.conf");
    assert_eq!(Sha256::digest(data).as_slice(),bytes("4f5cc8579cb37cbbf9ccdbc23a46ff40d319765336bf74b33ca4abd0a9160abb"));
    let conf=std::str::from_utf8(data).unwrap();
    assert_eq!(conf.lines().filter(|l|l.starts_with('[')).collect::<Vec<_>>(),["[global]"]);
    for line in ["private dir = /instance/private","state directory = /instance/state","netbios name = ZXSLOCAL",
        "passdb backend = zxs_sqlite","map to guest = Never","usershare max shares = 0"]{
        assert_eq!(conf.lines().map(str::trim).filter(|a|*a==line).count(),1);
    }
}

#[test]
fn default_live_profile_and_original_public_record_are_exact_not_physical_login_proofs(){
    let live=include_bytes!("../../resources/samba-domain/initial-live.conf");
    let sha:[u8;32]=Sha256::digest(live).into();
    assert_eq!(live.len(),1349);
    assert_eq!(sha.as_slice(),bytes("a4724d2af6596303e9bccf5db9456629328ad6c575d88adcd8c55481b57708d2"));
    let text=std::str::from_utf8(live).unwrap();
    assert_eq!(text.lines().filter(|s|s.starts_with('[')).collect::<Vec<_>>(),["[global]"]);
    for key in ["interfaces = 127.0.0.1","bind interfaces only = yes","multicast dns register = no","map to guest = Never","restrict anonymous = 2",
        "server signing = mandatory","ntlm auth = ntlmv2-only","passdb backend = tdbsam:/instance/accountdb/passdb.tdb","nt pipe support = no","host msdfs = no","usershare path =","usershare max shares = 0"]{
        assert_eq!(text.lines().filter(|l|l.trim()==key).count(),1);
    }
    let mut expected=sample();expected[64..96].copy_from_slice(&sha);
    let domain=match_install(&sample()).unwrap();
    let exact=domain.encode_binding(&[0x11;16],&[0x22;16],&[0x33;16],&sha).unwrap();
    assert_eq!(exact.as_slice(),expected);
    assert!(LocalDomain::expected_binding(&exact,&[0x11;16],&[0x22;16],&[0x33;16],&[0x44;32]).is_err());
}
