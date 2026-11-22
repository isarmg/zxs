use super::*;
fn public_plan()->Vec<u8>{
    // Immutable public plan, never a credential or real installation path.
    let hex=b"5a58532d525553542d534d422d49310011111111111111111111111111111111121212121212121212121212121212121616161616161616161616161616161617171717171717171717171717171717131313131313131313131313131313130b666978747572652e6c616e010418c00002110133333333333333333333333333333333020004446f63730006e59b9ee694b6";
    hex.as_chunks::<2>().0.iter().map(|n|u8::from_str_radix(std::str::from_utf8(n).unwrap(),16).unwrap()).collect()
}
fn record(system:&[u8],image:&[u8])->Vec<u8>{
    let mut raw=MAGIC.to_vec();
    raw.extend_from_slice(&public_plan()[16..96]);
    raw.extend([31;16]);raw.extend(1001u32.to_be_bytes());raw.extend(1002u32.to_be_bytes());
    raw.extend(crate::hash::digest(&public_plan()).unwrap());raw.extend([41;32]);
    for path in [system,image]{raw.extend((path.len() as u16).to_be_bytes());raw.extend(path);}
    raw
}
#[test]
fn rust_site_record_binds_original_scope_and_the_whole_plan(){
    let b=record(b"/zxs/disks/system",b"/zxs/bin/zxs");let r=Approval::decode(&b).unwrap();
    assert_eq!(r.origin.instance,[0x16;16]);assert_eq!(r.origin.setup,[0x13;16]);
    assert_eq!((r.subject.uid,r.subject.gid),(1001,1002));
    assert_eq!(r.system.as_path(),Path::new("/zxs/disks/system"));assert_eq!(r.image_sha,[41;32]);
    let p=public_plan();r.require_plan(&p).unwrap();
    for i in [0,17,96,p.len()-1]{let mut other=p.clone();other[i]^=1;assert!(r.require_plan(&other).is_err());}
    for k in 0..b.len(){assert!(Approval::decode(&b[..k]).is_err());}
}
#[test]
fn rust_site_record_does_not_authorize_aliases_unknown_principals_or_extras(){
    for s in [b"/".as_slice(),b"/mnt//system",b"/mnt/../system",b"/mnt/S\n",b"/\xff",b"/opt/zxs/bin/zxs/other"]{
        assert!(Approval::decode(&record(s,b"/opt/zxs/bin/zxs")).is_err());
    }
    let raw=record(b"/mnt/S",b"/opt/zxs/bin/zxs");
    let mut extra=raw.clone();extra.push(0);assert!(Approval::decode(&extra).is_err());
    for (start,len) in [(16,16),(96,16),(112,4),(116,4),(120,32),(152,32)]{
        let mut bad=raw.clone();bad[start..start+len].fill(0);assert!(Approval::decode(&bad).is_err());
    }
    let mut bad=raw;let id:[u8;16]=bad[16..32].try_into().unwrap();
    bad[32..48].copy_from_slice(&id);assert!(Approval::decode(&bad).is_err());
}
