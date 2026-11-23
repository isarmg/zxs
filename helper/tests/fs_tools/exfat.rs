use super::*;

const SIZE: u64 = 1 << 20;
fn boot() -> [u8; 512] {
    // This is only the framing inspected by helper, NOT a fabricated healthy filesystem.
    let mut b = [0; 512];
    b[..3].copy_from_slice(&[0xeb, 0x76, 0x90]);
    b[3..11].copy_from_slice(b"EXFAT   ");
    b[72..80].copy_from_slice(&(SIZE / 512).to_le_bytes());
    b[104..106].copy_from_slice(&[0, 1]);
    b[108] = 9;
    b[109] = 3;
    b[110] = 1;
    b[510..].copy_from_slice(&[0x55, 0xaa]);
    b
}
#[test]
fn supported_framing_does_not_walk_fat_or_validate_data_or_backup_boot() {
    let mut b = boot();
    b[64..72].copy_from_slice(&5000u64.to_le_bytes());
    let f = framing(&b, SIZE).unwrap();
    assert_eq!((f.sector, f.length), (512, SIZE));
    b[108] = 12;
    b[72..80].copy_from_slice(&(SIZE / 4096).to_le_bytes());
    assert_eq!(framing(&b, SIZE).unwrap().sector, 4096);
    assert!(framing(&b, SIZE + 4096).is_err());
    assert!(framing(&b, SIZE - 4096).is_err());
    for (i, v) in [
        (0, 0),
        (3, 0),
        (11, 1),
        (104, 1),
        (106, 1),
        (108, 8),
        (109, 25),
        (110, 2),
        (510, 0),
    ] {
        let mut bad = boot();
        bad[i] = v;
        assert!(framing(&bad, SIZE).is_err());
    }
    b[72..80].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(framing(&b, SIZE).is_err());
}
const B: &str = "exfatprogs version : 1.2.9\n";
const DONE: &str = "/image: clean. directories 1, files 0\n";
const BAD: &str =
    "/image: corrupted. directories 2, files 1\n/image: files corrupted 1, files fixed 0\n";
#[test]
fn version_transcript_exit_counters_and_observed_flags_are_independent() {
    let f = framing(&boot(), SIZE).unwrap();
    let check = |code, lines: &str, errors: &[u8]| {
        classify(code, (B.to_owned() + lines).as_bytes(), errors, f)
    };
    assert_eq!(check(0, DONE, b"").unwrap().exit(), 0);
    let d = check(0, DONE, b"failed to read volume label\n").unwrap();
    assert_eq!(
        (d.state, d.counts),
        (Outcome::ExtraDiagnostics, Some((1, 0)))
    );
    let found = check(4, BAD, b"bad filename <untrusted>\x1b[2J").unwrap();
    assert_eq!(
        (found.state, found.counts),
        (Outcome::Uncorrected, Some((2, 1)))
    );
    assert!(
        !check(4, BAD, b"bad filename <untrusted>\x1b[2J")
            .unwrap()
            .to_string()
            .contains("untrusted")
    );
    for msg in [
        "",
        DONE,
        "/image: corrupted. directories 2, files 1\n",
        "/image: corrupted. directories 2, files 1\n/image: files corrupted 1, files fixed 1\n",
    ] {
        assert_eq!(
            check(4, msg, b"failed to verify root directory\n")
                .unwrap()
                .exit(),
            2
        );
    }
    for code in [1, 2, 8, 16, 32, 128] {
        assert!(check(code, DONE, b"").is_err());
    }
    assert!(check(0, "", b"").is_err());
    assert!(check(0, BAD, b"").is_err());
    assert!(check(0, &DONE.replace("1, files", "0, files"), b"").is_err());
    assert!(check(0, &DONE.replace("files 0", "files 4294967295"), b"").is_err());
    assert!(classify(0, DONE.as_bytes(), b"", f).is_err());
    let mut b = boot();
    b[106] = 6;
    let v = classify(
        0,
        (B.to_owned() + DONE).as_bytes(),
        b"",
        framing(&b, SIZE).unwrap(),
    )
    .unwrap();
    assert_eq!(v.exit(), 1);
    assert!(
        v.to_string()
            .contains("volume_dirty=true media_failure_flag=true")
    );
}
#[test]
fn exfat_web_requires_its_own_explicit_terminal_preselection_with_no_path_from_http_or_repair() {
    use crate::{
        Command,
        volume_images::{CheckFormat, Format},
    };
    let h = "ab".repeat(32);
    let parse = |a: &[&str]| crate::parse(a.iter().map(|s| std::ffi::OsString::from(*s)));
    assert!(
        matches!(parse(&["check-exfat-copy","/saved/volume.img","--sha256",&h]),
        Ok(Command::CopyCheck(r)) if r.format() == CheckFormat::Sealed(Format::Exfat))
    );
    assert!(matches!(parse(&["disk-web"]), Ok(Command::DiskWeb(None))));
    assert!(
        matches!(parse(&["disk-web","--exfat-copy","/saved/volume.img","--sha256",&h]),
        Ok(Command::DiskWeb(Some(r))) if r.format() == CheckFormat::Sealed(Format::Exfat))
    );
    assert!(
        matches!(parse(&["disk-web","--fat32-copy","/saved/volume.img","--sha256",&h]),
        Ok(Command::DiskWeb(Some(r))) if r.format() == CheckFormat::Sealed(Format::Fat32))
    );
    for bad in [
        &["check-exfat-copy", "/saved/volume.img", "-p"][..],
        &[
            "check-exfat-copy",
            "/saved/volume.img",
            "--sha256",
            &h,
            "--repair",
        ],
        &["check-exfat-copy", "relative.img", "--sha256", &h],
        &["check-exfat-copy", "/saved/volume.img", "--sha256", "bad"],
        &[
            "disk-web",
            "--exfat-copy",
            "/saved/volume.img",
            "--sha256",
            &h,
            "--repair",
        ],
        &[
            "disk-web",
            "--exfat-copy",
            "/saved/volume.img",
            "--sha256",
            &h,
            "--fat32-copy",
            "/other",
        ],
        &["disk-web", "--exfat-copy", "relative.img", "--sha256", &h],
    ] {
        assert!(parse(bad).is_err());
    }
}

