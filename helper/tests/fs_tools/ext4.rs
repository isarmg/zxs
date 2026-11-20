use super::*;

fn base()->[u8;SUPER_LEN]{
    let mut sb=[0;SUPER_LEN];
    set(&mut sb,0,16384);        // inode count
    set(&mut sb,4,1<<17);        // 512 MiB with 4 KiB blocks
    set(&mut sb,0x18,2);
    sb[0x38..0x3a].copy_from_slice(&[0x53,0xef]);
    set(&mut sb,0x5c,0x1004);    // internal journal + supported orphan-file feature
    set(&mut sb,0x60,0x40);      // extents
    set(&mut sb,0xe0,8);         // internal journal inode
    sb
}
fn set(sb:&mut [u8;SUPER_LEN],off:usize,value:u32){sb[off..off+4].copy_from_slice(&value.to_le_bytes());}

#[test]
fn accepts_an_ext_extent_larger_than_a_fat_memfd_and_never_a_truncated_filesystem(){
    let sb=base();
    assert_eq!(frame(&sb,512<<20).unwrap().blocks,131072);
    let longer=frame(&sb,(512<<20)+2048).unwrap();
    assert_eq!(longer.filesystem_bytes,512<<20);
    assert_eq!(longer.bytes,(512<<20)+2048); // trailing bytes aren't inspected as ext
    assert!(frame(&sb,(512<<20)-1).is_err());
    let mut not_ext=sb;not_ext[0x38]=0;
    assert!(frame(&not_ext,512<<20).is_err());
    let mut large=sb;
    set(&mut large,0x60,0xc0);set(&mut large,0x150,1);set(&mut large,4,0);
    assert_eq!(frame(&large,1<<44).unwrap().blocks,1<<32);
    assert!(frame(&large,(1<<44)-1).is_err());
    set(&mut large,0x150,u32::MAX);set(&mut large,0x18,6);
    assert!(frame(&large,u64::MAX).is_err());
}
#[test]
fn no_external_journal_and_no_known_skipped_recovery_at_admission(){
    for (at,value) in [(0x60,0x44),(0x60,0x48),(0xd0,1),(0xe4,1),(0xe0,0),
                       (0x5c,0x1000),(0xe8,7),(0x64,0x10000)]{
        let mut sb=base();set(&mut sb,at,value);
        assert!(frame(&sb,512<<20).is_err(),"{at:#x} {value:#x}");
    }
}
fn transcript()->Vec<u8>{
    let mut out=Vec::new();
    for line in PASSES{out.extend_from_slice(line);out.push(b'\n');}
    out.extend_from_slice(b"/image: 11/16384 files (0.0% non-contiguous), 4568/131072 blocks\n");out
}
#[test]
fn a_zero_exit_requires_all_real_phases_eof_original_totals_and_no_warning(){
    let extent=frame(&base(),512<<20).unwrap();let good=transcript();
    let verdict=|out:&[u8],err:&[u8]|classify(0,out,err,extent).result;
    assert_eq!(verdict(&good,BANNER),ResultKind::NoFindings);
    assert_eq!(verdict(&good,b""),ResultKind::Incomplete);
    assert_eq!(verdict(&good,&[BANNER,b"cannot open mount table\n"].concat()),ResultKind::Incomplete);
    assert_eq!(verdict(&good[..good.len()-1],BANNER),ResultKind::Incomplete);
    assert_eq!(verdict(&good[PASSES[0].len()+1..],BANNER),ResultKind::Incomplete);
    let warning=&b"Warning: skipping journal recovery because doing a read-only filesystem check.\n"[..];
    assert_eq!(verdict(&[warning,&good].concat(),BANNER),ResultKind::Incomplete);
    let unexpected=String::from_utf8(good).unwrap().replace("131072 blocks","131071 blocks");
    assert_eq!(verdict(unexpected.as_bytes(),BANNER),ResultKind::Incomplete);
}
#[test]
fn only_the_unqualified_bit4_is_uncorrected_and_never_claimed_complete(){
    let extent=frame(&base(),512<<20).unwrap();
    assert_eq!(classify(4,b"",BANNER,extent).result,ResultKind::Uncorrected);
    assert_eq!(classify(4,b"",b"bwrap: early setup failed\n",extent).result,ResultKind::Incomplete);
    for status in [1,2,5,6,8,12,16,20,32,36,64,128,132,255]{
        assert_eq!(classify(status,&transcript(),BANNER,extent).exit(),2,"{status}");
    }
}
