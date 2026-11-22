use super::*;
#[test]
fn rust_site_vault_metadata_rejects_nonoriginal_shape_without_accessing_a_secret(){
    let v=Stamp{place:held::Place{entity:crate::fs::Entity{device:9,inode:1},mount:3},
        bytes:240,ctime:(12,21),mtime:(33,43),mode:0o100600,uid:0,gid:0};
    let x=witness(v).unwrap();
    assert_eq!((x.dev,x.ino,x.ctime_s,x.ctime_ns,x.mtime_s,x.mtime_ns),(9,1,12,21,33,43));
    for bad in [Stamp{mode:0o100644,..v},Stamp{gid:7,..v},Stamp{bytes:241,..v},
        Stamp{ctime:(12,-1),..v},Stamp{mtime:(33,1_000_000_000),..v}]{
        assert!(witness(bad).is_err());
    }
}
