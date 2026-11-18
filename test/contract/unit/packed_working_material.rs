use super::*;

#[test]
fn private_chunk_index_is_frame_independent_and_bound_to_one_pack_length_and_id() {
    let id = PackId::from_random([7; 16]);
    let entity = ContainerEntity::new(81, 2);
    let bytes = vec![42; VERIFY_CHUNK as usize + 19];
    let mut streamed = AppendIndex::default();
    for frame in bytes.chunks(60_011) { streamed.append(frame).unwrap(); }
    let prefix = streamed.expected_chunk(0).unwrap();
    assert_eq!(prefix, *blake3::hash(&bytes[..VERIFY_CHUNK as usize]).as_bytes());
    let (pack, index) = streamed.finish(id, entity).unwrap();
    let mut once = AppendIndex::default();
    once.append(&bytes).unwrap();
    assert_eq!(once.finish(id, entity).unwrap(), (pack, index.clone()));
    assert_eq!(pack.index_bytes(), 64);
    let indexed = CheckedIndex::from_trailer(pack, index.clone()).unwrap();
    assert_eq!(indexed.expected_chunk(0), Some(prefix));
    assert_eq!(indexed.expected_chunk(u64::MAX), None);
    let wire = json_codec::to_vec(&pack).unwrap();
    let reopened: WorkingPack = json_codec::from_slice(&wire).unwrap();
    assert_eq!(reopened, pack);
    for changed in [
        WorkingPack { id: PackId::from_random([8; 16]), ..pack },
        WorkingPack { data_bytes: pack.data_bytes() + 1, ..pack },
    ] { assert!(CheckedIndex::from_trailer(changed, index.clone()).is_err()); }
    assert!(CheckedIndex::from_trailer(pack, vec![0; index.len()]).is_err());
    assert!(CheckedIndex::from_trailer(pack, vec![0; index.len() + 1]).is_err());
}

#[test]
fn ten_gib_is_one_physical_segment_while_rollover_is_not_a_logical_file_ceiling() {
    const GIB: u64 = 1 << 30;
    assert_eq!(physical_bytes_for(10 * GIB).unwrap(), 10 * GIB + 10 * 1024 * 32);
    assert_eq!(physical_bytes_for(PACK_BYTES).unwrap(), PACK_BYTES + 524_288);
    for bytes in [PACK_BYTES + 1, u64::MAX] { assert!(physical_bytes_for(bytes).is_err()); }
    assert_eq!(physical_bytes_for(0).unwrap(), 0);
    let id = PackId::from_random([1; 16]);
    assert!(AppendIndex::default().finish(id, ContainerEntity::new(8, 2)).is_err());
    let mut one = AppendIndex::default();
    one.append(b"x").unwrap();
    assert!(one.finish(id, ContainerEntity::new(8, 0)).is_err());
}
