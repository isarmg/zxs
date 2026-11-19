//! Pure layer/identity/counter examples. These do not claim to have inspected a device.
use super::*;

fn part()->Identity {
    Identity {dev:Dev(259,8),object:Entity{dev:4,ino:222},name:"nvme0n1p7".into(),
        kind:Kind::Partition{number:7,start:2048,parent:Dev(259,0),parent_object:Entity{dev:4,ino:111}},
        sectors:Ok(4096),diskseq:Ok(92)}
}

#[test]
fn only_explicit_numeric_devices_and_an_optional_second_sample_are_accepted() {
    let model=|args:&[&str]|Query::parse(args.iter().map(|&s|OsString::from(s)));
    let d=model(&["--rates","--device","259:8"]).unwrap();
    assert!(d.rates); assert_eq!(d.device,Some(Dev(259,8)));
    let all=model(&[]).unwrap(); assert!(all.device.is_none()&&!all.rates);
    for args in [&["--device","/dev/sda"][..],&["--device","008:0"],&["--device","-1:0"],
        &["--device","0:1"],&["--device","8:0:0"],&["--device","8:4294967296"],
        &["--rates","--rates"],&["--device"], &["--device","8:0","--device","8:1"],&["--repair"]] {
        assert!(model(args).is_err(),"{args:?}");
    }
}

#[test]
fn use_actual_interval_and_512_byte_units_never_device_logical_sector_size_or_sum_of_layers(){
    let id=part();
    let a=Counts::parse(b"100 10 2000 0 40 0 900 0 3 25 99").unwrap();
    let b=Counts::parse(b"102 10 2003 0 41 0 902 0 0 27 100 0 0 0 0 0 0").unwrap();
    let r=rates(&id,&id,a,b,Duration::from_millis(1500)).unwrap();
    assert_eq!(r,Rate{read_bytes:1536,write_bytes:1024,interval_ns:1_500_000_000});
    assert_eq!((r.bps(r.read_bytes),r.bps(r.write_bytes)),("1024".into(),"682".into()));
    let idle=rates(&id,&id,b,b,Duration::from_secs(1)).unwrap();
    assert_eq!(idle.bps(idle.read_bytes),"0");
    let slow=rates(&id,&id,a,Counts{reads:a.reads+1,read_sectors:a.read_sectors+1,..a},Duration::from_secs(600)).unwrap();
    assert_eq!(slow.bps(slow.read_bytes),"<1");
    assert_eq!(slow.bps(slow.write_bytes),"0");
    assert_eq!(rates(&id,&id,b,a,Duration::from_secs(1)),Err(Reason::Reset));
    assert_eq!(rates(&id,&id,a,Counts{reads:0,..b},Duration::from_secs(1)),Err(Reason::Reset));
    assert_eq!(rates(&id,&id,a,a,Duration::ZERO),Err(Reason::Invalid));
    for raw in [&b"1 2 3"[..], b"1 0 3 0 4 0 -1 0 0 0 0",b"0 0 18446744073709551616 0 0 0 0 0 0 0 0"] {
        assert!(Counts::parse(raw).is_err());
    }
    let mut no_generation=id.clone();no_generation.diskseq=Err(Reason::NotReported);
    assert_eq!(rates(&no_generation,&no_generation,a,b,Duration::from_secs(1)),Err(Reason::NotReported));
}

#[test]
fn same_devno_or_even_same_parent_generation_is_not_sufficient_when_a_partition_is_recreated(){
    let a=part();let c=Counts{reads:1,writes:1,read_sectors:10,write_sectors:20};
    let mut new_inode=a.clone();new_inode.object.ino+=1;
    let mut replacement=a.clone();replacement.diskseq=Ok(95);
    let mut start=a.clone(); if let Kind::Partition{start,..}=&mut start.kind {*start+=1;}
    let mut different_parent=a.clone(); if let Kind::Partition{parent_object,..}=&mut different_parent.kind{parent_object.ino+=1;}
    let mut size=a.clone();size.sectors=Ok(4097);
    for changed in [new_inode,replacement,start,different_parent,size] {
        assert_eq!(rates(&a,&changed,c,c,Duration::from_secs(1)),Err(Reason::Changed));
    }
    let unavailable=Report{start_ms:Some(5),end_ms:Some(3),requested_rates:true,
        rows:vec![(a.dev,Ok(Row{id:a,model:Err(Reason::NotReported),fs_uuid:Err(Reason::Unsupported),
            partuuid:Ok("ab12-07".into()),mount_ids:Ok(vec![6,45]),rate:Err(Reason::Reset)}))]};
    let text=unavailable.to_string();
    assert!(text.contains("partuuid_udev_alias=ab12-07")&&text.contains("direct_mount_ids=6,45"));
    assert!(text.contains("sample_unix_ms=unknown")&&text.contains("rate=unknown(counter-reset)"));
    assert!(!unavailable.complete());
}
