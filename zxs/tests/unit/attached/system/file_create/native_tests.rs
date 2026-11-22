use super::*;
use crate::{system::reader::{InitialReader,ordinary_pool,sqlite3_get_autocommit,smb::SmbSession},
    test_support::PrivateTemp};
use std::cell::Cell;

#[test]
#[ignore="source fixture on two real SQLite observers and an ordinary F; acknowledges the native cut manually"]
fn native_offer_needs_exact_ack_before_sql_and_does_not_take_a_read_lock(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());let(path,pool,_,rw,ro)=ordinary_pool(&tmp);
    let src=InitialReader::open(&path,pool).unwrap();
    let s=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();
    let w=s.connect(rw).unwrap();let r=s.connect(ro).unwrap();
    let obs=InitialReader::alongside(&src.dir,&src.marker,pool,src.expected_schema).unwrap();
    let c=src.snapshot().unwrap();let root=OrdinaryRoot::for_body(&src,&c,w.setting.volume).unwrap();
    let name="native.txt";let reached=Cell::new(false);
    let gate=|b:&Birth,num:u64|{
        assert_eq!(unsafe{sqlite3_get_autocommit(src.conn.0)},1);
        assert_eq!(u64::from_be_bytes(b.file.0[..8].try_into().unwrap()),num);
        obs.at(|db,c|{assert!(by_op(db,&c,b.operation)?.is_none());Ok(())})?;
        assert!(!root.root.entry_present(&b.container()?)?);
        reached.set(true);Ok(())
    };
    assert!(w.create_native(&root,name,false,|b,num|{
        gate(b,num)?;Err(io::ErrorKind::NotConnected.into())
    },|_|Ok(()),&||Ok(())).is_err());
    assert!(reached.get());assert_eq!(w.occupied_name(name,true).unwrap(),None);
    let (born,())=w.create_native(&root,name,false,gate,|_|Ok(()),&||Ok(())).unwrap();
    let old=w.reader_from_newborn(born).unwrap();assert_eq!(old.size(),0);
    assert_eq!(r.occupied_name(name,false).unwrap(),Some(old.file()));
    let writer=w.open_writer(name,old.file()).unwrap();
    assert!(w.open_writer(name,old.file()).is_err());
    assert_eq!(old.smb_read_window(0,10,|b|Ok(b.len())).unwrap(),0);
    drop(writer);
    assert!(w.create_native(&root,name,true,|_,_|panic!("already present"),|_|Ok(()),&||Ok(())).is_err());
    let offered=Cell::new(None);
    let fd_limit=w.create_native(&root,"cannot-prepare",true,|b,n|{
        gate(b,n)?;offered.set(Some(b.operation));Ok(())
    },|_|Err::<(),_>(io::Error::from(io::ErrorKind::StorageFull)),&||Ok(()));
    assert_eq!(fd_limit.err().unwrap().kind(),io::ErrorKind::StorageFull);
    obs.at(|db,c|{
        let row=by_op(db,&c,offered.get().unwrap())?.unwrap();
        assert!(!row.committed);Ok(())
    }).unwrap();
    assert!(w.open_current("cannot-prepare").is_err());
}
