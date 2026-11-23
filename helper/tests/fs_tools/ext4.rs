use super::*;

fn base() -> [u8; SUPER_LEN] {
    let mut sb = [0; SUPER_LEN];
    set(&mut sb, 0, 16384); // inode count
    set(&mut sb, 4, 1 << 17); // 512 MiB with 4 KiB blocks
    set(&mut sb, 0x18, 2);
    sb[0x38..0x3a].copy_from_slice(&[0x53, 0xef]);
    set(&mut sb, 0x5c, 0x1004); // internal journal + supported orphan-file feature
    set(&mut sb, 0x60, 0x40); // extents
    set(&mut sb, 0xe0, 8); // internal journal inode
    sb
}
fn set(sb: &mut [u8; SUPER_LEN], off: usize, value: u32) {
    sb[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn accepts_an_ext_extent_larger_than_a_fat_memfd_and_never_a_truncated_filesystem() {
    let sb = base();
    assert_eq!(frame(&sb, 512 << 20).unwrap().blocks, 131_072);
    let longer = frame(&sb, (512 << 20) + 2048).unwrap();
    assert_eq!(longer.filesystem_bytes, 512 << 20);
    assert_eq!(longer.bytes, (512 << 20) + 2048); // trailing bytes aren't inspected as ext
    assert!(frame(&sb, (512 << 20) - 1).is_err());
    let mut not_ext = sb;
    not_ext[0x38] = 0;
    assert!(frame(&not_ext, 512 << 20).is_err());
    let mut large = sb;
    set(&mut large, 0x60, 0xc0);
    set(&mut large, 0x150, 1);
    set(&mut large, 4, 0);
    assert_eq!(frame(&large, 1 << 44).unwrap().blocks, 1 << 32);
    assert!(frame(&large, (1 << 44) - 1).is_err());
    set(&mut large, 0x150, u32::MAX);
    set(&mut large, 0x18, 6);
    assert!(frame(&large, u64::MAX).is_err());
}
#[test]
fn no_external_journal_and_no_known_skipped_recovery_at_admission() {
    for (at, value) in [
        (0x60, 0x44),
        (0x60, 0x48),
        (0xd0, 1),
        (0xe4, 1),
        (0xe0, 0),
        (0x5c, 0x1000),
        (0xe8, 7),
        (0x64, 0x10000),
    ] {
        let mut sb = base();
        set(&mut sb, at, value);
        assert!(frame(&sb, 512 << 20).is_err(), "{at:#x} {value:#x}");
    }
}
fn transcript() -> Vec<u8> {
    let mut out = Vec::new();
    for line in PASSES {
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out.extend_from_slice(b"/image: 11/16384 files (0.0% non-contiguous), 4568/131072 blocks\n");
    out
}
#[test]
fn a_zero_exit_requires_all_real_phases_eof_original_totals_and_no_warning() {
    let extent = frame(&base(), 512 << 20).unwrap();
    let good = transcript();
    let verdict = |out: &[u8], err: &[u8]| classify(0, out, err, extent).result;
    assert_eq!(verdict(&good, BANNER), ResultKind::NoFindings);
    assert_eq!(verdict(&good, b""), ResultKind::Incomplete);
    assert_eq!(
        verdict(&good, &[BANNER, b"cannot open mount table\n"].concat()),
        ResultKind::Incomplete
    );
    assert_eq!(
        verdict(&good[..good.len() - 1], BANNER),
        ResultKind::Incomplete
    );
    assert_eq!(
        verdict(&good[PASSES[0].len() + 1..], BANNER),
        ResultKind::Incomplete
    );
    let warning =
        &b"Warning: skipping journal recovery because doing a read-only filesystem check.\n"[..];
    assert_eq!(
        verdict(&[warning, &good].concat(), BANNER),
        ResultKind::Incomplete
    );
    let unexpected = String::from_utf8(good)
        .unwrap()
        .replace("131072 blocks", "131071 blocks");
    assert_eq!(
        verdict(unexpected.as_bytes(), BANNER),
        ResultKind::Incomplete
    );
}
#[test]
fn only_the_unqualified_bit4_is_uncorrected_and_never_claimed_complete() {
    let extent = frame(&base(), 512 << 20).unwrap();
    assert_eq!(
        classify(4, b"", BANNER, extent).result,
        ResultKind::Uncorrected
    );
    assert_eq!(
        classify(4, b"", b"bwrap: early setup failed\n", extent).result,
        ResultKind::Incomplete
    );
    for status in [1, 2, 5, 6, 8, 12, 16, 20, 32, 36, 64, 128, 132, 255] {
        assert_eq!(
            classify(status, &transcript(), BANNER, extent).exit(),
            2,
            "{status}"
        );
    }
}

#[test]
fn private_ext_receipts_bind_original_digest_large_length_real_exit_and_exact_packet() {
    let bytes = (1u64 << 44) + 4096;
    let extent = frame(&base(), bytes).unwrap();
    let sha = [11; 32];
    let good = classify(0, &transcript(), BANNER, extent).packet(&sha);
    let report = Report::from_packet(&good, Some(0), bytes, &sha).unwrap();
    assert_eq!(
        (report.result, report.bytes, report.filesystem_bytes),
        (ResultKind::NoFindings, bytes, 512 << 20)
    );
    assert!(Report::from_packet(&good, Some(0), bytes - 1, &sha).is_err());
    assert!(Report::from_packet(&good, Some(0), bytes, &[12; 32]).is_err());
    assert!(Report::from_packet(&good[..56], Some(0), bytes, &sha).is_err());
    assert!(Report::from_packet(&[good.as_slice(), b"!"].concat(), Some(0), bytes, &sha).is_err());
    assert!(Report::from_packet(&good, None, bytes, &sha).is_err());
    let mut bad = good;
    bad[..4].copy_from_slice(b"ECR1");
    assert!(Report::from_packet(&bad, Some(0), bytes, &sha).is_err());
    let mut bad = good;
    bad[17..25].copy_from_slice(&(bytes + 1).to_be_bytes());
    assert!(Report::from_packet(&bad, Some(0), bytes, &sha).is_err());
    let mut bad = good;
    bad[5..9].copy_from_slice(&4i32.to_be_bytes());
    assert!(Report::from_packet(&bad, Some(0), bytes, &sha).is_err());
    let unresolved = classify(4, b"", BANNER, extent).packet(&sha);
    assert_eq!(
        Report::from_packet(&unresolved, Some(1), bytes, &sha)
            .unwrap()
            .result,
        ResultKind::Uncorrected
    );
    assert!(Report::from_packet(&unresolved, Some(0), bytes, &sha).is_err());
    let incomplete = classify(0, b"", BANNER, extent).packet(&sha);
    assert_eq!(
        Report::from_packet(&incomplete, Some(2), bytes, &sha)
            .unwrap()
            .result,
        ResultKind::Incomplete
    );
    assert!(Report::from_packet(&incomplete, Some(3), bytes, &sha).is_err()); // late worker failure
}

#[test]
fn waiting_ext_selection_neither_needs_a_valid_header_nor_excludes_a_writer_but_will_not_handoff_a_replacement()
 {
    use crate::{pinned::Mount, volume_images::leased::Selected};
    use rustix::fs::{Mode, OFlags, fcntl_getfl, open};
    let fixture = crate::test_area::data_directory();
    let path = fixture.path().join("saved-copy");
    let writer = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    writer.set_len(4096).unwrap(); // no extent or ext header: preselection must not read it
    let selected = Selected::select(&path).unwrap();
    assert_eq!(selected.len(), 4096);
    drop(writer);
    let writer = open(
        &path,
        OFlags::WRONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .unwrap();
    drop(writer); // a waiting selection must not hold a read lease
    let reader = selected.reader().unwrap();
    assert!(fcntl_getfl(&reader).unwrap().contains(OFlags::NOATIME));
    assert!(Mount::on(&reader).unwrap().is_some());
    std::fs::rename(&path, fixture.path().join("still-held-original")).unwrap();
    let replacement = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    replacement.set_len(4096).unwrap();
    assert!(selected.reader().is_err());
    assert!(selected.require().is_err()); // final parent check also refuses
}

#[test]
fn a_readable_original_changed_after_preview_does_not_reach_the_worker() {
    use crate::volume_images::leased::Selected;
    let fixture = crate::test_area::data_directory();
    let path = fixture.path().join("edited");
    let writer = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    writer.set_len(4096).unwrap();
    let original = Selected::select(&path).unwrap();
    writer.set_len(8192).unwrap();
    assert!(original.require().is_err());
    assert!(original.reader().is_err());
}

#[test]
fn preen_needs_an_actual_final_summary_but_does_not_misread_corrections_as_readonly_passes() {
    let extent = frame(&base(), 512 << 20).unwrap();
    let valid = b"/image: 11/16384 files (0.0% non-contiguous), 4568/131072 blocks\n";
    assert!(preen_summary(valid, extent));
    assert!(preen_summary(
        &[b"some earlier correction\n".as_slice(), valid].concat(),
        extent
    ));
    assert!(!preen_summary(&valid[..valid.len() - 1], extent));
    assert!(!preen_summary(
        &[valid.as_slice(), b"not the native final stats\n"].concat(),
        extent
    ));
    assert!(!preen_summary(
        b"/image: 11/16384 files (0.0% non-contiguous), 4568/131073 blocks\n",
        extent
    ));
}
