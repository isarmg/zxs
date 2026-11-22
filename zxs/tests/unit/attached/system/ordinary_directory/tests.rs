use super::*;
use crate::{system::reader::{InitialReader,ordinary_pool,sqlite3_get_autocommit,smb::SmbSession},test_support::PrivateTemp};
use std::{cell::Cell,fs::OpenOptions,os::unix::fs::{FileExt,OpenOptionsExt}};
fn op(byte:u8)->OperationId{OperationId([byte;16])}
#[test]
#[ignore="source only: real ordinary parent, dual witnesses, SQLite, work and Web publication; no SMB listener"]
fn dir_and_file_share_exact_parent_name_but_not_locks_or_ancestry(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());let (path,pool,f,rw,ro)=ordinary_pool(&tmp);
    let src=InitialReader::open(&path,pool).unwrap();
    let other=InitialReader::alongside(&src.dir,&src.marker,pool,src.expected_schema).unwrap();
    let s=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();
    let tree=s.connect(rw).unwrap();let reader=s.connect(ro).unwrap();
    let (_,root)=tree.ordinary_route(&[],true).unwrap();
    let reached=Cell::new(false);
    assert!(tree.make_ordinary_directory(&root,op(0x91),"Reports",|b|{
        assert_eq!(unsafe{sqlite3_get_autocommit(src.conn.0)},1);
        assert!(other.at(|db,c|original(db,&c,b.original))?.is_none());
        assert!(!f.join("Reports").exists());reached.set(true);Err(io::ErrorKind::ConnectionAborted.into())
    },&||Ok(()),|_|Ok(())).is_err());
    assert!(reached.get());

    let cut=Cell::new(false);
    assert!(tree.make_ordinary_directory(&root,op(0x91),"Reports",|_|Ok(()),&||Ok(()),|phase|{
        if phase==Cut::Installed{
            cut.set(true);
            assert!(f.join("Reports").is_dir());
            assert!(!other.at(|db,c|original(db,&c,op(0x91)))?.unwrap().done);
            assert!(tree.ordinary_route(&["Reports".into()],false).is_err());
            assert_eq!(tree.ordinary_kind(&["Reports".into()]).err().unwrap().kind(),io::ErrorKind::WouldBlock);
            return Err(io::ErrorKind::Interrupted.into());
        }Ok(())
    }).is_err());
    assert!(cut.get());assert_eq!(tree.resumable_directory(&root,"Reports").unwrap(),Some(op(0x91)));
    let confirmed=other.at(|db,c|original(db,&c,op(0x91))).unwrap().unwrap();
    assert!(!confirmed.done);
    let (capital,_)=tree.make_ordinary_directory(&root,op(0x91),"Reports",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    // The real observer saw that prior pending cut. If it retries
    // after the first caller confirmed, no second native OFFER exists.
    let a2=SmbSession::fixture_authenticated(&other,"first.admin").unwrap();let second=a2.connect(rw).unwrap();
    let (_,r2)=second.ordinary_route(&[],true).unwrap();
    let (same,_)=second.make_ordinary_directory(&r2,confirmed.birth.original,"Reports",|_|panic!("already confirmed"),&||Ok(()),|_|Ok(())).unwrap();
    assert_eq!(same.id,capital.id);
    let (lower,_)=tree.make_ordinary_directory(&root,op(0x92),"reports",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    assert_ne!(capital.id,lower.id);
    let partial=Cell::new(None);
    assert!(tree.make_ordinary_directory(&root,op(0xa1),"Partial",|b|{partial.set(Some(b.original));Ok(())},&||Ok(()),|phase|{
        if phase==Cut::Reserved{
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(f.join(format!(".zxs-new-{}-4",op(0xa1))))?;
            return Err(io::ErrorKind::Interrupted.into());
        }Ok(())
    }).is_err());assert_eq!(partial.get(),Some(op(0xa1)));
    assert!(!f.join("Partial").exists());
    let (fresh,_)=tree.make_ordinary_directory(&root,op(0xa2),"Partial",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    assert_eq!(fresh.original,op(0xa2));
    assert!(other.at(|db,c|original(db,&c,op(0xa1))).unwrap().unwrap().aborted);
    assert!(tree.create_file(op(0x93),"Reports").is_err());
    let stopped=Cell::new(false);
    assert!(tree.create_after(op(0xa3),"Staged.md",|point|{
        if point==super::super::file_create::Point::Reserved{stopped.set(true);return Err(io::ErrorKind::Interrupted.into());}
        Ok(())
    }).is_err());
    assert!(stopped.get());
    let terminal=other.at(|db,_|{
        let mut q=db.prepare(c"SELECT terminal FROM main.root_entry WHERE create_op=?1")?;
        q.bind_bytes(1,&op(0xa3).0)?;assert!(work_record::start_one(&q)?);
        let name=display::component(q.row()?.blob(0,255)?,255)?.to_owned();q.done()?;Ok(name)
    }).unwrap();
    assert!(!f.join(&terminal).exists());
    assert!(tree.make_ordinary_directory(&root,op(0xa4),&terminal,|_|panic!("pending FILE owns this physical slot"),&||Ok(()),|_|Ok(())).is_err());
    assert_eq!(tree.create_file(op(0xa3),"Staged.md").unwrap().created,op(0xa3));
    assert!(reader.make_ordinary_directory(&root,op(0x94),"Denied",|_|panic!("read-only"),&||Ok(()),|_|Ok(())).is_err());
    let (_,parent)=tree.ordinary_route(&["Reports".into()],true).unwrap();
    let (year,_)=tree.make_ordinary_directory(&parent,op(0x95),"2027",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    let (c,route)=tree.ordinary_route(&["Reports".into(),"2027".into()],true).unwrap();
    assert_eq!(route.parent(),DirectoryParent::Directory(year.id));
    let dir=OrdinaryRoot::at_parent(&src,&c,&route,None).unwrap();
    assert_eq!(dir.folder().path,f.join("Reports/2027"));
    let inside=tree.scoped(route.parent());let outside=tree.scoped(DirectoryParent::Directory(lower.id));
    let new=inside.create_file(op(0x96),"Data").unwrap();
    let sibling=outside.create_file(op(0x97),"Data").unwrap();
    assert_ne!(new.file,sibling.file);
    assert!(matches!(reader.ordinary_kind(&["Reports".into(),"2027".into()]).unwrap().1,
        NamedKind::Directory(d) if d.parent()==route.parent()));
    assert!(matches!(reader.ordinary_kind(&["Reports".into(),"2027".into(),"Data".into()]).unwrap().1,NamedKind::File));
    assert_eq!(reader.ordinary_kind(&["Reports".into(),"2027".into(),"data".into()]).err().unwrap().kind(),io::ErrorKind::NotFound);
    assert!(inside.make_ordinary_directory(&route,op(0x98),"Data",|_|panic!("FILE owns logical name"),&||Ok(()),|_|Ok(())).is_err());
    assert!(tree.open_current("Data").is_err());
    let staged=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(tmp.0.join("input")).unwrap();
    staged.write_all_at(b"v11",0).unwrap();staged.sync_all().unwrap();
    let writer=inside.open_writer("Data",new.file).unwrap();
    assert!(inside.open_writer("Data",new.file).is_err());
    let older=inside.open_current("Data").unwrap();
    assert_eq!(older.size(),0);
    writer.save(op(0x99),None,Some(0),3,&staged).unwrap();drop(writer);
    let mut found=Vec::new();inside.open_current("Data").unwrap().range_to(0,3,&mut found).unwrap();
    assert_eq!(found,b"v11");assert_eq!(older.size(),0);
    let admin=src.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let (chosen,item)=admin.manual_preview(new.file).unwrap();
    assert_eq!(item.name,"Reports\\2027\\Data");
    use crate::system::reader::account::manual_work::PickerItem;
    let page=admin.manual_files(None).unwrap().0;
    assert!(page.iter().any(|x|matches!(x,PickerItem::Named(i) if i.file==new.file && i.name=="Reports\\2027\\Data")));
    assert!(page.iter().any(|x|matches!(x,PickerItem::Named(i) if i.file==sibling.file && i.name=="reports\\Data")));
    let receipt=admin.freeze(op(0x9a),chosen).unwrap();
    assert_eq!(receipt.ord,1);
    let shown=admin.manual_timeline(new.file,None).unwrap();
    assert_eq!(shown.place,crate::system::reader::account::history::TimelinePlace::Ordinary{
        name:"Reports\\2027\\Data".into(),volume:route.volume(),
    });
    assert_eq!(shown.rows[0].version,receipt.version);
    let base=OrdinaryRoot::at_parent(&src,&c,&root,None).unwrap();
    let listing=super::super::directory::mixed_page(&tree,&base,b"",2,||Ok(())).unwrap();
    assert_eq!(listing.iter().map(|e|e.name()).collect::<Vec<_>>(),vec!["Partial","Reports"]);
    let tail=super::super::directory::mixed_page(&tree,&base,b"Reports",2,||Ok(())).unwrap();
    assert_eq!(tail.iter().map(|e|e.name()).collect::<Vec<_>>(),vec!["Staged.md","reports"]);
    let listed=super::super::directory::mixed_page(&inside,&dir,b"",2,||Ok(())).unwrap();
    assert!(matches!(&listed[..],[super::super::directory::Child::File(r)] if r.file==new.file&&r.size==3));
    let swapped=f.join("Reports/2027");let away=f.join("reattach");
    std::fs::rename(&swapped,&away).unwrap();
    let (c,selected)=reader.ordinary_kind(&["Reports".into(),"2027".into()]).unwrap();
    let NamedKind::Directory(selected)=selected else{panic!("lost real directory cannot become a FILE hint")};
    assert!(OrdinaryRoot::at_parent(&src,&c,&selected,None).is_err());
    assert!(inside.open_current("Data").is_err());
    std::fs::rename(&away,&swapped).unwrap();
    assert_eq!(inside.open_current("Data").unwrap().file(),new.file);
}
