//! Payload fixtures only. No authority, objects or reply sender minted here.
use super::super::*;

#[test]
fn root_fs_attributes_has_distinct_small_warning_and_full_shapes(){
    for output in [0,11]{
        let body=root_fs_attributes_body(output,true,255);
        assert_eq!(body.status,0xc000_0004);
        assert_eq!(&body.bytes[..body.len],&[9,0,1,0,8,0,0,0,0,0,0,0,0,0,0,0]);
    }
    let full=root_fs_attributes_body(4096,false,255);
    assert_eq!(full.status,0);
    assert_eq!(&full.bytes[..full.len],&[
        9,0,72,0,18,0,0,0,7,0,0,0,255,0,0,0,6,0,0,0,b'Z',0,b'X',0,b'S',0,
    ]);
    for output in [12usize,13,17,18]{
        let actual=root_fs_attributes_body(output as u32,true,255);
        assert_eq!(actual.status,if output==18{0}else{0x8000_0005});
        assert_eq!(actual.len,8+output);
        assert_eq!(&actual.bytes[2..8],&[72,0,output as u8,0,0,0]);
        assert_eq!(&actual.bytes[8..20],&[7,0,8,0,255,0,0,0,6,0,0,0]);
        assert_eq!(&actual.bytes[20..actual.len],&full.bytes[20..actual.len]);
    }
}

#[test]
fn related_last_close_is_padded_but_has_no_successor(){
    assert_eq!(probe_reply_layout(184,0),(184,184)); // CREATE + MxAc
    assert_eq!(probe_reply_layout(90,1),(96,96)); // full FS Attribute
    assert_eq!(probe_reply_layout(96,1),(96,96)); // full FS Size
    assert_eq!(probe_reply_layout(104,1),(104,104)); // full FS FullSize, no inserted padding
    assert_eq!(probe_reply_layout(80,1),(80,80)); // too small, one empty error context
    assert_eq!(probe_reply_layout(66,1),(72,72)); // SET_INFO success plus six signed zero bytes
    assert_eq!(probe_reply_layout(72,2),(72,0)); // blocked CLOSE, no attrs or next
    assert_eq!(probe_reply_layout(124,2),(128,0)); // final CLOSE plus its four MAC-covered zeros
}

#[test]
fn original_fs_size_is_still_24_bytes_or_an_error_never_a_partial_or_zero_disk(){
    let format=CapacityFormat::Size;
    let units=|total,caller_available|CapacityUnits{total,caller_available,actual_free:caller_available};
    for width in [0,23]{
        let (status,body,len)=root_fs_size_body(format,width,None).unwrap();
        assert_eq!(status,0xc000_0004);
        assert_eq!(&body[..len],&[9,0,1,0,8,0,0,0,0,0,0,0,0,0,0,0]);
        assert!(root_fs_size_body(format,width,Some(units(123,100))).is_err());
    }
    let (status,body,len)=root_fs_size_body(format,24,None).unwrap();
    assert_eq!(status,0xc000_00a3);assert_eq!(&body[..len],&[9,0,0,0,0,0,0,0]);
    for (width,available) in [(24,16u64),(2000,0)]{
        let (status,body,len)=root_fs_size_body(format,width,Some(units(0x123,available))).unwrap();
        assert_eq!(status,0);assert_eq!(len,32);
        let mut exact=[9,0,72,0,24,0,0,0,0x23,1,0,0,0,0,0,0,
            0,0,0,0,0,0,0,0,8,0,0,0,0,2,0,0];
        exact[16..24].copy_from_slice(&available.to_le_bytes());
        assert_eq!(&body[..len],&exact);
        assert_eq!(&body[len..],&[0;8]);
    }
    for (total,caller) in [(0,0),(1,2),(1u64<<63,0)]{
        assert!(root_fs_size_body(format,24,Some(units(total,caller))).is_err());
    }
}

#[test]
fn full_size_is_fixed_32_bytes_and_preserves_three_distinct_counts(){
    let format=CapacityFormat::FullSize;
    let counts=CapacityUnits{total:0x4321,caller_available:7,actual_free:21};
    for offered in [0,24,31]{
        let (status,body,len)=root_fs_size_body(format,offered,None).unwrap();
        assert_eq!(status,0xc000_0004);
        assert_eq!(&body[..len],&[9,0,1,0,8,0,0,0,0,0,0,0,0,0,0,0]);
        assert!(root_fs_size_body(format,offered,Some(counts)).is_err());
    }
    let (status,body,len)=root_fs_size_body(format,32,None).unwrap();
    assert_eq!(status,0xc000_00a3);
    assert_eq!(&body[..len],&[9,0,0,0,0,0,0,0]);
    for output in [32,65_536]{
        let (status,body,len)=root_fs_size_body(format,output,Some(counts)).unwrap();
        assert_eq!((status,len),(0,40));
        assert_eq!(body,[
            9,0,72,0,32,0,0,0,
            0x21,0x43,0,0,0,0,0,0,
            7,0,0,0,0,0,0,0,
            21,0,0,0,0,0,0,0,
            8,0,0,0,0,2,0,0,
        ]);
    }
    for invalid in [CapacityUnits{caller_available:22,..counts},CapacityUnits{actual_free:0x4322,..counts},
        CapacityUnits{total:1<<63,..counts}]{
        assert!(root_fs_size_body(format,32,Some(invalid)).is_err());
    }
}
