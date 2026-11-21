//! Fixed body fixtures, not a FILE authority or signed-success producer.
use super::*;

#[test]
fn file_standard_is_fixed_and_a_short_request_has_the_311_default_context(){
    let file=FileStandardFields::File{allocation:4096,eof:5,delete_pending:false};
    for capacity in [0,23]{
        let (status,body,len)=file_standard_body(capacity,file);
        assert_eq!(status,0xc000_0004);
        assert_eq!(&body[..len],&[9,0,1,0,8,0,0,0,0,0,0,0,0,0,0,0]);
    }
    for capacity in [24,4096]{
        let (status,body,len)=file_standard_body(capacity,file);
        assert_eq!(status,0);assert_eq!(len,32);
        assert_eq!(body,[
            9,0,72,0,24,0,0,0,
            0,16,0,0,0,0,0,0,5,0,0,0,0,0,0,0,
            1,0,0,0,0,0,0,0,
        ]);
    }
}

#[test]
fn file_standard_counts_the_one_logical_link_only_when_unmarked(){
    // EndOfFile=4097 of this logical view; not the rounded allocation.
    for pending in [false,true]{
        let fields=FileStandardFields::File{allocation:8192,eof:4097,delete_pending:pending};
        let (status,body,len)=file_standard_body(24,fields);
        assert_eq!((status,len),(0,32));
        assert_eq!(&body[8..24],&[0,32,0,0,0,0,0,0,1,16,0,0,0,0,0,0]);
        assert_eq!(&body[24..],&[u8::from(!pending),0,0,0,u8::from(pending),0,0,0]);
    }
}

#[test]
fn ordinary_directory_standard_uses_one_logical_link_and_no_data_stream(){
    for capacity in [0,23]{
        let (status,body,len)=file_standard_body(capacity,FileStandardFields::Directory);
        assert_eq!(status,0xc000_0004);
        assert_eq!(&body[..len],&[9,0,1,0,8,0,0,0,0,0,0,0,0,0,0,0]);
    }
    for capacity in [24,4096]{
        let (status,body,len)=file_standard_body(capacity,FileStandardFields::Directory);
        assert_eq!((status,len),(0,32));
        assert_eq!(body,[
            9,0,72,0,24,0,0,0,
            0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
            1,0,0,0,0,1,0,0,
        ]);
    }
}

#[test]
fn ordinary_file_attribute_tag_is_two_words_or_a_default_error_context(){
    use crate::smb::backend::FILE_ATTRIBUTES;
    for output in [0,7]{
        let (status,body)=file_attribute_tag_body(output,FILE_ATTRIBUTES,0);
        assert_eq!(status,0xc000_0004);
        assert_eq!(body,[9,0,1,0,8,0,0,0,0,0,0,0,0,0,0,0]);
    }
    for output in [8,4096]{
        let (status,body)=file_attribute_tag_body(output,FILE_ATTRIBUTES,0);
        assert_eq!(status,0);
        assert_eq!(body,[9,0,72,0,8,0,0,0,0x20,0,0,0,0,0,0,0]);
    }
}

#[test]
fn directory_attribute_tag_uses_the_create_attribute_and_no_reparse_point(){
    use crate::smb::backend::DIRECTORY_ATTRIBUTES;
    for output in [0,7]{
        let (status,body)=file_attribute_tag_body(output,DIRECTORY_ATTRIBUTES,0);
        assert_eq!(status,0xc000_0004);
        assert_eq!(body,[9,0,1,0,8,0,0,0,0,0,0,0,0,0,0,0]);
    }
    for output in [8,4096]{
        let (status,body)=file_attribute_tag_body(output,DIRECTORY_ATTRIBUTES,0);
        assert_eq!(status,0);
        assert_eq!(body,[9,0,72,0,8,0,0,0,0x10,0,0,0,0,0,0,0]);
    }
}
