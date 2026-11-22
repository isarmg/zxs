//! Golden FILE_NAME_INFORMATION bytes. No synthetic storage/name authority.
use super::*;

fn encoded(path:&str,output:u32)->(u32,Vec<u8>){
    let body=NameBody::from_path(path,output).unwrap();
    let mut bytes=Vec::new();body.append(path,&mut bytes);
    assert_eq!(bytes.len(),body.len());(body.status,bytes)
}

#[test]
fn normalized_name_reports_a_full_exact_case_utf16_path_without_a_terminator(){
    const PATH:&str=r"\F\文\A🙂";
    const RAW:[u8;16]=[b'\\',0,b'F',0,b'\\',0,0x87,0x65,b'\\',0,b'A',0,0x3d,0xd8,0x42,0xde];
    let mut expected=vec![9,0,72,0,20,0,0,0,16,0,0,0];
    expected.extend_from_slice(&RAW);
    for output in [20,21,4096]{assert_eq!(encoded(PATH,output),(0,expected.clone()));}
}

#[test]
fn normalized_name_short_and_partial_keep_their_distinct_311_bodies(){
    const PATH:&str=r"\F\文\A🙂";
    for output in [0,4,7]{
        assert_eq!(encoded(PATH,output),(INFO_LENGTH_MISMATCH,
            vec![9,0,1,0,8,0,0,0,0,0,0,0,0,0,0,0]));
    }
    for output in [8,9]{
        assert_eq!(encoded(PATH,output),(BUFFER_OVERFLOW,vec![9,0,72,0,8,0,0,0,16,0,0,0,b'\\',0,b'F',0]));
    }
    // Output 19 truncates only to a complete UTF-16 code unit, not a
    // scalar: FSA copies the exact prefix even when one unit is a surrogate.
    assert_eq!(encoded(PATH,19),(BUFFER_OVERFLOW,vec![9,0,72,0,18,0,0,0,16,0,0,0,
        b'\\',0,b'F',0,b'\\',0,0x87,0x65,b'\\',0,b'A',0,0x3d,0xd8]));
}

#[test]
fn normalized_name_encodes_a_linked_directory_under_the_share_but_not_the_virtual_root(){
    assert_eq!(encoded(r"\F",8),(0,vec![9,0,72,0,8,0,0,0,4,0,0,0,b'\\',0,b'F',0]));
    assert!(NameBody::from_path(r"\",8).is_err());
    assert!(NameBody::from_path(r"\\server\ZXS\F",4096).is_err());
}