#[test]
fn only_the_matching_held_exfat_operand_worker_exit_and_whole_typed_receipt_can_report() {
    let mut b = boot();
    let h = [0x11u8; 32];
    for (code, msg, err, flags, expected) in [
        (0, DONE, "", 0, 0),
        (0, DONE, "", 6, 1),
        (0, DONE, "ignored untrusted diagnostic", 0, 2),
        (4, BAD, "", 0, 1),
        (4, DONE, "failed early", 0, 2),
    ] {
        b[106] = flags;
        let frame = framing(&b, SIZE).unwrap();
        let r = classify(code, (B.to_owned() + msg).as_bytes(), err.as_bytes(), frame).unwrap();
        let p = r.packet(&h);
        let status = Some(expected);
        let read = Report::from_packet(&p, status, frame, &h).unwrap();
        assert_eq!(read.state, r.state);
        assert_eq!(read.exit(), expected as u8);
        assert_eq!(read.counts, r.counts);
        assert!(!read.panel_text().contains("untrusted"));
        assert!(Report::from_packet(&[], Some(2), frame, &h).is_err()); // naked 2 != typed "incomplete"
        assert!(Report::from_packet(&p, None, frame, &h).is_err());
        assert!(Report::from_packet(&p, Some((expected + 1) % 3), frame, &h).is_err());
        assert!(Report::from_packet(&p, status, frame, &[0x22; 32]).is_err());
        assert!(Report::from_packet(&p[..56], status, frame, &h).is_err());
        assert!(Report::from_packet(&[p.as_slice(), &[0]].concat(), status, frame, &h).is_err());
        for offset in [0, 5, 37, 45, 47] {
            let mut wrong = p;
            wrong[offset] ^= 1;
            assert!(Report::from_packet(&wrong, status, frame, &h).is_err());
        }
        let mut wrong = p;
        wrong[4] = 99;
        assert!(Report::from_packet(&wrong, status, frame, &h).is_err());
        let mut wrong = p;
        wrong[49..53].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(Report::from_packet(&wrong, status, frame, &h).is_err());
        let mut other = p;
        other[..4].copy_from_slice(b"FCR1");
        assert!(Report::from_packet(&other, status, frame, &h).is_err());
    }
    b[106] = 0;
    let frame = framing(&b, SIZE).unwrap();
    let clean = classify(0, (B.to_owned() + DONE).as_bytes(), b"", frame).unwrap();
    let mut flagged = clean.packet(&h);
    flagged[4] = 1;
    assert!(Report::from_packet(&flagged, Some(1), frame, &h).is_err());
    assert!(
        Report::from_packet(b"FCR1\0\0\0\0\x01\0\0\0\0\0\x01\0\0", Some(0), frame, &h).is_err()
    );
}
