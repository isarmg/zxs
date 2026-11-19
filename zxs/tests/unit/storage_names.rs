//! Source fixtures for the deterministic name codec only. No mount, SYSTEM,
//! filesystem namespace or SMB client is opened by these tests.
use super::*;
const A:&str="0123456789abcdef0123456789abcdef";
const B:&str="0123456789abcdef0123456789abcdee";
fn file(n:&str,id:&str)->String{
    let human=FileName::parse(n.as_bytes()).unwrap();assert_eq!(human.display(),n);
    human.container(id.parse().unwrap()).unwrap().name().to_str().unwrap().to_owned()
}
#[test]
fn the_complete_file_id_precedes_only_the_last_nonleading_extension(){
    for(input,stem,ext)in [("方案.xlsx","方案",".xlsx"),("a.tar.gz","a.tar",".gz"),
        ("README","README",""),(".env",".env",""),(".env.local",".env",".local"),
        ("a..z","a.",".z"),("文 件.XLSX","文 件",".XLSX"),("a-b.a-b","a-b",".a-b")]{
        let x=file(input,A);let y=file(input,B);assert_eq!(x,format!("{stem}-{A}{ext}"));assert_eq!(y,format!("{stem}-{B}{ext}"));
        assert_ne!(x,y);assert_eq!(x.len(),input.len()+33);
    }
    let n=format!("方案-{A}.xlsx");assert_eq!(file(&n,B),format!("方案-{A}-{B}.xlsx"));
    assert!(FileName::parse(b"x").unwrap().container(FileId([0;16])).is_err());
}
#[test]
fn exact_spelling_is_not_case_or_unicode_folding(){
    let first=FileName::parse(b"Report.xlsx").unwrap();let other=FileName::parse(b"report.xlsx").unwrap();
    let miss=FileName::parse(b"REPORT.xlsx").unwrap();
    let mut lookup=std::collections::BTreeMap::new();lookup.insert(first,A);lookup.insert(other,B);
    assert_eq!(lookup.get(&first),Some(&A));assert_eq!(lookup.get(&other),Some(&B));assert_eq!(lookup.get(&miss),None);
    assert_ne!(file(first.display(),A),file(other.display(),A)); // even at the same identity, no lowercasing
    let nfc="Café.txt";let nfd="Cafe\u{301}.txt";
    assert_ne!(FileName::parse(nfc.as_bytes()).unwrap(),FileName::parse(nfd.as_bytes()).unwrap());
    assert_eq!(file(nfc,A),format!("Café-{A}.txt"));assert_eq!(file(nfd,A),format!("Cafe\u{301}-{A}.txt"));
    let d=DirectoryName::parse("方案".as_bytes()).unwrap();assert_eq!(d.display(),"方案");assert_eq!(d.leaf().as_bytes(),"方案".as_bytes());
}
#[test]
fn exact_private_grammar_is_disjoint_from_file_terminals_not_from_allowed_display_names(){
    let t:OperationId=A.parse().unwrap();
    let isolation=TicketLeaf::isolation(t).unwrap();let q=isolation.name();assert_eq!(q.to_bytes().len(),ISOLATE_LEN);
    assert_eq!(q.to_str().unwrap(),format!(".zxs-moveout-{A}_"));assert_eq!(TicketLeaf::parse(q.to_bytes()),Some(isolation));
    assert_eq!(isolation.as_stage(),None);assert_eq!(super::super::material::private_slot(q.to_bytes()),None);
    assert!(DirectoryName::parse(q.to_bytes()).is_err());assert!(FileName::parse(q.to_bytes()).is_ok());
    for i in 0..8{
        let stage=TicketLeaf::stage(t,i).unwrap();let encoded=stage.name();let n=encoded.to_str().unwrap();
        assert_eq!(n.len(),STAGE_LEN);assert_eq!(n,format!(".zxs-new-{A}-{i}"));
        assert_eq!(TicketLeaf::parse(n.as_bytes()),Some(stage));assert_eq!(super::super::material::private_slot(n.as_bytes()),Some((t,i)));
        assert!(DirectoryName::parse(n.as_bytes()).is_err());assert!(FileName::parse(n.as_bytes()).is_ok());
        for display in [n,q.to_str().unwrap(),".zxs-move-out",".zxs-new",".env","x..z","name-with-id",".a.Z"]{
            let terminal=file(display,A);assert!(TicketLeaf::parse(terminal.as_bytes()).is_none());
        }
    }
    for n in [format!(".ZXS-new-{A}-0"),format!(".zxs-new-{A}-8"),format!(".zxs-new-{}-0","0".repeat(32)),
        format!(".zxs-new-{}-1",A.to_ascii_uppercase()),format!(".zxs-move-out-{A}"),format!(".zxs-moveout-{A}-"),
        ".env".to_owned(),".zxs-move-out".to_owned()]{
        assert!(TicketLeaf::parse(n.as_bytes()).is_none());assert_eq!(DirectoryName::parse(n.as_bytes()).unwrap().display(),n);
    }
    assert!(TicketLeaf::stage(t,8).is_err());assert!(TicketLeaf::stage(OperationId([0;16]),1).is_err());
    assert!(TicketLeaf::isolation(OperationId([0;16])).is_err());
}
#[test]
fn budgets_are_utf8_bytes_and_invalid_input_is_not_truncated_repaired_or_reinterpreted(){
    for s in ["",".","..","tail.","tail ","a/b","a\\b","a:b","a<b","a>b","a?b","a*b","a|b","a\"b","a\0b","a\nb","\u{85}"]{
        assert!(FileName::parse(s.as_bytes()).is_err());assert!(DirectoryName::parse(s.as_bytes()).is_err());
    }
    for raw in [&[0xff][..],&[0xed,0xa0,0x80][..]]{assert!(FileName::parse(raw).is_err());assert!(DirectoryName::parse(raw).is_err());}
    for s in ["n".repeat(FILE_NAME_MAX),"界".repeat(74)]{assert_eq!(s.len(),222);
        assert_eq!(file(&s,A).as_bytes().len(),COMPONENT_MAX);let over=format!("{s}x");
        assert!(FileName::parse(over.as_bytes()).is_err());assert!(DirectoryName::parse(over.as_bytes()).is_ok());}
    let sheet=format!("{}.xlsx","方".repeat(72));assert_eq!(sheet.len(),221);assert_eq!(file(&sheet,A).len(),254);
    let too_long=format!("{}.xlsx","方".repeat(73));assert_eq!(too_long.len(),224);
    assert!(FileName::parse(too_long.as_bytes()).is_err()); // a human-visible character is not one native byte
    let full="x".repeat(255);assert_eq!(DirectoryName::parse(full.as_bytes()).unwrap().leaf().as_bytes(),full.as_bytes());
    assert!(DirectoryName::parse(format!("{full}x").as_bytes()).is_err());
}
