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
