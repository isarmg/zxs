use super::*;

const VECTOR:&str=include_str!("rust-install-v2.txt");
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
    assert_eq!(p.recycle().as_str(),"回收站");
    for (i,(f,ids)) in p.folders().iter().enumerate(){
        assert_eq!(f.name,["Docs","docs"][i]);
        for (key,id) in [("share",ids.share),("publish",ids.publish),("mode",ids.setting)]{
            assert_eq!(id.as_slice(),bytes(row(&format!("f{}_{key}",i+1))).as_slice());
        }
    }
    let mut old=raw.clone();old[14]=b'1';
    assert!(RustPlan::decode(&old).is_err());
    // A former endpoint row is not accepted even with the current tag.
    let at=97+usize::from(raw[96]);
    let with_address=[&raw[..at],&[1,4,24,192,0,2,17],&raw[at..]].concat();
    assert!(RustPlan::decode(&with_address).is_err());
}
#[test]
fn preserves_hostname_and_rejects_trailing_or_noncanonical_bytes(){
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
        "5a58532d525553542d534d422d493200111111111111111111111111111111",
        "11121212121212121212121212121212121616161616161616161616161616",
        "16161717171717171717171717171717171713131313131313131313131313",
        "1313130b666978747572652e6c616e01333333333333333333333333333333",
        "33020004446f63730006e59b9ee694b6"
    ));
    let p=RustPlan::decode(&raw).unwrap();
    assert_eq!(p.sha().as_slice(),bytes("17e08c7c898fdfeef4da8c07f05191002ea4720086719cfed85a707b7c21b764").as_slice());
    let (f,ids)=&p.folders()[0];
    assert_eq!(f.name,"Docs");assert_eq!(f.mode.code(),2);
    assert_eq!(ids.share.as_slice(),bytes("c94640448309832bfd618d007462ae28").as_slice());
    assert_eq!(ids.publish.as_slice(),bytes("f91393b8c5f08af0e12da493a1845556").as_slice());
    assert_eq!(ids.setting.as_slice(),bytes("3bca408a4df417ff75ff7300c664ec3c").as_slice());
}
