use super::*;
use crate::{ordinary::first_account::fixture_frame,pool::{MAX_SYSTEM,format::maintenance::tests::register_fixture}};

const EXAMPLE:&str=include_str!("../vectors/first-account-v1.txt");
// The publicly documented argon2-cffi 25.1.0 howto input, not a secret or a
// supported preset. The example text is never written into a frame/receipt.
const PUBLIC_INPUT:&str="correct horse battery staple";
// If ignored media fixtures are explicitly included alongside regular tests,
// they must not compete for the production verifier's one process-wide slot.
static FIXTURE_VERIFIER:std::sync::Mutex<()>=std::sync::Mutex::new(());
fn raw(field:&str)->Vec<u8>{
    let hex=EXAMPLE.lines().filter_map(|l|l.split_once('=')).find(|(k,_)|*k==field).unwrap().1;
    assert_eq!(hex.len()%2,0);(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect()
}
fn array<const N:usize>(field:&str)->[u8;N]{raw(field).try_into().expect("published width")}
fn exact_frame(pool:PoolId,init:OperationId,original:[u8;32])->(Vec<u8>,fs::Frontier){
    fixture_frame(pool,init,original,OperationId(array("op")),AccountId(array("account")),
        array("operator_ref"),&raw("login"),array("salt"),array("argon_tag"))
}

#[test]
fn independent_bytes_have_a_single_account_not_an_issuer_or_a_fallback_prefix(){
    let pool=PoolId(array("pool"));let init=OperationId([0x22;16]);let genesis=array("genesis_digest");
    let(frame,floor)=exact_frame(pool,init,genesis);
    assert_eq!(frame,raw("frame"));assert_eq!(floor,array("frontier"));
    let first=First::confirmed(pool,init,genesis,&frame,floor).unwrap();
    assert_eq!(first.account,AccountId(array("account")));assert_eq!(first.operation,OperationId(array("op")));
    assert_eq!(first.decision,array("decision_digest"));
    assert!(First::confirmed(PoolId([3;16]),init,genesis,&frame,floor).is_err());
    assert!(First::confirmed(pool,first.operation,genesis,&frame,floor).is_err());
    assert!(First::confirmed(pool,init,[9;32],&frame,floor).is_err());
    assert!(First::confirmed(pool,init,genesis,&frame,fs::frontier(0,0,genesis)).is_err());
    assert!(First::confirmed(pool,init,genesis,&frame[..frame.len()-1],floor).is_err());
    for damage in 0..4{
        let mut foreign=frame.clone();let body=foreign.windows(14).position(|w|w==b"ZXS/ACCOUNT/1\0").unwrap();
        let account=body+14+1+16;let login=account+16+16+1;
        match damage{
            0=>foreign[body-33]=10, // other decision kind
            1=>foreign[body-1]^=1, // request not derivable from this account operation
            2=>foreign[account..account+16].fill(0),
            _=>foreign[login]=b'F',
        }
        let end=foreign.len()-32;let d=crate::hash::digest(&foreign[..end]).unwrap();foreign[end..].copy_from_slice(&d);
        assert!(First::confirmed(pool,init,genesis,&foreign,fs::frontier(1,foreign.len(),d)).is_err());
    }
    let mut extra=frame;extra.push(0);
    assert!(First::confirmed(pool,init,genesis,&extra,floor).is_err());
}

#[test]
fn the_public_reference_password_matches_only_the_fixed_tag_and_exact_login(){
    let _serial=FIXTURE_VERIFIER.lock().unwrap();
    let f=First::confirmed(PoolId(array("pool")),OperationId([0x22;16]),array("genesis_digest"),
        &raw("frame"),array("frontier")).unwrap();
    assert!(f.password_matches("first.admin",PUBLIC_INPUT).unwrap());
    assert!(!f.password_matches("first.admin","wrong public fixture").unwrap());
    assert!(!f.password_matches("First.Admin",PUBLIC_INPUT).unwrap());
}

#[test]
#[ignore="two private Linux filesystems with native handles and UUID; target libargon2"]
fn real_first_account_checks_the_original_after_kdf_but_cannot_read_any_file(){
    let _serial=FIXTURE_VERIFIER.lock().unwrap();
    use std::fs as os;
    use crate::test_support::PrivateTemp;
    let a=PrivateTemp::under(&std::env::temp_dir());
    let b=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=a.child("system");let data=a.child("ordinary");let bin=b.child("bin");
    let selection=Selection::open(&sys,&bin,std::slice::from_ref(&data)).unwrap();
    let root=&selection.roots[0].1;let(record,birth,other)=register_fixture(&selection);drop(birth);
    let pool=record.info.id;let gen=record.digest();let empty=fs::frontier(0,0,gen);
    assert!(FirstAuthority::open(&sys,pool).is_err());assert!(OnlineEmpty::open(&sys).is_ok());
    let(frame,floor)=exact_frame(pool,record.info.initialized_by,gen);
    {
        other.try_lock().unwrap();let writer=HeldDir::open(&sys).unwrap();writer.exclusive().unwrap();
        let(stamp,g)=writer.record(READY,MAX_SYSTEM).unwrap().unwrap();assert_eq!(g,record.bytes);
        writer.append_exact(READY,&stamp,g.len() as u64,&frame,MAX_SYSTEM).unwrap();
        other.unlock().unwrap();
    }
    // No lock contention in these observations: a complete but unconfirmed
    // ACCOUNT cannot authenticate, and is not an empty pool either.
    assert!(FirstAuthority::open(&sys,pool).is_err());assert!(OnlineEmpty::open(&sys).is_err());
    {
        other.try_lock().unwrap();let writer=HeldDir::open(&sys).unwrap();writer.exclusive().unwrap();
        let(stamp,all)=writer.record(READY,MAX_SYSTEM).unwrap().unwrap();
        assert_eq!(&all[record.bytes.len()..],frame.as_slice());writer.advance_frontier(READY,&stamp,MAX_SYSTEM,&empty,&floor,||Ok(())).unwrap();
        other.unlock().unwrap();
    }
    let parked=bin.with_file_name("retained-unavailable-recycle");os::rename(&bin,&parked).unwrap();
    assert!(FirstAuthority::open(&sys,PoolId([7;16])).is_err());assert!(OnlineEmpty::open(&sys).is_err());
    let access=FirstAuthority::open(&sys,pool).unwrap();
    assert_eq!(io::Error::from(other.try_lock().unwrap_err()).kind(),io::ErrorKind::WouldBlock);
    assert!(access.authenticate("First.Admin",PUBLIC_INPUT).is_err());
    assert!(access.authenticate("first.admin","wrong public fixture").is_err());
    let p=access.authenticate("first.admin",PUBLIC_INPUT).unwrap();
    for op in [Intent::ManageAccounts,Intent::PublishShares,Intent::ManageReadGrants]{
        let seen=p.observe(op).unwrap();
        assert_eq!(seen.pool,pool);assert_eq!(seen.account,AccountId(array("account")));
        assert_eq!(seen.credential_epoch,1);assert_eq!(seen.credential_operation,OperationId(array("op")));
        assert_eq!(seen.credential_decision.as_slice(),&floor[16..]);
    }
    for op in [Intent::Browse,Intent::ReadCurrent,Intent::ReadVersion]{
        assert_eq!(p.observe(op).unwrap_err().kind(),io::ErrorKind::PermissionDenied);
    }
    assert!(access.authenticate_at("first.admin",PUBLIC_INPUT,||{
        // Slow verification holds the original REGISTRATION SH, not the
        // SYSTEM directory lock; even an empty unknown sidecar blocks issue.
        let short=HeldDir::open(&sys)?;short.exclusive()?;os::write(sys.join("unrecognized-fixture"),[])
    }).is_err());
    assert!(p.observe(Intent::ManageAccounts).is_err());assert!(FirstAuthority::open(&sys,pool).is_err());
    os::rename(sys.join("unrecognized-fixture"),sys.with_file_name("retained-sidecar")).unwrap();
    assert!(p.observe(Intent::ManageAccounts).is_ok());
    {
        // A later unsupported tail is fail-closed for both old and freshly
        // reopened readers; a password match must not authenticate as seq=1.
        let short=HeldDir::open(&sys).unwrap();short.exclusive().unwrap();
        let(stamp,all)=short.record(READY,MAX_SYSTEM).unwrap().unwrap();
        short.append_exact(READY,&stamp,all.len() as u64,&[0x7f],MAX_SYSTEM).unwrap();
    }
    assert!(p.observe(Intent::ManageAccounts).is_err());assert!(FirstAuthority::open(&sys,pool).is_err());
    assert!(root.entry_present(READY).unwrap());drop(access);other.try_lock().unwrap();
    os::rename(parked,bin).unwrap();
}
