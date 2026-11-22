use super::*;
use crate::FileId;

#[test]
fn directory_metadata_accepts_only_three_canonical_segments(){
    let share="ab".repeat(16);let parent="cd".repeat(16);let next="ef".repeat(16);
    let complete=format!("{share}/{parent}/{next}");
    assert_eq!(format!("{PREFIX}{complete}").len(),107); // existing request target cap is 128.
    assert_eq!(Query::parse(&complete),Some(Query{
        share:share.parse().unwrap(),parent:Some(parent.parse().unwrap()),after:Some(Handle(next.parse().unwrap())),filter:None,
    }));
    assert_eq!(Query::parse(&format!("{share}/-/-")),Some(Query{share:share.parse().unwrap(),parent:None,after:None,filter:None}));
    assert_eq!(Query::filtered(&share,&parent,&next,"436164").unwrap().filter,Label::parse("Cad"));
    assert!(Query::filtered(&share,&parent,&next,"-").is_none());
    for x in [format!("{share}/-"),format!("{share}/-/-/"),format!("{share}//-"),format!("{share}/-/-?page=2"),
        format!("{share}/{}/-",parent.to_uppercase()),"-/-/-".to_owned(),format!("{share}/-/{:032}",0)]{
        assert!(Query::parse(&x).is_none(),"{x}");
    }
}

#[test]
fn directory_metadata_writes_the_typed_json_on_the_existing_response_boundary(){
    let p=Page{
        ancestors:vec![Directory{id:DirectoryId([0x12;16]),name:"上级".into()}],
        entries:vec![
            Entry::Directory(Directory{id:DirectoryId([0x13;16]),name:"第一层".into()}),
            Entry::File{id:FileId([0x14;16]),name:"页 \"一\"\n".into(),position:OperationId([0x16;16])},
        ],next:None,scope:[0x17;32],writable:true,
    };
    let mut wire=Vec::new();
    metadata(&p,Some(Handle(OperationId([0x15;16])))).unwrap().write(&mut wire).unwrap();
    let wire=String::from_utf8(wire).unwrap();let (header,body)=wire.split_once("\r\n\r\n").unwrap();
    assert!(header.starts_with("HTTP/1.1 200 OK"));
    assert!(header.contains("Content-Type: application/json; charset=utf-8\r\n"));
    assert!(header.contains(&format!("Content-Length: {}\r\n",body.len())));
    assert!(header.contains("Cache-Control: no-store\r\n"));
    assert!(header.contains("Content-Security-Policy: default-src 'none'; style-src 'self';"));
    assert_eq!(body,format!(concat!("{{\"ancestors\":[{{\"id\":\"12121212121212121212121212121212\",\"name\":\"上级\"}}],",
        "\"entries\":[{{\"kind\":\"directory\",\"id\":\"13131313131313131313131313131313\",\"name\":\"第一层\"}},",
        "{{\"kind\":\"file\",\"id\":\"14141414141414141414141414141414\",\"name\":\"页 \\\"一\\\"\\u000a\",",
        "\"position\":\"16161616161616161616161616161616\"}}],",
        "\"next\":\"15151515151515151515151515151515\",\"scope\":\"{}\",\"writable\":true}}"),"17".repeat(32)));
    let empty=Page{ancestors:Vec::new(),entries:Vec::new(),next:None,scope:[0;32],writable:false};
    assert_eq!(&*metadata(&empty,None).unwrap().body,format!(
        "{{\"ancestors\":[],\"entries\":[],\"next\":null,\"scope\":\"{}\",\"writable\":false}}","00".repeat(32)).as_bytes());
}

#[test]
fn directory_metadata_keeps_only_the_eight_recently_minted_slots(){
    let mut slots=VecDeque::new();
    for i in 0..(PAGE_HANDLES+2){
        remember(&mut slots,i);
        assert!(slots.len()<=PAGE_HANDLES);
    }
    assert_eq!(slots.into_iter().collect::<Vec<_>>(),(2..PAGE_HANDLES+2).collect::<Vec<_>>());
}
