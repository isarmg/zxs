// All keys and fixture credential bytes are public synthetic/test inputs; no
// installation, native account, shell login, Samba instance or medium is used.
use super::*;
use crate::sqlite_install::stage::SCHEMA;
use rusqlite::{params, Connection};

fn field(key: &str) -> &'static str {
    include_str!("first-account-v1.txt").lines()
        .filter_map(|line| line.split_once('='))
        .find(|(k, _)| *k == key).unwrap().1
}
fn hex(key: &str) -> Vec<u8> {
    field(key).as_bytes().chunks_exact(2)
        .map(|x| u8::from_str_radix(std::str::from_utf8(x).unwrap(), 16).unwrap()).collect()
}
fn fixed() -> PairRequest {
    let id = |s| hex(s).try_into().unwrap();
    PairRequest::new(id("pool"), id("init_op"), id("setup_op"), id("offline_actor"), id("approval_ref"),
        &hex("web_login_hex"), &hex("smb_login_hex"), hex("smb_sid").try_into().unwrap()).unwrap()
}

#[test]
fn public_original_binds_every_nonsecret_field_but_no_verifier_or_password() {
    let r = fixed();
    assert_eq!(PREFIX.len() + 5 * 16 + 2 + r.web.len() + r.smb.len() + r.sid.len(),
               field("preimage_bytes").parse::<usize>().unwrap());
    assert_eq!(r.sha().as_slice(), hex("request_sha"));
    for which in 0..8 {
        let mut other = fixed();
        match which {
            0 => other.pool[0] ^= 1, 1 => other.init[0] ^= 1, 2 => other.op[0] ^= 1,
            3 => other.actor[0] ^= 1, 4 => other.approval[0] ^= 1, 5 => other.web.push(b'w'),
            6 => other.smb.push(b's'), _ => other.sid[12] ^= 1,
        }
        assert_ne!(other.sha(), r.sha());
    }
}

#[test]
fn no_guest_shared_name_unknown_sid_domain_or_builtin_rid_gets_a_valid_request() {
    let r = fixed();
    let make = |w: &[u8], s: &[u8], sid| PairRequest::new(r.pool, r.init, r.op, r.actor, r.approval, w, s, sid);
    for bad in [b"guest".as_slice(), b"anonymous", b"nobody", b"root", b".user", b"Name", b"f\0x", b""] {
        assert!(make(&r.web, bad, r.sid).is_err());
    }
    assert!(make(&r.smb, &r.smb, r.sid).is_err());
    assert!(make(&[b'a'; 64], &r.smb, r.sid).is_ok());
    assert!(make(&[b'a'; 65], &r.smb, r.sid).is_err());
    let mut unknown = r.sid; unknown[12..24].fill(0); assert!(make(&r.web, &r.smb, unknown).is_err());
    for i in [0, 1, 7, 8, 24] {
        let mut sid = r.sid; sid[i] ^= 1; assert!(make(&r.web, &r.smb, sid).is_err());
    }
    assert!(PairRequest::new(r.pool, r.init, r.init, r.actor, r.approval, &r.web, &r.smb, r.sid).is_err());
}

