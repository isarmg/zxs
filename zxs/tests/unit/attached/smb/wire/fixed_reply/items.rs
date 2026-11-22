#[test]
fn asynchronous_move_overlays_only_its_flags_and_routing_union(){
    let mut raw=[0u8;MAX_SMALL_REPLY];
    raw[16]=9; // existing signed server reply flags
    raw[24..32].copy_from_slice(&23u64.to_le_bytes());
    raw[36..40].copy_from_slice(&53u32.to_le_bytes());
    raw[40..48].copy_from_slice(&17u64.to_le_bytes());
    let fixed=raw;
    move_async_header(&mut raw,NonZeroU64::new(24).unwrap());
    assert_eq!(raw[16..20],11u32.to_le_bytes());
    assert_eq!(raw[32..40],24u64.to_le_bytes());
    assert_eq!(raw[..16],fixed[..16]);
    assert_eq!(raw[20..32],fixed[20..32]);
    assert_eq!(raw[40..],fixed[40..]);
}

#[test]
fn create_action_depends_on_the_exact_original_disposition(){
    let cases:[(Disposition,&[u32]);6]=[
        (Disposition::Supersede,&[]),(Disposition::Open,&[1]),
        (Disposition::Create,&[2]),(Disposition::OpenIf,&[1,2]),
        (Disposition::Overwrite,&[3]),(Disposition::OverwriteIf,&[2,3]),
    ];
    for (disposition,allowed) in cases{
        for action in 0..=4{
            assert_eq!(create_action_matches(disposition,action),allowed.contains(&action));
        }
    }
    assert_eq!(RootFailure::FileLockConflict as u32,WriteFailure::FileLockConflict as u32);
}
