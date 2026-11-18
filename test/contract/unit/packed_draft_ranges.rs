use super::*;
const GIB: u64 = 1 << 30;

#[test]
fn ten_separately_flushed_ten_gib_edits_map_to_ten_flat_runs_not_transport_frames() {
    // Geometry source only; no 100 GiB file, Samba connection or I/O timing is simulated.
    let mut view = RunTable::default();
    for save in 0..10 {
        let pack = PackId::from_random([save as u8 + 1; 16]);
        let start = save * 10 * GIB;
        for frame in 0..(10 * GIB / 65_536) {
            let local = frame * 65_536;
            view.replace(start + local, 65_536, RunSource::Data { pack, offset: local }).unwrap();
        }
        assert_eq!(view.runs().len(), save as usize + 1);
        assert_eq!(view.references().len(), save as usize + 1);
        assert_eq!(view.at(start, 100 * GIB).unwrap(), (ReadSpan::Data { pack, offset: 0 }, 10 * GIB));
        if start + 10 * GIB < 100 * GIB {
            assert_eq!(view.at(start + 10 * GIB, 100 * GIB).unwrap().0, ReadSpan::Gap);
        }
    }
    assert_eq!(view.runs().last().unwrap().end(), 100 * GIB);
}

#[test]
fn last_write_wins_preserves_both_old_offsets_and_explicit_zero_is_not_a_base_gap() {
    let a = PackId::from_random([1; 16]);
    let b = PackId::from_random([2; 16]);
    let mut table = RunTable::default();
    table.replace(10, 20, RunSource::Data { pack: a, offset: 6 }).unwrap();
    table.replace(14, 8, RunSource::Data { pack: b, offset: 0 }).unwrap();
    table.replace(16, 3, RunSource::Zero {}).unwrap();
    assert_eq!(table.at(9, 40).unwrap(), (ReadSpan::Gap, 1));
    assert_eq!(table.at(11, 40).unwrap(), (ReadSpan::Data { pack: a, offset: 7 }, 3));
    assert_eq!(table.at(14, 40).unwrap(), (ReadSpan::Data { pack: b, offset: 0 }, 2));
    assert_eq!(table.at(16, 40).unwrap(), (ReadSpan::Zero, 3));
    assert_eq!(table.at(19, 40).unwrap(), (ReadSpan::Data { pack: b, offset: 5 }, 3));
    assert_eq!(table.at(22, 40).unwrap(), (ReadSpan::Data { pack: a, offset: 18 }, 8));
    table.truncate(24);
    assert_eq!(table.at(22, 40).unwrap(), (ReadSpan::Data { pack: a, offset: 18 }, 2));
    assert_eq!(table.at(24, 40).unwrap(), (ReadSpan::Gap, 16));
}

#[test]
fn fragmented_ranges_and_distinct_pack_limits_are_separate_and_failure_preserves_original() {
    let mut fragmented = RunTable::default();
    for n in 0..MAX_LIVE_RUNS { fragmented.replace(2 * n as u64, 1, RunSource::Zero {}).unwrap(); }
    let previous = fragmented.clone();
    assert!(fragmented.replace(MAX_LIVE_RUNS as u64 * 2, 1, RunSource::Zero {}).is_err());
    assert_eq!(fragmented, previous);
    assert!(fragmented.replace(u64::MAX, 1, RunSource::Zero {}).is_err());
    fragmented.replace(1, 1, RunSource::Zero {}).unwrap();
    assert_eq!(fragmented.runs().len(), MAX_LIVE_RUNS - 1);
    let mut many_packs = RunTable::default();
    for n in 0..MAX_LIVE_PACKS {
        many_packs.replace(n as u64 * 2, 1, RunSource::Data { pack: PackId::from_random([n as u8; 16]), offset: 0 }).unwrap();
    }
    let before = many_packs.clone();
    assert!(many_packs.replace(1024, 1, RunSource::Data { pack: PackId::from_random([254; 16]), offset: 0 }).is_err());
    assert_eq!(many_packs, before);
}
