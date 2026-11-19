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
    assert_eq!(fs::read_dir(new.join(support::physical("shared",&id))).unwrap().count(),5);
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

#[test]
#[ignore="source-only end-to-end case-sensitive names; real private registered FILE and separate recycle filesystems required"]
fn exact_display_lookup_two_equal_spelling_histories_and_one_unchanged_name_per_container(){
    let h=support::PrivateTemp::under(&std::env::temp_dir());
    let rh=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=h.child("system");let a=h.child("A");let b=h.child("B");let c=h.child("C");let bin=rh.child("dedicated-R");
    let registry=zxs::initialize(op(),&sys,&bin,&[a.clone(),b.clone(),c.clone()]).unwrap();
    let volume=|p:&Path|registry.volumes.iter().find(|r|r.volume.path==p).unwrap().id.to_string();
    let(va,vb,vc)=(volume(&a),volume(&b),volume(&c));
    let lookup=|v:&str,name:&str|cli(&sys,"file-lookup",None,&[v,name],None);
    let at=|v:&str,name:&str,body:&[u8],sha:&str|{
        let create=op();let receipt=text(cli(&sys,"file-create",Some(create),&[v,name],None));
        let id=token(&receipt,"file").to_owned();assert_eq!(token(&text(lookup(v,name)),"file"),id);
        bytes(cli(&sys,"file-save",Some(op()),&[&id,&body.len().to_string(),sha],Some(body)));
        let first=text(cli(&sys,"file-commit",Some(op()),&[&id,"1"],None));let node=token(&first,"node").to_owned();
        assert_eq!(bytes(cli(&sys,"file-node-read",None,&[&id,&node],None)),body);(id,node)
    };
    let u=at(&va,"Report.xlsx",b"abc",ABC_SHA);let l=at(&va,"report.xlsx",b"hello",HELLO_SHA);
    let other=at(&vb,"Report.xlsx",b"hello",HELLO_SHA);assert_ne!(u.0,l.0);assert_ne!(u.0,other.0);
    let upper=format!("Report-{}.xlsx",u.0);let lower=format!("report-{}.xlsx",l.0);let duplicate=format!("Report-{}.xlsx",other.0);
    let leaves=|p:&Path|fs::read_dir(p).unwrap().map(|e|e.unwrap().file_name().into_string().unwrap()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(leaves(&a),[upper.clone(),lower.clone()].into_iter().collect());assert_eq!(leaves(&b),[duplicate.clone()].into_iter().collect());
    refused(lookup(&va,"REPORT.xlsx"));refused(lookup(&va,&upper)); // administrative leaf is not a second route
    for verb in ["file-list","dir-list"]{let args=if verb=="dir-list"{vec![va.as_str(),"root"]}else{vec![va.as_str()]};
        let listing=text(cli(&sys,verb,None,&args,None));assert!(listing.contains("name Report.xlsx")&&listing.contains("name report.xlsx"));
        assert!(!listing.contains(&upper)&&!listing.contains(&lower));}
    refused(cli(&sys,"dir-mkdir",Some(op()),&[&va,"root","Report.xlsx"],None)); // exact logical collision
    refused(cli(&sys,"dir-mkdir",Some(op()),&[&va,"root",&upper],None));       // exact native collision, different logical name
    bytes(cli(&sys,"dir-mkdir",Some(op()),&[&vc,"root","Report.xlsx"],None));
    bytes(cli(&sys,"dir-mkdir",Some(op()),&[&vb,"root",&upper],None)); // collides with U only, not with this different FILE
    let del=|file:&str,v:&str,e:&str|{
        let op=op();let reply=text(cli(&sys,"file-delete",Some(op),&[file,v,e],None));assert!(reply.contains("CompleteCheckedNow"));
        (op,token(&reply,"delete-decision").to_owned())
    };
    let restore=|op_id,file:&str,d:&(OperationId,String),e:&str,v:&str|{
        cli(&sys,"file-restore",Some(op_id),&[file,&d.0.to_string(),&d.1,e,v],None)
    };
    let du=del(&u.0,&va,"0");let dl=del(&l.0,&va,"0");let db=del(&other.0,&vb,"0");
    assert_eq!(leaves(&bin),[upper.clone(),lower.clone(),duplicate.clone()].into_iter().collect());
    refused(lookup(&va,"Report.xlsx"));refused(lookup(&va,"report.xlsx"));
    let before=fs::read(sys.join("REGISTRATION")).unwrap();let items=leaves(&bin);
    for target in [&vb,&vc]{ // first conflicts only physically, second only logically
        let o=op();refused(restore(o,&u.0,&du,"1",target));assert!(zxs::recycle_receipt(&sys,o,1,zxs::RecycleAction::Restore{
            deleted:zxs::DeletionKey::from_hex(u.0.parse().unwrap(),du.0,&du.1).unwrap(),
            target:zxs::RestoreTarget::Volume(target.parse().unwrap())}).unwrap().is_none());
        assert_eq!(fs::read(sys.join("REGISTRATION")).unwrap(),before);
        assert!(!sys.join("FILE-ALLOCATION").exists()&&!sys.join("FILE-INTENT").exists());assert_eq!(leaves(&bin),items);}
    // Reverse the three original deletes. A stored spelling is never parsed
    // into a claimant; the delete Op and its digest remain mandatory.
    bytes(restore(op(),&other.0,&db,"1","original"));bytes(restore(op(),&l.0,&dl,"1","original"));
    let old_restore=op();bytes(restore(old_restore,&u.0,&du,"1","original"));assert!(leaves(&bin).is_empty());
    for (v,name,row,native,payload) in [(&va,"Report.xlsx",&u,&upper,&b"abc"[..]),(&va,"report.xlsx",&l,&lower,&b"hello"[..]),(&vb,"Report.xlsx",&other,&duplicate,&b"hello"[..])]{
        assert_eq!(token(&text(lookup(v,name)),"file"),row.0);
        assert_eq!(bytes(cli(&sys,"file-read",None,&[&row.0],None)),payload);
        assert_eq!(bytes(cli(&sys,"file-node-read",None,&[&row.0,&row.1],None)),payload);
        let parent=if v==&va{&a}else{&b};assert!(parent.join(native).join("FILE").is_file());
    }
    let again=del(&u.0,&va,"2");assert_ne!(du,again);assert_eq!(leaves(&bin),[upper.clone()].into_iter().collect());
    assert!(text(restore(old_restore,&u.0,&du,"1","original")).contains("OriginalPublished"));
    refused(restore(op(),&u.0,&du,"3","original"));refused(lookup(&va,"Report.xlsx"));assert_eq!(leaves(&bin),[upper.clone()].into_iter().collect());
    bytes(restore(op(),&u.0,&again,"3","original"));assert_eq!(bytes(cli(&sys,"file-node-read",None,&[&u.0,&u.1],None)),b"abc");
    // Exact Unicode business names, without case mapping or NFC/NFD lookup.
    let cn=at(&va,"方案.xlsx",b"abc",ABC_SHA);assert!(a.join(format!("方案-{}.xlsx",cn.0)).is_dir());
    let composed=at(&vc,"é.txt",b"hello",HELLO_SHA);
    refused(lookup(&vc,"e\u{301}.txt"));
    let decomposed=at(&vc,"e\u{301}.txt",b"abc",ABC_SHA);assert_ne!(composed.0,decomposed.0);
    assert_eq!(token(&text(lookup(&vc,"é.txt")),"file"),composed.0);
}

#[test]
#[ignore="native private ordinary roots on separate filesystems and an independent recycle filesystem; CLI source, no automatic run"]
fn three_scoped_parents_keep_separate_file_identities_through_history_move_and_recycle(){
    let a=support::PrivateTemp::under(&std::env::temp_dir());
    let b=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_MOVE_FILE_TMPDIR").unwrap()));
    let rr=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=a.child("system");let old=a.child("A");let new=b.child("B");let bin=rr.child("R");
    assert_ne!(fs::metadata(&old).unwrap().dev(),fs::metadata(&new).unwrap().dev());
    bytes(Command::new(env!("CARGO_BIN_EXE_zxs")).arg("init").arg(op().to_string()).arg(&sys).args([&bin,&old,&new]).output().unwrap());
    let pool=zxs::inspect(&sys).unwrap();let vol=|p:&Path|pool.volumes.iter().find(|v|v.volume.role==Role::File&&v.volume.path==p).unwrap().id.to_string();
    let va=vol(&old);let vb=vol(&new);let name="方案.xlsx";
    let md=|v:&str,p:&str,n:&str|{
        let s=text(cli(&sys,"dir-mkdir",Some(op()),&[v,p,n],None));
        token(s.lines().find(|l|l.starts_with("directory ")).unwrap(),"directory").to_owned()
    };
    let al=md(&va,"root","left");let ar=md(&va,"root","right");
    let bl=md(&vb,"root","left");let br=md(&vb,"root","right");
    let get=|v:&str,p:&str|cli(&sys,"file-lookup-at",None,&[v,p,name],None);
    let make=|v:&str,p:&str,create|{let s=text(cli(&sys,"file-create-at",Some(create),&[v,p,name],None));
        (token(&s,"file").to_owned(),token(&s,"entry").to_owned(),token(&s,"history").to_owned())};
    let id_op=op();let(file,entry,history)=make(&va,&al,id_op);
    let root_file=make(&va,"root",op()).0;let sibling=make(&va,&ar,op()).0;let other=make(&vb,&br,op()).0;
    let unique=[&file,&root_file,&sibling,&other].into_iter().collect::<std::collections::BTreeSet<_>>();assert_eq!(unique.len(),4);
    assert_eq!(token(&text(get(&va,&al)),"file"),file);assert_eq!(token(&text(get(&va,&ar)),"file"),sibling);
    assert_eq!(token(&text(cli(&sys,"file-lookup",None,&[&va,name],None)),"file"),root_file);
    let root_list=text(cli(&sys,"file-list",None,&[&va],None));assert!(root_list.contains(&root_file)&&!root_list.contains(&file)&&!root_list.contains(&sibling));
    let child_list=text(cli(&sys,"dir-list",None,&[&va,&al],None));assert!(child_list.contains(&file)&&!child_list.contains(&root_file)&&!child_list.contains(&sibling));
    refused(cli(&sys,"file-create",Some(id_op),&[&va,name],None));
    refused(cli(&sys,"dir-mkdir",Some(op()),&[&va,&al,name],None));
    bytes(cli(&sys,"file-save",Some(op()),&[&other,"5",HELLO_SHA],Some(b"hello")));
    let bn=text(cli(&sys,"file-commit",Some(op()),&[&other,"1"],None));let other_node=token(&bn,"node");
    let sibling_path=new.join("right").join(format!("方案-{other}.xlsx"));
    let observed=(fs::metadata(&sibling_path).unwrap().dev(),fs::metadata(&sibling_path).unwrap().ino());
    let saved=op();bytes(cli(&sys,"file-save",Some(saved),&[&file,"43",F0_SHA],Some(F0)));
    let c0=op();let f=text(cli(&sys,"file-commit",Some(c0),&[&file,"1"],None));let f0=token(&f,"node");
    bytes(cli(&sys,"file-write-range",Some(op()),&[&file,"1","4","5",HELLO_SHA,"12"],Some(b"hello")));
    let f=text(cli(&sys,"file-commit",Some(op()),&[&file,"2"],None));let i1=token(&f,"node");
    assert_eq!(token(&f,"parent"),f0);
    let mv=op();let args=[&file[..],&va,&al,&vb,&bl,"0"];
    refused(cli(&sys,"file-move-at",Some(mv),&[&file,&va,&ar,&vb,&bl,"0"],None));
    refused(cli(&sys,"file-move-at",Some(mv),&[&file,&va,&al,&vb,&br,"0"],None));
    assert!(!sys.join("FILE-ALLOCATION").exists());let out=text(cli(&sys,"file-move-at",Some(mv),&args,None));
    assert!(out.contains("CompleteCheckedNow")&&out.contains(&al)&&out.contains(&bl));refused(get(&va,&al));
    let after=text(get(&vb,&bl));assert_eq!((token(&after,"file"),token(&after,"entry"),token(&after,"history")),(&file[..],&entry[..],&history[..]));
    // Reuse the old name for a new and entirely unrelated FILE, then lose the old volume.
    let reuse=make(&va,&al,op()).0;assert_ne!(reuse,file);
    fs::rename(&old,a.0.join("unavailable-A")).unwrap();
    assert!(text(cli(&sys,"file-move-at-receipt",Some(mv),&args,None)).contains("OriginalPublished"));
    refused(cli(&sys,"file-move-at-receipt",Some(mv),&[&file,&va,&ar,&vb,&bl,"0"],None));
    refused(cli(&sys,"file-move-receipt",Some(mv),&[&file,&va,&vb,"0"],None));
    bytes(cli(&sys,"file-write-range",Some(op()),&[&file,"2","6","3",ABC_SHA,"20"],Some(b"abc")));
    let f=text(cli(&sys,"file-commit",Some(op()),&[&file,"3"],None));let i2=token(&f,"node");
    assert_eq!(token(&f,"parent"),i1);assert_eq!(token(&f,"packed-bytes"),"3");
    let mut latest=b"The heabc br".to_vec();latest.resize(20,0);
    let verify=||{
        for(n,want)in[(f0,F0),(i1,&b"The hello br"[..]),(i2,latest.as_slice())]{
            assert_eq!(bytes(cli(&sys,"file-node-read",None,&[&file,n],None)),want);}
        assert_eq!(bytes(cli(&sys,"file-read",None,&[&file],None)),latest);
        assert_eq!(bytes(cli(&sys,"file-read-range",None,&[&file,"12","8"],None)),vec![0;8]);
    };
    verify();let native=format!("方案-{file}.xlsx");assert_eq!(new.join("left").join(&native).read_dir().unwrap().count(),5);
    let del=op();let reply=text(cli(&sys,"file-delete-at",Some(del),&[&file,&vb,&bl,"1"],None));
    let digest=token(&reply,"delete-decision");let d=del.to_string();assert!(reply.contains(&format!("deletion-source {vb}:{bl}")));
    assert!(bin.join(&native).is_dir());refused(get(&vb,&bl));refused(cli(&sys,"file-node-read",None,&[&file,f0],None));
    let page=text(cli(&sys,"recycle-list",None,&["16",&vb,"-","-"],None));assert!(page.contains(&file)&&page.contains(&bl)&&!page.contains(&br));
    assert!(text(cli(&sys,"file-delete-at-receipt",Some(del),&[&file,&vb,&bl,"1"],None)).contains("OriginalPublished"));
    refused(cli(&sys,"file-delete-receipt",Some(del),&[&file,&vb,"1"],None));
    let restore=op();
    refused(cli(&sys,"file-restore",Some(restore),&[&file,&d,digest,"2","original"],None));
    refused(cli(&sys,"file-restore-at",Some(restore),&[&file,&d,digest,"2",&vb,&br],None));
    // An installed exact display-name directory in the real deletion parent
    // blocks its original site. An identically spelled sibling is not that parent.
    let blocker=md(&vb,&bl,name);let s_before=fs::read(sys.join("REGISTRATION")).unwrap();
    refused(cli(&sys,"file-restore-at",Some(restore),&[&file,&d,digest,"2","original"],None));
    assert!(!sys.join("FILE-ALLOCATION").exists()&&!sys.join("FILE-INTENT").exists());
    assert_eq!(fs::read(sys.join("REGISTRATION")).unwrap(),s_before);
    assert!(bin.join(&native).is_dir());let listing=text(cli(&sys,"dir-list",None,&[&vb,&bl],None));assert!(listing.contains(&blocker));
    let sub=md(&vb,"root","recovered");let dest=[&file[..],&d,digest,"2",&vb,&sub];
    let rec=text(cli(&sys,"file-restore-at",Some(restore),&dest,None));assert!(rec.contains("CompleteCheckedNow"));
    assert_eq!(token(&text(get(&vb,&sub)),"history"),history);assert!(new.join("recovered").join(&native).is_dir());assert!(!bin.join(&native).exists());
    verify();assert_eq!(bytes(cli(&sys,"file-node-read",None,&[&other,other_node],None)),b"hello");
    assert_eq!((fs::metadata(&sibling_path).unwrap().dev(),fs::metadata(&sibling_path).unwrap().ino()),observed);
    let del2=op();let reply=text(cli(&sys,"file-delete-at",Some(del2),&[&file,&vb,&sub,"3"],None));let digest2=token(&reply,"delete-decision");
    assert_ne!(digest,digest2);refused(cli(&sys,"file-restore-at",Some(op()),&[&file,&d,digest,"4","original"],None));
    assert!(text(cli(&sys,"file-restore-at",Some(restore),&dest,None)).contains("OriginalPublished"));assert!(bin.join(&native).is_dir());
    let reopen=op();let d2=del2.to_string();bytes(cli(&sys,"file-restore-at",Some(reopen),&[&file,&d2,digest2,"4","original"],None));
    // Explicitly selecting the same original parent is the very same request.
    assert!(text(cli(&sys,"file-restore-at-receipt",Some(reopen),&[&file,&d2,digest2,"4",&vb,&sub],None)).contains("OriginalPublished"));
    refused(cli(&sys,"file-restore-at-receipt",Some(reopen),&[&file,&d2,digest2,"4",&vb,&bl],None));
    verify();assert_eq!(token(&text(cli(&sys,"file-create-at",Some(id_op),&[&va,&al,name],None)),"file"),file);
    fs::rename(a.0.join("unavailable-A"),&old).unwrap();assert_eq!(token(&text(get(&va,&al)),"file"),reuse);
    assert_eq!(token(&text(get(&va,&ar)),"file"),sibling);
}
