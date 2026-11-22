//! The bounded context encoding only. No synthetic OPEN, Core or signer.
use super::super::*;

#[test]
fn unconditional_root_mxac_is_one_aligned_context_with_its_real_mask(){
    let mut reply=[0x55;MAX_SMALL_REPLY];
    encode_open_contexts(&mut reply,OpenContexts{maximum:Some(0x0010_00a1),disk_id:None});
    assert!(reply[..144].iter().all(|b|*b==0x55));
    assert_eq!(&reply[144..184],&[
        152,0,0,0,32,0,0,0, // fixed body context offset and length
        0,0,0,0,16,0,4,0,0,0,24,0,8,0,0,0,
        b'M',b'x',b'A',b'c',0,0,0,0,0,0,0,0,0xa1,0,0x10,0,
    ]);
    assert!(reply[184..].iter().all(|b|*b==0x55));
}

#[test]
fn qfid_supplies_the_same_fixed_index_as_file_internal_information(){
    let mut reply=[0x55;MAX_SMALL_REPLY];
    let both=OpenContexts{maximum:Some(0x0010_00a1),disk_id:Some((1,1))};
    assert_eq!(CREATE_FIXED+both.length(),240);
    encode_open_contexts(&mut reply,both);
    assert_eq!(&reply[144..152],&[152,0,0,0,88,0,0,0]);
    assert_eq!(&reply[152..156],&[32,0,0,0]);
    assert_eq!(&reply[184..208],&[
        0,0,0,0,16,0,4,0,0,0,24,0,32,0,0,0,b'Q',b'F',b'i',b'd',0,0,0,0,
    ]);
    let (status,body)=internal_body(8,1);
    assert_eq!(status,0);
    assert_eq!(&body[..8],&[9,0,72,0,8,0,0,0]);
    assert_eq!(&reply[208..216],&body[8..16]);
    assert_eq!(&reply[216..224],&1u64.to_le_bytes());
    assert_eq!(&reply[224..240],&[0;16]);

    let alone=OpenContexts{maximum:None,disk_id:Some((1,1))};
    assert_eq!(CREATE_FIXED+alone.length(),208);
    encode_open_contexts(&mut reply,alone);
    assert_eq!(&reply[144..156],&[152,0,0,0,56,0,0,0,0,0,0,0]);
    assert_eq!(&reply[168..172],b"QFid");
    for short in 0..8{
        let (status,body)=internal_body(short,1);
        assert_eq!(status,0xc000_0004);
        assert_eq!(body,[9,0,1,0,8,0,0,0,0,0,0,0,0,0,0,0]);
    }
}

#[test]
fn ordinary_file_wire_index_is_the_full_same_eight_bytes_not_a_file_handle(){
    // Encoding fixture only; proving this file number belongs to an
    // original Core FILE is still required before the real sender.
    let exact=0xfedc_ba98_7654_3210u64;
    let mut create=[0x55;MAX_SMALL_REPLY];
    encode_open_contexts(&mut create,OpenContexts{maximum:None,disk_id:Some((exact,1))});
    let (status,info)=internal_body(8,exact);
    assert_eq!(status,0);
    assert_eq!(&info[8..16],&[0x10,0x32,0x54,0x76,0x98,0xba,0xdc,0xfe]);
    assert_eq!(&create[176..184],&info[8..16]);
    assert_eq!(&create[184..192],&1u64.to_le_bytes());
    assert_eq!(&create[192..208],&[0;16]);
    assert_eq!(&create[128..144],&[0x55;16]); // not either SMB2_FILEID half
}
