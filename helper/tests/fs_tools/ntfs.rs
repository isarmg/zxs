use super::*;

fn boot(bps:u16,sectors:i64)->[u8;BOOT]{
    let mut b=[0;BOOT];
    b[3..11].copy_from_slice(b"NTFS    ");
    b[11..13].copy_from_slice(&bps.to_le_bytes());
    b[40..48].copy_from_slice(&sectors.to_le_bytes());
    b
}
const NORMAL:&[u8]=b"Mounting volume... OK\nProcessing of $MFT and $MFTMirr completed successfully.\nChecking the alternate boot sector... OK\nNTFS volume version is 3.1.\nNTFS partition /image was processed successfully.\n";

#[test]
fn only_a_whole_ntfs_volume_with_a_sector_after_its_declared_extent_is_admitted(){
    for bps in [512,1024,2048,4096]{
        let bytes=u64::from(bps);
        let b=boot(bps,127);
        assert_eq!(frame(&b,128*bytes).unwrap().declared_bytes,127*bytes);
        assert!(frame(&b,135*bytes).is_ok()); // ntfsfix verifies the actual last sector
        for length in [126*bytes,127*bytes,128*bytes-1,128*bytes+1]{
            assert!(frame(&b,length).is_err());
        }
    }
    for (s,n) in [(0,128),(256,128),(513,128),(512,0),(512,-1),(4096,i64::MAX)]{
        assert!(frame(&boot(s,n),1<<40).is_err());
    }
    assert!(frame(&[0;BOOT],1<<40).is_err());
    assert!(frame(&boot(512,127),MAX_PROBE).is_ok());
    assert!(frame(&boot(512,127),MAX_PROBE+512).is_err());
}

#[test]
fn zero_requires_first_ro_success_backup_success_supported_version_and_complete_end(){
    let e=frame(&boot(512,127),65536).unwrap();
    assert!(classify(0,NORMAL,b"",e).observed);
    for accepted in [b"1.1".as_slice(),b"1.2",b"2.0",b"2.255",b"3.0",b"3.1"]{assert!(native_version(accepted));}
    for rejected in [b"".as_slice(),b"1.0",b"2.01",b"2.256",b"3.2",b"4.0",b"2.-1",b"2.1\r"]{assert!(!native_version(rejected));}
    for replacement in [
        ("Mounting volume... OK","Mounting volume... FAILED"),
        ("Checking the alternate boot sector... OK","Checking the alternate boot sector... BAD"),
        ("Checking the alternate boot sector... OK","Checking file system overflow... OK"),
        ("NTFS volume version is 3.1.","NTFS volume version is 3.2."),
        ("partition /image","partition /some/untrusted/path"),
    ]{
        let bad=std::str::from_utf8(NORMAL).unwrap().replace(replacement.0,replacement.1);
        assert_eq!(classify(0,bad.as_bytes(),b"",e).exit(),2);
    }
    for (code,out,err) in [(1,NORMAL,b"".as_slice()),(0,NORMAL,b"journal warning"),
                          (0,&NORMAL[..NORMAL.len()-1],b""),(0,END,b"")]{
        assert_eq!(classify(code,out,err,e).exit(),2);
    }
    assert!(!complete(&[NORMAL,b"Failed to unmount partition\n"].concat(),b""));
}

#[test]
fn wire_is_bound_to_selected_format_digest_lengths_and_actual_worker_exit(){
    let e=frame(&boot(512,127),65536).unwrap();
    let sha=[0x12;32];let p=classify(0,NORMAL,b"",e).packet(&sha);
    assert_eq!(Report::from_packet(&p,Some(0),65536,&sha).unwrap().exit(),0);
    assert!(Report::from_packet(&p,Some(3),65536,&sha).is_err());
    assert!(Report::from_packet(&p,None,65536,&sha).is_err());
    assert!(Report::from_packet(&p,Some(0),65536,&[0;32]).is_err());
    assert!(Report::from_packet(&p,Some(0),65536+512,&sha).is_err());
    let mut bad=p;bad[..4].copy_from_slice(b"XFR1");assert!(Report::from_packet(&bad,Some(0),65536,&sha).is_err());
    let mut bad=p;bad[17..25].copy_from_slice(&65536u64.to_be_bytes());assert!(Report::from_packet(&bad,Some(0),65536,&sha).is_err());
    let failure=classify(0,END,b"",e).packet(&sha);
    assert_eq!(Report::from_packet(&failure,Some(2),65536,&sha).unwrap().exit(),2);
    assert!(Report::from_packet(&failure,Some(0),65536,&sha).is_err());
}
