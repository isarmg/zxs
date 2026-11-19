use super::*;

const SIZE:u64=1<<20;
fn boot()->[u8;512]{
    // This is only the framing inspected by helper, NOT a fabricated healthy filesystem.
    let mut b=[0;512];b[..3].copy_from_slice(&[0xeb,0x76,0x90]);b[3..11].copy_from_slice(b"EXFAT   ");
    b[72..80].copy_from_slice(&(SIZE/512).to_le_bytes());b[104..106].copy_from_slice(&[0,1]);b[108]=9;
    b[109]=3;b[110]=1;b[510..].copy_from_slice(&[0x55,0xaa]);b
}
#[test]
fn supported_framing_does_not_walk_fat_or_validate_data_or_backup_boot(){
    let mut b=boot();b[64..72].copy_from_slice(&5000u64.to_le_bytes());let f=framing(&b,SIZE).unwrap();
    assert_eq!((f.sector,f.length),(512,SIZE));
    b[108]=12;b[72..80].copy_from_slice(&(SIZE/4096).to_le_bytes());assert_eq!(framing(&b,SIZE).unwrap().sector,4096);
    assert!(framing(&b,SIZE+4096).is_err());assert!(framing(&b,SIZE-4096).is_err());
    for (i,v) in [(0,0),(3,0),(11,1),(104,1),(106,1),(108,8),(109,25),(110,2),(510,0)]{
        let mut bad=boot();bad[i]=v;assert!(framing(&bad,SIZE).is_err());
    }
    b[72..80].copy_from_slice(&u64::MAX.to_le_bytes());assert!(framing(&b,SIZE).is_err());
}
const B:&str="exfatprogs version : 1.2.9\n";
const DONE:&str="/image: clean. directories 1, files 0\n";
const BAD:&str="/image: corrupted. directories 2, files 1\n/image: files corrupted 1, files fixed 0\n";
#[test]
fn version_transcript_exit_counters_and_observed_flags_are_independent(){
    let f=framing(&boot(),SIZE).unwrap();
    let check=|code,lines:&str,errors:&[u8]|classify(code,(B.to_owned()+lines).as_bytes(),errors,f);
    assert_eq!(check(0,DONE,b"").unwrap().exit(),0);
    let d=check(0,DONE,b"failed to read volume label\n").unwrap();
    assert_eq!((d.state,d.counts),(Outcome::ExtraDiagnostics,Some((1,0))));
    let found=check(4,BAD,b"bad filename <untrusted>\x1b[2J").unwrap();
    assert_eq!((found.state,found.counts),(Outcome::Uncorrected,Some((2,1))));
    assert!(!check(4,BAD,b"bad filename <untrusted>\x1b[2J").unwrap().to_string().contains("untrusted"));
    for msg in ["",DONE,"/image: corrupted. directories 2, files 1\n",
        "/image: corrupted. directories 2, files 1\n/image: files corrupted 1, files fixed 1\n"]{
        assert_eq!(check(4,msg,b"failed to verify root directory\n").unwrap().exit(),2);
    }
    for code in [1,2,8,16,32,128]{assert!(check(code,DONE,b"").is_err());}
    assert!(check(0,"",b"").is_err());assert!(check(0,BAD,b"").is_err());
    assert!(check(0,&DONE.replace("1, files","0, files"),b"").is_err());
    assert!(check(0,&DONE.replace("files 0","files 4294967295"),b"").is_err());
    assert!(classify(0,DONE.as_bytes(),b"",f).is_err());
    let mut b=boot();b[106]=6;
    let v=classify(0,(B.to_owned()+DONE).as_bytes(),b"",framing(&b,SIZE).unwrap()).unwrap();
    assert_eq!(v.exit(),1);assert!(v.to_string().contains("volume_dirty=true media_failure_flag=true"));
}
#[test]
fn public_exfat_check_requires_an_independent_hash_and_never_enables_a_web_action_or_repairs(){
    let h="ab".repeat(32);
    let parse=|a:&[&str]|crate::parse(a.iter().map(|s|std::ffi::OsString::from(*s)));
    assert!(matches!(parse(&["check-exfat-copy","/saved/volume.img","--sha256",&h]),Ok(crate::Command::Exfat(_))));
    for bad in [&["check-exfat-copy","/saved/volume.img","-p"][..],
        &["check-exfat-copy","/saved/volume.img","--sha256",&h,"--repair"],
        &["check-exfat-copy","relative.img","--sha256",&h],
        &["check-exfat-copy","/saved/volume.img","--sha256","bad"],
        &["disk-web","--exfat-copy","/saved/volume.img","--sha256",&h]]{
        assert!(parse(bad).is_err());
    }
}
