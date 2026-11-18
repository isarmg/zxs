//! Actual local argv on independent processes/roots. Small literal data
//! proves edit semantics and an original replay; this is not a timing test.
use std::{fs,io::Write,os::unix::fs::MetadataExt,path::Path,process::{Command,Output,Stdio}};
use zxs::{Entity,OperationId,Role};
#[path="unit/support.rs"]mod support;
const F0:&[u8]=b"The quick brown fox jumps over the lazy dog";
const F0_SHA:&str="d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592";
const HELLO_SHA:&str="2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
const ABC_SHA:&str="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
fn op()->OperationId{OperationId::new().unwrap()}
fn cli(sys:&Path,verb:&str,operation:Option<OperationId>,args:&[&str],input:Option<&[u8]>)->Output{
    let mut command=Command::new(env!("CARGO_BIN_EXE_zxs"));command.arg(verb);
    if let Some(id)=operation{command.arg(id.to_string());}
    let mut child=command.arg(sys).args(args).stdin(if input.is_some(){Stdio::piped()}else{Stdio::null()})
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    if let Some(b)=input{child.stdin.take().unwrap().write_all(b).unwrap();}
    child.wait_with_output().unwrap()
}
fn bytes(o:Output)->Vec<u8>{assert!(o.status.success(),"{}",String::from_utf8_lossy(&o.stderr));o.stdout}
fn text(o:Output)->String{String::from_utf8(bytes(o)).unwrap()}
fn token<'a>(line:&'a str,word:&str)->&'a str{
    let mut words=line.split_ascii_whitespace();words.find(|s|*s==word).unwrap_or_else(||panic!("no {word} in {line}"));
    words.next().unwrap()
}
fn refused(o:Output){assert!(!o.status.success()&&o.stdout.is_empty());}
#[test]
#[ignore="private registered FILE A, FILE B, and genuinely separate dedicated recycle filesystems"]
fn two_editors_one_file_three_fixed_histories_and_original_replays_after_custody_round_trip(){
    let a=support::PrivateTemp::under(&std::env::temp_dir());
    let b=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_MOVE_FILE_TMPDIR").unwrap()));
    let rr=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=a.child("system");let old=a.child("A");let new=b.child("B");let bin=rr.child("R");
    assert_ne!(fs::metadata(&old).unwrap().dev(),fs::metadata(&new).unwrap().dev());
    bytes(Command::new(env!("CARGO_BIN_EXE_zxs")).arg("init").arg(op().to_string()).arg(&sys)
        .args([&bin,&old,&new]).output().unwrap());
    let pool=zxs::inspect(&sys).unwrap();let registered=|path:&Path|pool.volumes.iter()
        .find(|v|v.volume.role==Role::File&&v.volume.path==path).unwrap().id.to_string();
    let from=registered(&old);let to=registered(&new);
    let create=op();let birth=text(cli(&sys,"file-create",Some(create),&[&from,"shared"],None));
    let id=token(&birth,"file");let entry=token(&birth,"entry");let history=token(&birth,"history");
    let first_save=op();let first_args=[id,"43",F0_SHA];
    let w0=text(cli(&sys,"file-save",Some(first_save),&first_args,Some(F0)));
    let c0=op();let first=text(cli(&sys,"file-commit",Some(c0),&[id,"1"],None));let f0=token(&first,"node");
    let save_a=op();let a_args=[id,"1","4","5",HELLO_SHA,"12"];
    let wa=text(cli(&sys,"file-write-range",Some(save_a),&a_args,Some(b"hello")));assert_eq!(token(&wa,"revision"),"2");
    let a_bytes=b"The hello br";
    assert_eq!(bytes(cli(&sys,"file-read",None,&[id],None)),a_bytes);
    let c1=op();let first_child=text(cli(&sys,"file-commit",Some(c1),&[id,"2"],None));let i1=token(&first_child,"node");
    assert_eq!(token(&first_child,"parent"),f0);assert_eq!(token(&first_child,"packed-bytes"),"5");

    // A→B froze exactly F0 + I1. I2 will be B's independent later pack.
    let mv=op();let old_site=[id,&from[..],&to[..],"0"];
    let moved=text(cli(&sys,"file-move",Some(mv),&old_site,None));assert!(moved.contains("CompleteCheckedNow"));
    fs::rename(&old,a.0.join("unavailable-A")).unwrap();
    let save_b=op();let b_args=[id,"2","6","3",ABC_SHA,"20"];
    let wb=text(cli(&sys,"file-write-range",Some(save_b),&b_args,Some(b"abc")));assert_eq!(token(&wb,"node"),i1);
    let mut b_bytes=b"The heabc br".to_vec();b_bytes.resize(20,0);
    assert_eq!(bytes(cli(&sys,"file-read",None,&[id],None)),b_bytes);
    let c2=op();let second=text(cli(&sys,"file-commit",Some(c2),&[id,"3"],None));let i2=token(&second,"node");
    assert_ne!(i2,i1);assert_ne!(i2,f0);assert_eq!(token(&second,"parent"),i1);assert_eq!(token(&second,"packed-bytes"),"3");
    let checked=||{
        assert_eq!(bytes(cli(&sys,"file-node-read",None,&[id,f0],None)),F0);
        assert_eq!(bytes(cli(&sys,"file-node-read",None,&[id,i1],None)),a_bytes);
        assert_eq!(bytes(cli(&sys,"file-node-read",None,&[id,i2],None)),b_bytes);
        assert_eq!(bytes(cli(&sys,"file-read",None,&[id],None)),b_bytes);
        assert_eq!(bytes(cli(&sys,"file-node-range",None,&[id,i2,"4","5"],None)),b"heabc");
        assert_eq!(bytes(cli(&sys,"file-read-range",None,&[id,"12","8"],None)),[0;8]);
        assert_eq!(bytes(cli(&sys,"file-node-range",None,&[id,f0,"12","8"],None)),&F0[12..20]);
        refused(cli(&sys,"file-node-range",None,&[id,i1,"12","1"],None));
        let actual=text(cli(&sys,"file-info",None,&[id],None));
        assert_eq!(token(&actual,"file"),id);assert_eq!(token(&actual,"entry"),entry);assert_eq!(token(&actual,"history"),history);
    };
    checked();
    let delete=op();let dargs=[id,&to[..],"1"];let deleted=text(cli(&sys,"file-delete",Some(delete),&dargs,None));
    assert!(deleted.contains("CompleteCheckedNow"));let digest=token(&deleted,"delete-decision");let del=delete.to_string();
    for n in [f0,i1,i2]{refused(cli(&sys,"file-node-read",None,&[id,n],None));}
    let listed=text(cli(&sys,"recycle-list",None,&["8","all","-","-"],None));assert!(listed.contains(id)&&listed.contains(digest));
    let rest=op();let rargs=[id,&del[..],digest,"2",&to[..]];let restored=text(cli(&sys,"file-restore",Some(rest),&rargs,None));
    assert!(restored.contains("CompleteCheckedNow")&&restored.contains(digest));checked();
    assert_eq!(fs::read_dir(new.join("shared")).unwrap().count(),5);
    // Completed requests do not need the old source, the current FILE or
    // the recycle volume. Retrying a SAVE supplies EOF, never a fresh body.
    fs::rename(&new,b.0.join("unavailable-B")).unwrap();fs::rename(&bin,rr.0.join("unavailable-R")).unwrap();
    assert_eq!(text(cli(&sys,"file-save",Some(first_save),&first_args,None)),w0);
    for(op,args,original)in[(save_a,&a_args,&wa),(save_b,&b_args,&wb)]{
        assert_eq!(&text(cli(&sys,"file-write-range",Some(op),args,None)),original);
    }
    for(op,rev,original)in[(c0,"1",&first),(c1,"2",&first_child),(c2,"3",&second)]{
        assert_eq!(&text(cli(&sys,"file-commit",Some(op),&[id,rev],None)),original);
    }
    assert!(text(cli(&sys,"file-move",Some(mv),&old_site,None)).contains("OriginalPublished"));
    assert!(text(cli(&sys,"file-delete",Some(delete),&dargs,None)).contains(digest));
    assert!(text(cli(&sys,"file-restore",Some(rest),&rargs,None)).contains("OriginalPublished"));
    refused(cli(&sys,"file-write-range",Some(save_a),&[id,"2","4","5",HELLO_SHA,"12"],None));
    let wrong="00".repeat(32);refused(cli(&sys,"file-restore-receipt",Some(rest),&[id,&del,&wrong,"2",&to],None));
}
