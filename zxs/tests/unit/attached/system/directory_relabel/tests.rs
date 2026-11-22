use super::{selected,recover,RelabelCut,Rename,OperationId,Birth,Connection,OrdinaryRoot};
use crate::system::reader::smb::ordinary_directory::{DirectoryParent,HeldDir,Route};
use std::io;
use crate::{fs::directory_birth::{DirectoryRename,DirectoryRenameSide},
    system::reader::{InitialReader,ordinary_pool,smb::SmbSession,account::{history::TimelinePlace,manual_work::PickerItem,share_mode::Mode}},
    test_support::PrivateTemp};
use std::{cell::Cell,os::unix::fs::{FileExt,MetadataExt,OpenOptionsExt}};
fn op(x:u8)->OperationId{OperationId([x;16])}
#[test]
#[ignore="source-only actual SQLite, same-directory inode and immutable descendant parent, no SMB listener"]
fn exact_pending_names_and_single_location_survive_real_directory_rename(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let(s,pool,f,rw,_)=ordinary_pool(&tmp);let src=InitialReader::open(&s,pool).unwrap();
    let session=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();let tree=session.connect(rw).unwrap();
    let (_,root)=tree.ordinary_route(&[],true).unwrap();
    let (born,held)=tree.make_ordinary_directory(&root,op(0x61),"Reports",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    let(catalog,route)=tree.ordinary_route(&["Reports".into()],true).unwrap();
    let (child,next)=tree.make_ordinary_directory(&route,op(0x62),"Notes",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    let native=std::fs::metadata(&next.directory().path).unwrap().ino();drop(next);
    let occupied=tree.create_file(op(0x63),"Other.txt").unwrap();
    let inside=tree.scoped(DirectoryParent::Directory(child.id));
    let entry=inside.create_file(op(0x69),"Inside.txt").unwrap();
    let reader=InitialReader::open(&s,pool).unwrap();
    let web=reader.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let input=std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600)
        .open(tmp.0.join("history-input")).unwrap();
    input.write_all_at(b"v",0).unwrap();input.sync_all().unwrap();
    inside.open_writer("Inside.txt",entry.file).unwrap().save(op(0x6b),None,Some(0),1,&input).unwrap();
    let version=web.freeze(op(0x6c),web.manual_target(entry.file).unwrap()).unwrap();
    input.write_all_at(b"v2",0).unwrap();input.sync_all().unwrap();
    inside.open_writer("Inside.txt",entry.file).unwrap().save(op(0x6d),Some(op(0x6b)),Some(0),2,&input).unwrap();
    let previous=web.manual_timeline(entry.file,None).unwrap();
    assert_eq!(previous.place,TimelinePlace::Ordinary{name:"Reports\\Notes\\Inside.txt".into(),volume:born.volume});
    let (draft,_) = web.manual_preview(entry.file).unwrap();
    let raw=Connection::open(&src.dir,true).unwrap();
    let orig=route.selected().unwrap();
    let a=&session.original;
    let request=|n,leaf:&str|Rename::prepare(op(n),orig,leaf,tree.setting.share,tree.setting.op,a.op,a.account,a.generation).unwrap();
    let terminal=Birth::placed_container("Other.txt",occupied.file).unwrap();
    for (op_code,leaf) in [(0x64,"Other.txt"),(0x65,terminal.to_str().unwrap())]{
        let bad=request(op_code,leaf);
        assert!(src.transaction(&raw,c"BEGIN IMMEDIATE",|db,_|bad.issue(db)).is_err());
        assert!(src.at(|db,c|selected(db,&c,bad.op)).unwrap().is_none());
    }
    let unproved=request(0x6a,"Unproved");
    // Neither a deferred orphan nor a width-correct but wrong request
    // digest is a positive pending result. Keep both probes uncommitted.
    let breached=src.transaction(&raw,c"BEGIN IMMEDIATE",|db,c|->io::Result<()>{
        let q=db.prepare(c"INSERT INTO main.directory_rename(rename_op,directory_id,source_op,source_gen,
                   source_name,target_name,share_id,setting_op,auth_op) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)")?;
        for(i,id)in [(1,unproved.op.0),(2,unproved.birth.id.0),(3,unproved.source.0),(7,unproved.share.0),
                    (8,unproved.setting.0),(9,unproved.auth.0)]{q.bind_bytes(i,&id)?;}
        q.bind_integer(4,unproved.source_gen)?;q.bind_bytes(5,unproved.old.as_bytes())?;
        q.bind_bytes(6,unproved.target.as_bytes())?;q.done()?;drop(q);
        let scope=DirectoryParent::Directory(child.id);
        let orphan=Route::settled_at(db,&c,born.volume,scope).unwrap_err();
        assert!(orphan.to_string().contains("does not have its exact original rename"));
        let mut wrong=unproved.digest()?;wrong[0]^=1;
        let q=db.prepare(c"INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,16,?2,?3,0)")?;
        q.bind_bytes(1,&unproved.op.0)?;q.bind_bytes(2,&unproved.actor.0)?;q.bind_bytes(3,&wrong)?;q.done()?;drop(q);
        let wrong=Route::settled_at(db,&c,born.volume,scope).unwrap_err();
        assert!(wrong.to_string().contains("differs from its original request"));
        Err(io::Error::other("rollback malformed picker specimen"))
    }).unwrap_err();
    assert_eq!(breached.to_string(),"rollback malformed picker specimen");
    let r=request(0x66,"reports");
    src.transaction(&raw,c"BEGIN IMMEDIATE",|db,_|r.issue(db)).unwrap();
    assert_eq!(src.at(|db,c|selected(db,&c,r.op)).unwrap(),Some(r.clone()));
    assert!(f.join("Reports").is_dir());
    assert_eq!(OrdinaryRoot::at_parent(&src,&catalog,&route,None).err().unwrap().kind(),io::ErrorKind::WouldBlock);
    let(rows,after)=web.manual_files(None).unwrap();
    assert_eq!(rows.len(),2);assert!(after.is_none());
    assert!(rows.iter().any(|row|matches!(row,PickerItem::ParentPending(id) if *id==entry.file)));
    assert!(rows.iter().any(|row|matches!(row,PickerItem::Named(item) if item.file==occupied.file&&item.name=="Other.txt")));
    let pending=web.manual_timeline(entry.file,None).unwrap();
    assert_eq!((pending.file,pending.place,pending.next),(entry.file,TimelinePlace::ParentPending,None));
    assert!(matches!(&pending.rows[..],[row] if (row.order,row.node,row.version,row.length)==(1,version.node,version.version,1)));
    assert_eq!(web.manual_preview(entry.file).err().unwrap().kind(),io::ErrorKind::WouldBlock);
    assert_eq!(web.freeze(op(0x6e),draft).err().unwrap().kind(),io::ErrorKind::WouldBlock);
    assert!(web.manual_receipt(op(0x6e),draft).unwrap().is_none());
    let conflicting=request(0x66,"Wrong");
    assert!(src.transaction(&raw,c"BEGIN IMMEDIATE",|db,c|conflicting.confirm(db,&c)).is_err());
    assert_eq!(src.at(|db,c|selected(db,&c,r.op)).unwrap(),Some(r.clone()));
    // A pending relabel is not a pending CREATE, even for the old
    // name; the kind-13 abandon path cannot hide the original birth.
    assert!(src.transaction(&raw,c"BEGIN IMMEDIATE",|db,_|{
        let q=db.prepare(c"INSERT INTO main.directory_abort(create_op,setting_op,auth_op,reason) VALUES(?1,?2,?3,2)")?;
        q.bind_bytes(1,&born.original.0)?;q.bind_bytes(2,&r.setting.0)?;q.bind_bytes(3,&r.auth.0)?;q.done()
    }).is_err());
    for name in ["Reports","reports"]{
        assert_eq!(tree.ordinary_kind(&[name.into()]).err().unwrap().kind(),io::ErrorKind::WouldBlock);
        assert!(tree.create_file(op(0x67),name).is_err()); // exact pending directory names also reserve FILE namespace
    }
    assert_eq!(src.at(|db,c|Route::at(db,&c,born.volume,DirectoryParent::Directory(child.id))).err().unwrap().kind(),
        io::ErrorKind::WouldBlock);
    let parent=HeldDir::trusted_storage(&f).unwrap();
    let actual=DirectoryRename::hold(&born,&parent,"Reports","reports",Some(held)).unwrap();
    let (moved,())=actual.settle(DirectoryRenameSide::Destination,||Ok(()),||{
        assert_eq!(tree.ordinary_route(&["reports".into()],false).err().unwrap().kind(),io::ErrorKind::WouldBlock);Ok(())
    },|_|src.transaction(&raw,c"BEGIN IMMEDIATE",|db,c|r.confirm(db,&c))).unwrap();
    assert_eq!(moved.directory().path,f.join("reports"));drop(moved);
    assert_eq!(tree.ordinary_route(&["Reports".into()],false).err().unwrap().kind(),io::ErrorKind::NotFound);
    let (_,route)=tree.ordinary_route(&["reports".into(),"Notes".into()],false).unwrap();
    assert!(route.selected().unwrap().is_birth(&child));
    let(_,now)=tree.ordinary_route(&["reports".into()],false).unwrap();
    let now=now.selected().unwrap();
    assert_eq!(now.birth(),&born);
    assert_eq!((now.position(),now.generation()),(r.op,2));
    assert_eq!(std::fs::metadata(f.join("reports/Notes")).unwrap().ino(),native);
    let(rows,_)=web.manual_files(None).unwrap();
    assert!(rows.iter().any(|row|matches!(row,PickerItem::Named(item)
        if item.file==entry.file && item.name=="reports\\Notes\\Inside.txt")));
    let after=web.manual_timeline(entry.file,None).unwrap();
    assert_eq!(after.place,TimelinePlace::Ordinary{name:"reports\\Notes\\Inside.txt".into(),volume:born.volume});
    assert!(matches!(&after.rows[..],[row] if (row.node,row.version,row.length)==(version.node,version.version,1)));
    assert_eq!(web.manual_preview(entry.file).unwrap().0,draft);
    assert!(src.transaction(&raw,c"BEGIN IMMEDIATE",|db,c|r.confirm(db,&c)).is_err());
    let (separate,_)=tree.make_ordinary_directory(&root,op(0x68),"Reports",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    assert_ne!(separate.id,born.id);
}

#[test]
#[ignore="source-only original SMB tree, real filesystem cut and independent SQLite writer; no listener"]
fn the_hot_operation_and_cold_exact_recovery_do_not_invent_a_new_directory(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let(s,pool,f,rw,_)=ordinary_pool(&tmp);let src=InitialReader::open(&s,pool).unwrap();
    let another=InitialReader::open(&s,pool).unwrap();
    let session=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();let tree=session.connect(rw).unwrap();
    let (_,root)=tree.ordinary_route(&[],true).unwrap();
    let (born,open)=tree.make_ordinary_directory(&root,op(0x71),"Draft",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    let mark=std::fs::metadata(&open.directory().path).unwrap().ino();
    let(_,old)=tree.ordinary_route(&["Draft".into()],true).unwrap();
    let child=tree.scoped(DirectoryParent::Directory(born.id));
    tree.create_file(op(0x72),"Occupied").unwrap();
    let mut saved=Some(open);
    assert_eq!(child.relabel_directory("Occupied",op(0x73),&old,&mut saved,&||Ok(()),|_|Ok(())).err().unwrap().kind(),
        io::ErrorKind::AlreadyExists);
    assert!(saved.is_some());
    let reached=Cell::new(false);
    let allow=|_:&Connection,_:&super::Catalog,share,volume| if (share,volume)==(rw,born.volume){Ok(())}
        else{Err(io::ErrorKind::PermissionDenied.into())};
    assert_eq!(child.relabel_directory("Final",op(0x74),&old,&mut saved,&||Ok(()),|cut|{
        if cut==RelabelCut::Reserved{reached.set(true);return Err(io::ErrorKind::Interrupted.into());}Ok(())
    }).err().unwrap().kind(),io::ErrorKind::ConnectionAborted);
    assert!(reached.get()&&saved.is_none()&&f.join("Draft").is_dir()&&!f.join("Final").exists());
    assert!(another.at(|db,c|selected(db,&c,op(0x74))).unwrap().is_some_and(|r|!r.aborted&&!r.confirmed));
    recover(&another,op(0x74),&allow,&||Ok(())).unwrap();
    assert!(another.at(|db,c|selected(db,&c,op(0x74))).unwrap().unwrap().aborted);
    assert!(f.join("Draft").is_dir()&&!f.join("Final").exists());

    let parent=HeldDir::trusted_storage(&f).unwrap();saved=Some(born.open(&parent).unwrap());
    reached.set(false);
    assert_eq!(child.relabel_directory("Final",op(0x75),&old,&mut saved,&||Ok(()),|cut|{
        if cut==RelabelCut::NameChanged{reached.set(true);return Err(io::ErrorKind::Interrupted.into());}Ok(())
    }).err().unwrap().kind(),io::ErrorKind::ConnectionAborted);
    assert!(reached.get()&&saved.is_none()&&!f.join("Draft").exists()&&f.join("Final").is_dir());
    let web=another.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let active=web.share_setting(rw).unwrap();
    web.set_share_mode(op(0x76),rw,active.op,Mode::ReadOnly).unwrap();
    assert!(recover(&another,op(0x75),&allow,&||Ok(())).is_err());
    assert!(f.join("Final").is_dir()&&!f.join("Draft").exists());
    web.set_share_mode(op(0x77),rw,op(0x76),Mode::ReadWrite).unwrap();
    recover(&another,op(0x75),&allow,&||Ok(())).unwrap();
    assert!(another.at(|db,c|selected(db,&c,op(0x75))).unwrap().unwrap().aborted);
    let again=session.connect(rw).unwrap();
    let (_,restored)=again.ordinary_route(&["Draft".into()],true).unwrap();
    assert_eq!(restored,old);
    assert_eq!(std::fs::metadata(f.join("Draft")).unwrap().ino(),mark);

    saved=Some(born.open(&parent).unwrap());
    let exact=again.scoped(DirectoryParent::Directory(born.id));
    reached.set(false);
    assert!(exact.relabel_directory("Accepted",op(0x78),&restored,&mut saved,&||Ok(()),|at|{
        if at==RelabelCut::NameChanged{reached.set(true);return Err(io::ErrorKind::Interrupted.into());}Ok(())
    }).is_err());
    assert!(reached.get());recover(&another,op(0x78),&allow,&||Ok(())).unwrap();
    let(_,winner)=again.ordinary_route(&["Accepted".into()],false).unwrap();
    assert_eq!(winner.selected().unwrap().birth(),&born);
    assert_eq!(std::fs::metadata(f.join("Accepted")).unwrap().ino(),mark);
}
