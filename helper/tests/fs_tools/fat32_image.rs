use super::*;

fn boot()->([u8;512],u64) {
    let mut b=[0u8;512];b[11..13].copy_from_slice(&512u16.to_le_bytes());b[13]=1;
    b[14..16].copy_from_slice(&32u16.to_le_bytes());b[16]=2;
    let sectors=32+2*520+65526u32;
    b[32..36].copy_from_slice(&sectors.to_le_bytes());
    b[36..40].copy_from_slice(&520u32.to_le_bytes());b[44..48].copy_from_slice(&2u32.to_le_bytes());
    b[510..].copy_from_slice(&[0x55,0xaa]);(b,u64::from(sectors)*512)
}

#[test]
fn classify_by_layout_not_printable_label_and_reject_trailing_devices_or_tiny_fat(){
    let(b,size)=boot();
    assert_eq!(geometry(&b,size).unwrap().data_clusters,65526);
    assert!(geometry(&b,size+512).is_err());assert!(geometry(&b,size-512).is_err());
    let mut small=b;small[36..40].copy_from_slice(&8u32.to_le_bytes());assert!(geometry(&small,size).is_err());
    let mut fake=b;fake[82..90].copy_from_slice(b"FAT32   ");fake[32..36].copy_from_slice(&40000u32.to_le_bytes());
    assert!(geometry(&fake,40000*512).is_err());
    let mut bad=b;bad[17]=10;assert!(geometry(&bad,size).is_err());
    let mut bad=b;bad[44..48].copy_from_slice(&90000u32.to_le_bytes());assert!(geometry(&bad,size).is_err());
}

#[test]
fn sealed_copy_is_not_the_input_and_wrong_expected_bytes_never_become_a_checked_image(){
    use std::os::unix::fs::MetadataExt;
    let temp=crate::test_area::data_directory();let source=temp.path().join("private-volume.img");
    let (boot,len)=boot();let mut out=File::create_new(&source).unwrap();out.write_all(&boot).unwrap();out.set_len(len).unwrap();drop(out);
    let mut h=Sha256::new();h.update(boot);let zeros=[0u8;32768];let mut left=len-512;
    while left>0{let n=left.min(zeros.len() as u64) as usize;h.update(&zeros[..n]);left-=n as u64;}
    let hash:[u8;32]=h.finalize().into();
    let sealed=Sealed::capture(&source,&hash).unwrap();
    let orig=std::fs::metadata(&source).unwrap();let m=sealed.fd.metadata().unwrap();
    assert_ne!((orig.dev(),orig.ino()),(m.dev(),m.ino()));
    assert!(sealed.fd.write_at(b"X",20).is_err());assert!(sealed.fd.set_len(0).is_err());
    assert_eq!(sealed.geometry.length,len);
    let received=Sealed::from_sealed(sealed.file().try_clone().unwrap(),&hash).unwrap();
    assert_eq!(received.geometry(),sealed.geometry());
    let mut other=hash;other[16]^=1;
    assert!(Sealed::from_sealed(sealed.file().try_clone().unwrap(),&other).is_err());
    let src=File::open(&source).unwrap();let mut actual=[0;512];src.read_exact_at(&mut actual,0).unwrap();assert_eq!(actual,boot);
    let mut wrong=hash;wrong[0]^=1;assert!(Sealed::capture(&source,&wrong).is_err());
}
#[test]
fn supported_512_byte_source_and_credible_4k_layout_outside_the_budget(){
    let(head,len)=boot();assert_eq!(geometry(&head,len).unwrap().sector_bytes,512);
    let mut head=head;head[11..13].copy_from_slice(&4096u16.to_le_bytes());
    let fat_sectors=65u32;let sectors=32+2*fat_sectors+65526u32;
    head[32..36].copy_from_slice(&sectors.to_le_bytes());head[36..40].copy_from_slice(&fat_sectors.to_le_bytes());
    let len=u64::from(sectors)*4096;let sample=geometry(&head,len).unwrap();
    assert_eq!((sample.sector_bytes,sample.data_clusters),(4096,65526));assert!(len>MAX);
    // A sparse private file carries plausible geometry, not evidence of filesystem health.
    let area=crate::test_area::data_directory();let src=area.path().join("large-4k.img");
    let mut f=File::create_new(&src).unwrap();f.write_all(&head).unwrap();f.set_len(len).unwrap();
    let err=Sealed::capture(&src,&[0;32]).err().expect("over the selected copy budget");
    assert_eq!(err.kind(),io::ErrorKind::Unsupported);assert_eq!(f.metadata().unwrap().len(),len);
}

#[test]
fn worker_rejects_a_named_file_or_an_unfinished_anonymous_copy_before_reading(){
    let t=tempfile::tempdir().unwrap();let named=File::create_new(t.path().join("not-a-memfd")).unwrap();
    named.set_len(512).unwrap();assert!(Sealed::from_sealed(named,&[0;32]).is_err());
    let unsealed=File::from(memfd_create(c"unfinished",MemfdFlags::CLOEXEC|MemfdFlags::NOEXEC_SEAL).unwrap());
    unsealed.set_len(512).unwrap();assert!(Sealed::from_sealed(unsealed,&[0;32]).is_err());
}