fn database() -> Connection {
    let mut db = Connection::open_in_memory().unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    let tx=db.transaction().unwrap(); tx.execute_batch(SCHEMA).unwrap();
    tx.execute("INSERT INTO operation VALUES(?1,1,?2,?3,0)",params![&[1u8;16][..],&[2u8;16][..],&[8u8;32][..]]).unwrap();
    tx.execute("INSERT INTO pool_birth VALUES(1,?1,?2,?3,1000,3)",params![&[4u8;16][..],&[1u8;16][..],&[3u8;16][..]]).unwrap();
    for (i, path) in [b"/s".as_slice(), b"/r", b"/f"].into_iter().enumerate() {
        tx.execute("INSERT INTO volume_claim VALUES(?1,?2,?2,?3,?4,255,?5)",
            params![&[1u8;16][..],i as i64,&[10+i as u8;16][..],path,&[20+i as u8;32][..]]).unwrap();
    }
    tx.commit().unwrap(); db
}
fn pair_rows(tx: &Transaction<'_>, r: &PairRequest, both: bool) {
    tx.execute("INSERT INTO account_pair VALUES(1,?1,?2,?3,?4,?5,?6,?7)", params![
        &r.op[..], &r.approval[..], hex("result_web_id"), &r.web[..], hex("result_smb_id"), &r.smb[..], &r.sid[..]]).unwrap();
    tx.execute("INSERT INTO web_password VALUES(1,?1,1,?2,?3)",params![&r.op[..],hex("web_salt"),hex("web_verifier")]).unwrap();
    if both {
        tx.execute("INSERT INTO smb_password VALUES(1,?1,1,?2)",params![&r.op[..],hex("synthetic_smb_field")]).unwrap();
    }
}
fn commit_origin(tx: &Transaction<'_>, r: &PairRequest) -> rusqlite::Result<usize> {
    tx.execute("INSERT INTO operation VALUES(?1,2,?2,?3,1)",params![&r.op[..],&r.actor[..],&r.sha()[..]])
}
#[test]
fn real_sqlite_history_never_completes_half_a_pair_or_reissues_a_previous_result() {
    let mut db=database(); let tpl=Canonical::new().unwrap(); let r=fixed();
    {
        let tx=db.transaction().unwrap(); assert!(r.original(&tx,&tpl).is_err()); // pending INIT
        tx.execute("UPDATE operation SET state=1 WHERE op_id=?1", [&r.init[..]]).unwrap();
        tx.commit().unwrap();
    }
    {
        let tx=db.transaction().unwrap();
        assert_eq!(r.original(&tx,&tpl).unwrap(),History::Absent);
        pair_rows(&tx,&r,false); assert!(commit_origin(&tx,&r).is_err());
        assert!(r.original(&tx,&tpl).is_err()); tx.rollback().unwrap();
    }
    {
        let tx=db.transaction().unwrap(); assert_eq!(r.original(&tx,&tpl).unwrap(),History::Absent);
        pair_rows(&tx,&r,true); assert_eq!(commit_origin(&tx,&r).unwrap(),1);
        tx.commit().unwrap();
    }
    db.execute_batch("PRAGMA query_only=ON;").unwrap();
    let tx=db.transaction().unwrap(); let changes=tx.total_changes();
    let expected=History::Committed { web:hex("result_web_id").try_into().unwrap(), smb:hex("result_smb_id").try_into().unwrap() };
    assert_eq!(r.original(&tx,&tpl).unwrap(),expected); assert_eq!(r.original(&tx,&tpl).unwrap(),expected);
    for which in 0..5 {
        let mut other=fixed();
        match which {0=>other.actor[0]^=1,1=>other.approval[0]^=1,2=>other.op[0]^=1,3=>other.web.push(b'z'),_=>other.sid[16]^=1}
        assert!(other.original(&tx,&tpl).is_err());
    }
    assert_eq!(tx.total_changes(),changes);
}

#[test]
fn committed_pool_with_normal_file_volume_publications_checks_negative_history_without_scanning_log() {
    let mut db=database();let request=fixed();
    let t=db.transaction().unwrap();
    t.execute("UPDATE operation SET state=1 WHERE op_id=?1",[&request.init[..]]).unwrap();
    pair_rows(&t,&request,true);assert_eq!(commit_origin(&t,&request).unwrap(),1);
    t.commit().unwrap();
    let web_id=hex("result_web_id");
    for i in 0..32u8 {
        let t=db.transaction().unwrap();let op=[0x80+i;16];let share=[0x20+i;16];
        t.execute("INSERT INTO root_share VALUES(?1,?2,?3,?4,?5)", params![&share[..],&op[..],&request.op[..],&[12u8;16][..],format!("share-{i}").as_bytes()]).unwrap();
        t.execute("INSERT INTO operation VALUES(?1,3,?2,?3,1)",params![&op[..],&web_id[..],&[0x62u8;32][..]]).unwrap();
        t.commit().unwrap();
    }
    let tx=db.transaction().unwrap();let mut lookup=tx.prepare(CONFLICT).unwrap();
    for id in [[0xfeu8;16],[0x80u8;16]] {
        // An unused Op on an already-occupied pair and a used, wrong-kind Op
        // are both refusals. Neither negative case may walk ordinary operations.
        assert!(lookup.query_row([&id[..]],|r|r.get::<_,bool>(0)).unwrap());
        assert_eq!(lookup.get_status(rusqlite::StatementStatus::FullscanStep),0);
        let mut wrong=fixed();wrong.op=id;
        assert!(wrong.original(&tx,&Canonical::new().unwrap()).is_err());
    }
}
