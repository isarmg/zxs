use super::*;

const VECTOR:&str=include_str!("rust-install-v1.txt");
fn row(key:&str)->&'static str{
    VECTOR.lines().filter_map(|l|l.split_once('=')).find(|(k,_)|*k==key).unwrap().1
}
fn bytes(hex:&str)->Vec<u8>{
    (0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect()
}
#[test]
fn independently_bound_first_scope_keeps_actual_routes_and_separate_family_ids(){
    let raw=bytes(row("wire_hex"));
    let p=RustPlan::decode(&raw).unwrap();
    assert_eq!(p.wire(),raw.as_slice());
    assert_eq!(p.wire().len(),row("bytes").parse::<usize>().unwrap());
    assert_eq!(p.sha().as_slice(),bytes(row("install_sha256")).as_slice());
    assert_eq!(p.host(),"vault-lan.example");
    assert_eq!(p.origin.bind,[4;16]);
    assert_eq!(p.endpoints(),[Endpoint::named("192.0.2.17/24").unwrap(),Endpoint::named("2001:db8:7::2/64").unwrap()]);
    assert_eq!(p.recycle().as_str(),"回收站");
    for (i,(f,ids)) in p.folders().iter().enumerate(){
        assert_eq!(f.name,["Docs","docs"][i]);
        for (key,id) in [("share",ids.share),("publish",ids.publish),("mode",ids.setting)]{
            assert_eq!(id.as_slice(),bytes(row(&format!("f{}_{key}",i+1))).as_slice());
        }
    }
    // The retired family has no remaining parser or maintainable fixture.
    let mut foreign=raw.clone();foreign[0]^=1;
    assert!(RustPlan::decode(&foreign).is_err());
}
#[test]
fn no_normalization_trailing_data_or_ambiguous_address_spelled_as_host(){
    let raw=bytes(row("wire_hex"));
    assert!(RustPlan::decode(&[raw.as_slice(),b"x"].concat()).is_err());
    for h in ["","-lan","lan-","Lan","lan.","192.0.2.17","123","lan..retail","a123456789012345"]{
        assert!(!valid_host(h),"{h}");
    }
    assert!(valid_host("lan"));assert!(valid_host("zxs-7.office"));
    let mut edited=raw;edited[97]=b'V';
    assert!(RustPlan::decode(&edited).is_err());
}

#[test]
fn agrees_with_the_independent_single_ordinary_sample(){
    let raw=bytes(concat!(
        "5a58532d525553542d534d422d49310011111111111111111111111111111111",
        "1212121212121212121212121212121216161616161616161616161616161616",
        "1717171717171717171717171717171713131313131313131313131313131313",
        "0b666978747572652e6c616e010418c000021101333333333333333333333333",
        "33333333020004446f63730006e59b9ee694b6"
    ));
    let p=RustPlan::decode(&raw).unwrap();
    assert_eq!(p.sha().as_slice(),bytes("e06d3be19b79dbbc03395e297938370a1aa8f66f9c680b92db721823fb1e8e00").as_slice());
    let (f,ids)=&p.folders()[0];
    assert_eq!(f.name,"Docs");assert_eq!(f.mode.code(),2);
    assert_eq!(ids.share.as_slice(),bytes("b5203c873ae8c37befe82574a4f157c5").as_slice());
    assert_eq!(ids.publish.as_slice(),bytes("e9653a5d2d051054c2496eb0a19d3672").as_slice());
    assert_eq!(ids.setting.as_slice(),bytes("9b95192b020c378ba058a588d3026df2").as_slice());
}
