use super::*;
use crate::{system::reader::{InitialReader,ordinary_pool,smb::SmbSession},test_support::PrivateTemp};
use std::os::unix::fs::{FileExt,OpenOptionsExt};

#[test]
#[ignore="true private SQL and multiple FILE work rounds; source only"]
fn confirmed_history_pages_keep_the_first_unreturned_ordinal_and_original_links(){
    let temp=PrivateTemp::under(&std::env::temp_dir());let (s,p,path,rw,_)=ordinary_pool(&temp);
    let src=InitialReader::open(&s,p).unwrap();let read=InitialReader::open(&s,p).unwrap();
    let account=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();
    let tree=account.connect(rw).unwrap();
    let admin=read.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let file=tree.create_file(OperationId([0x7f;16]),"Tracked.txt").unwrap().file;
    let input=std::fs::OpenOptions::new().write(true).read(true).create_new(true).mode(0o600).open(temp.0.join("input")).unwrap();
    let mut prev=None;let mut originals=Vec::new();
    for i in 0..34u8{
        // The pool already issued repeated-byte IDs for shares/modes.
        // Give both operations in each round an independent test key.
        let id=|kind|{let mut bytes=[0x7f;16];bytes[14]=i;bytes[15]=kind;OperationId(bytes)};
        let op=id(1);let commit=id(2);
        input.write_all_at(&[i],0).unwrap();input.sync_all().unwrap();
        tree.open_writer("Tracked.txt",file).unwrap().save(op,prev,Some(0),1,&input).unwrap();
        originals.push(admin.freeze(commit,admin.manual_target(file).unwrap()).unwrap());prev=Some(op);
    }
    let page=admin.manual_timeline(file,None).unwrap();
    assert_eq!(page.rows.len(),32);assert_eq!(page.next,Some(2));
    assert_eq!(page.rows[0].version,originals[33].version);
    assert_eq!(page.rows[0].parent,Some(originals[32].node));
    // Metadata remains precise even when the actual F is absent.
    let aside=temp.0.join("old-F");std::fs::rename(path,&aside).unwrap();
    let early=admin.manual_timeline(file,page.next).unwrap();
    assert_eq!(early.rows.iter().map(|r|r.order).collect::<Vec<_>>(),vec![2,1]);
    assert_eq!(early.rows[0].length,1);assert_eq!(early.rows[1].node,originals[0].node);
    assert!(early.rows[1].parent.is_none()&&early.next.is_none());
}
