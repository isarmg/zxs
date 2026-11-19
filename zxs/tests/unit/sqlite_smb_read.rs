//! SQLite-to-buffer side of one bounded SMB READ. Only the successful
//! protocol login is a test fixture; it does not run a network client.
use super::*;

#[test]
#[ignore="real SQLite, private ordinary FILE and two independently verified blocks; not run"]
fn checked_window_clips_eof_and_never_hands_off_a_prefix_of_a_damaged_read(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p,f,rw,ro)=enrollment(&t);
    let src=InitialReader::open(&s,p).unwrap();let peer=InitialReader::open(&s,p).unwrap();
    let manager=peer.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let smb=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
    let a=smb.connect(rw).unwrap();let b=smb.connect(ro).unwrap();
    let id=a.create_file(OperationId([0xda;16]),NAME).unwrap().file;
    let before=b.open_current(NAME).unwrap();
    let block=4*1024*1024;let mut data=vec![0x13u8;block+2];data[block..].copy_from_slice(b"XY");
    let received=input(&t,"received", &data);a.save_first(SAVE,NAME,id,&received).unwrap();
    let held=b.open_current(NAME).unwrap();let len=data.len() as u64;
    let blk=block as u64;
    assert!(before.smb_read_window(0,16,|v|Ok(v.to_vec())).unwrap().is_empty());
    assert_eq!(held.smb_read_window(blk-1,15,|v|Ok(v.to_vec())).unwrap(),[0x13,b'X',b'Y']);
    for (off,n) in [(len,1),(len+37,1),(len,0),(0,0)]{
        assert!(held.smb_read_window(off,n,|v|Ok(v.to_vec())).unwrap().is_empty());
    }
    for(off,n) in [(u64::MAX,0),(i64::MAX as u64,1),(0,0x00ff_ff01)]{
        assert!(held.smb_read_window(off,n,|_|->io::Result<()>{panic!("invalid request reached native reply")}).is_err());
    }
    drop(held);drop(before);
    // Damage only the last short block before the next logical open. Its
    // original checksums still exist; the first good block remains readable.
    let raw=f.join(format!("方案-{id}.xlsx/change-{SAVE}-00000000"));
    let disk=OpenOptions::new().write(true).open(raw).unwrap();disk.write_all_at(b"Z",blk+1).unwrap();
    disk.sync_all().unwrap();drop(disk);
    let h=b.open_current(NAME).unwrap();
    assert_eq!(h.smb_read_window(0,1,|v|Ok(v.to_vec())).unwrap(),[0x13]);
    assert!(h.smb_read_window(blk-1,3,|_|->io::Result<()>{panic!("integrity-failed window exposed its good prefix")}).is_err());
    // The consumer only fills pending native output. A concurrent revocation
    // is still a failed request, never a repeat of a previous handoff.
    let setting=manager.share_setting(ro).unwrap();let mut copies=0;
    assert!(h.smb_read_window(2,1,|v|{
        assert_eq!(v,[0x13]);copies+=1;
        manager.set_share_mode(OperationId([0xdb;16]),ro,setting.op,Mode::Disabled).unwrap();Ok(())
    }).is_err());assert_eq!(copies,1);
    assert!(h.smb_read_window(len,0,|_|->io::Result<()>{panic!("revoked empty request")}).is_err());
}
