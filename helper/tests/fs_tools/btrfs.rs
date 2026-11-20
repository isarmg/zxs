use super::*;
const SIZE:u64=1<<35;
const ID:[u8;16]=[0x01,0x23,0x45,0x67,0x89,0xab,0x4d,0xef,0x80,0x12,0x34,0x56,0x78,0x9a,0xbc,0xde];
fn put(sb:&mut [u8;SUPER_BYTES],n:usize,v:u64){sb[n..n+8].copy_from_slice(&v.to_le_bytes());}
fn primary()->[u8;SUPER_BYTES]{
    let mut sb=[0;SUPER_BYTES];sb[32..48].copy_from_slice(&ID);sb[64..72].copy_from_slice(b"_BHRfS_M");
    put(&mut sb,48,PRIMARY);put(&mut sb,112,SIZE);put(&mut sb,136,1);put(&mut sb,201,9);put(&mut sb,209,SIZE);
    put(&mut sb,180,FREE_SPACE_TREE);sb
}
const OUT:&[u8]=b"Opening filesystem to check...\nChecking filesystem on /image\nUUID: 01234567-89ab-4def-8012-3456789abcde\nfound 98304 bytes used, no error found\ntotal csum bytes: 0\ntotal tree bytes: 147456\ntotal fs tree bytes: 32768\ntotal extent tree bytes: 16384\nbtree space waste bytes: 128542\nfile data blocks allocated: 0\n referenced 0\n";
const PHASES:&[u8]=b"[1/8] checking log skipped (none written)\n[2/8] checking root items\n[3/8] checking extents\n[4/8] checking free space tree\n[5/8] checking fs roots\n[6/8] checking only csums items (without verifying data)\n[7/8] checking root refs\n[8/8] checking quota groups skipped (not enabled on this FS)\n";
#[test]
fn primary_admission_is_one_complete_ordinary_device_not_one_member_of_a_set(){
    let sb=primary();
    assert_eq!(frame(&sb,SIZE+4096).unwrap().device_bytes,SIZE);
    assert!(frame(&sb,SIZE-1).is_err());
    for (pos,value) in [(48,64<<20),(136,2),(136,0),(56,SEEDING),(56,METADUMP),(56,METADUMP_V2),
                        (188,ZONED),(209,SIZE-4096),(112,0)] {
        let mut bad=sb;put(&mut bad,pos,value);assert!(frame(&bad,SIZE).is_err());
    }
    for bit in [35,36,38,39,40]{
        let mut pending=sb;put(&mut pending,56,1u64<<bit);
        assert!(frame(&pending,SIZE).is_err());
    }
    let mut settled=sb;put(&mut settled,188,1<<10); // METADATA_UUID is not excluded
    assert!(frame(&settled,SIZE).is_ok());
}

#[test]
fn zero_exit_with_missing_device_or_partial_phases_never_confirms_the_structure_check(){
    let mut sb=primary();
    let e=frame(&sb,SIZE).unwrap();
    assert!(classify(0,OUT,PHASES,e).completed);
    assert!(!classify(1,OUT,PHASES,e).completed);
    for (needle,replacement) in [
        ("Checking filesystem on /image","Checking filesystem on /other"),
        ("01234567-89ab-4def-8012-3456789abcde","11234567-89ab-4def-8012-3456789abcde"),
        ("no error found","error(s) found"),
        (" referenced 0\n",""),("total tree bytes: 147456","total tree bytes: 18446744073709551616"),
    ]{
        let wrong=std::str::from_utf8(OUT).unwrap().replace(needle,replacement);
        assert!(!classify(0,wrong.as_bytes(),PHASES,e).completed);
    }
    let noisy=[b"warning, device 2 is missing\n".as_slice(),OUT].concat();
    assert!(!classify(0,&noisy,PHASES,e).completed);
    let qgroups=[b"Qgroup rescan is running, qgroups will not be printed.\n".as_slice(),OUT].concat();
    assert!(!classify(0,&qgroups,PHASES,e).completed);
    assert!(!classify(0,OUT,&PHASES[..PHASES.len()-1],e).completed);
    let varied=std::str::from_utf8(PHASES).unwrap()
        .replace("checking log skipped (none written)","checking log")
        .replace("free space tree","free space cache")
        .replace("checking quota groups skipped (not enabled on this FS)","checking quota groups");
    assert!(!classify(0,OUT,varied.as_bytes(),e).completed);
    put(&mut sb,96,1<<20);put(&mut sb,180,0);
    assert!(classify(0,OUT,varied.as_bytes(),frame(&sb,SIZE).unwrap()).completed);
}
#[test]
fn receipt_from_this_original_lease_needs_exact_digest_length_format_and_real_exit(){
    let e=frame(&primary(),SIZE).unwrap();
    let expected=[0x38;32];let p=classify(0,OUT,PHASES,e).packet(&expected);
    assert_eq!(Report::from_packet(&p,Some(0),SIZE,&expected).unwrap().exit(),0);
    assert!(Report::from_packet(&p,Some(3),SIZE,&expected).is_err());
    assert!(Report::from_packet(&p,None,SIZE,&expected).is_err());
    assert!(Report::from_packet(&p,Some(0),SIZE,&[0;32]).is_err());
    assert!(Report::from_packet(&p,Some(0),SIZE+4096,&expected).is_err());
    let mut other=p;other[..4].copy_from_slice(b"NFR1");
    assert!(Report::from_packet(&other,Some(0),SIZE,&expected).is_err());
    let unconfirmed=classify(0,OUT,b"",e).packet(&expected);
    assert_eq!(Report::from_packet(&unconfirmed,Some(2),SIZE,&expected).unwrap().exit(),2);
}
