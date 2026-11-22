use super::*;

#[test]
fn health_needs_an_explicit_canonical_number_and_no_other_operand() {
    let parse=|args:&[&str]|HealthQuery::parse(args.iter().map(OsString::from));
    assert!(parse(&[]).is_err());
    assert!(parse(&["--device","0:0"]).is_err());
    assert!(parse(&["--device","0259:1"]).is_err());
    assert!(parse(&["--device","/dev/nvme0n1"]).is_err());
    assert!(parse(&["--device","259:1","--zero"]).is_err());
    assert!(parse(&["--device","259:1","--rates"]).is_err());
    assert_eq!(parse(&["--device","259:1"]).unwrap().0,Dev(259,1));

    for good in ["nvme0n1","nvme20n100"]{assert!(super::super::aliases::nvme_namespace_name(good));}
    for bad in ["nvme0","nvme01n1","nvme0n0","nvme0c0n1","nvme0n1p1","../nvme0n1","nvme0n1\n"]{
        assert!(!super::super::aliases::nvme_namespace_name(bad));
    }
}

#[test]
fn fixed_log_decoding_preserves_large_endurance_and_media_counters() {
    let mut page=[0;LOG_BYTES];
    assert_eq!(Health::decode(&page).unwrap().temperature_kelvin,None);
    page[..6].copy_from_slice(&[3,0x2c,1,93,10,150]); // 300 K, endurance may exceed 100%
    let errors=1u128<<116;
    page[160..176].copy_from_slice(&errors.to_le_bytes());
    let seen=Health::decode(&page).unwrap();
    assert_eq!(seen,Health{warnings:3,temperature_kelvin:Some(300),available_spare:93,
        spare_threshold:10,endurance_used:150,media_errors:errors});
    assert!(seen.to_string().contains("endurance-used-percent=150"));
    page[3]=101;
    assert_eq!(Health::decode(&page),Err(Reason::Invalid));
}
