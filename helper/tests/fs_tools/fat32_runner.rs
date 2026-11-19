use super::*;
use crate::fat32_check::Outcome;

const GOOD:&[u8]=b"fsck.fat 4.2 (2021-01-31)\n/image: 1 files, 1/65526 clusters\n";
const DELIVERED:&[u8]=b"{ \"child-pid\": 52 }\n{ \"exit-code\": 0 }\n";
#[test]
fn launcher_failure_or_truncated_report_is_never_a_completed_disk_check(){
    assert_eq!(classify(0,GOOD,b"",DELIVERED,65526).unwrap().state,Outcome::NoFindingsReported);
    assert_eq!(classify(0,GOOD,CP850_FALLBACK,DELIVERED,65526).unwrap().state,Outcome::NoFindingsReported);
    let mut added=CP850_FALLBACK.to_vec();added.extend_from_slice(b"extra diagnostic\n");
    assert_eq!(classify(0,GOOD,&added,DELIVERED,65526).unwrap().state,Outcome::ReportedMessages);
    for s in [&b""[..], b"{ \"child-pid\": 52 }\n", b"{ \"exit-code\": 2 }\n", b"{ \"exit-code\": 0 }\n{ \"exit-code\": 0 }\n"]{
        assert!(classify(0,GOOD,b"",s,65526).is_err());
    }
    assert!(classify(0,&GOOD[..GOOD.len()-1],b"",DELIVERED,65526).is_err());
    assert!(classify(0,GOOD,b"",DELIVERED,65525).is_err());
    assert!(classify(1,b"fsck.fat 4.2 (2021-01-31)\n",b"Failed to read sector 73",b"{ \"exit-code\": 1 }\n",65526).is_err());
    assert!(classify(1,GOOD,b"wrong volume counts",b"{ \"exit-code\": 1 }\n",65525).is_err());
    assert!(classify(1,b"",b"bwrap failed",b"{ \"exit-code\": 1 }\n",65526).is_err());
    assert!(classify(9,GOOD,b"",b"{ \"exit-code\": 9 }\n",65526).is_err());
}
#[test]
fn ambiguous_checker_status_and_extra_output_are_not_certificates_of_health(){
    assert_eq!(classify(1,GOOD,b"internal error",b"{ \"exit-code\": 1 }\n",65526).unwrap().state,Outcome::FindingsOrToolError);
    assert_eq!(classify(0,GOOD,b"notice from tool",DELIVERED,65526).unwrap().state,Outcome::ReportedMessages);
    let noisy=b"fsck.fat 4.2 (2021-01-31)\nuser-controlled\x1b[2J\n/image: 1 files, 1/65526 clusters\n";
    let verdict=classify(0,noisy,b"",DELIVERED,65526).unwrap();
    assert_eq!(verdict.state,Outcome::ReportedMessages);
    assert_eq!((verdict.files,verdict.used,verdict.total),(1,1,65526));
    assert!(!verdict.to_string().contains("user-controlled"));
    let mut v=Vec::new();assert!(drain(&mut &b"abcd"[..],&mut v,3).is_err());
}
#[test]
fn accepts_only_an_explicit_absolute_image_and_exact_independent_digest(){
    use crate::fat32_check::Request;
    let parse=|a:&[&str]|Request::parse(a.iter().map(|s|std::ffi::OsString::from(*s)));
    let d="af".repeat(32);
    assert!(parse(&["/img","--sha256",&d]).is_ok());
    for a in [&["/img","--sha256","none"][..], &["img","--sha256",&d],&["/img","--sha256",&d,"-a"],&["/img","-r",&d]]{
        assert!(parse(a).is_err());
    }
}
