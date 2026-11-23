use super::super::Witness;
use super::*;
use crate::pinned::Entity;

fn ticket(index: u8, path: &[u8], name: &[u8]) -> Ticket {
    Ticket {
        seed: [index; 16],
        pool: [2; 16],
        token: [3; 32],
        target: b"/data".to_vec(),
        source_root: b"/source".to_vec(),
        relative: path.to_vec(),
        source: Witness {
            entity: Entity {
                dev: 1,
                ino: u64::from(index),
            },
            uid: 1000,
            mode: 0o100_600,
            links: 1,
            modified: (1, 0),
            changed: (2, 0),
            length: 0,
        },
        sha256: [9; 32],
        name: name.to_vec(),
    }
}

fn pair() -> Vec<Ticket> {
    vec![
        ticket(1, b"dir/a\t\xff", b"Report"),
        ticket(2, b"line\nb", b"report"),
    ]
}

fn encode(tickets: Vec<Ticket>) -> Vec<u8> {
    let mut out = Vec::new();
    // Also construct well-framed invalid plans to exercise the reader's own checks.
    Batch(tickets).write(&mut out).unwrap();
    out
}

fn parse(words: &[&str]) -> Result<ImportBatch, &'static str> {
    ImportBatch::parse(words.iter().copied().map(OsString::from))
}

#[test]
fn all_explicit_pairs_follow_the_complete_role_list() {
    let args = [
        "prepare", "/source", "/two", "/sys", "/recycle", "/one", "/two", "--", "dir/a", "Report",
        "b", "report",
    ];
    assert!(
        matches!(parse(&args), Ok(ImportBatch::Prepare { roles, mapping, .. })
        if roles.len() == 4 && mapping.len() == 2)
    );
    assert!(parse(&args[..args.len() - 1]).is_err());
    assert!(parse(&args[..8]).is_err());
    assert!(parse(&args[..7]).is_err());
    assert!(parse(&["apply", "/sys", "/recycle"]).is_err());
}

#[test]
fn tree_and_saved_raw_maps_do_not_require_all_overrides_in_argv() {
    assert!(matches!(
        parse(&["prepare-tree", "/src", "/data", "/sys", "/rec", "/data"]),
        Ok(ImportBatch::Prepare {
            whole_tree: true,
            from_stdin: false,
            ..
        })
    ));
    assert!(matches!(
        parse(&[
            "prepare-tree",
            "/src",
            "/data",
            "/sys",
            "/rec",
            "/data",
            "--map0"
        ]),
        Ok(ImportBatch::Prepare {
            whole_tree: true,
            from_stdin: true,
            ..
        })
    ));
    assert!(
        parse(&[
            "prepare-tree",
            "/src",
            "/data",
            "/sys",
            "/rec",
            "/data",
            "--map0",
            "extra"
        ])
        .is_err()
    );
    assert!(matches!(parse(&["show"]), Ok(ImportBatch::Show)));
    assert!(parse(&["show", "/extra"]).is_err());
    let valid = b"n/one\t\xff\0First\0next\nname\0Second\0\0";
    let rows = mapping_input::read(&mut valid.as_slice()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0.as_bytes(), b"n/one\t\xff");
    assert_eq!(rows[1].0.as_bytes(), b"next\nname");
    for cut in 0..valid.len() {
        assert!(mapping_input::read(&mut &valid[..cut]).is_err());
    }
    for malformed in [b"\0tail".as_slice(), b"x\0\0\0", b"x\0y\0\0\0"] {
        assert!(mapping_input::read(&mut &malformed[..]).is_err());
    }
}

#[test]
fn show_lists_paths_without_control_injection_and_preserves_printable_unicode() {
    let rows = vec![ticket(1, b"line\n\t\\\xff", "项目".as_bytes())];
    let batch = Batch::checked(rows).unwrap();
    let mut view = Vec::new();
    batch.show(&mut view).unwrap();
    let text = String::from_utf8(view).unwrap();
    assert_eq!(text.lines().count(), 3);
    assert!(text.contains("1\tline\\n\\t\\\\\\xff\t项目\t0\n"));
}

#[test]
fn rows_preserve_raw_sources_case_and_exact_original_ticket_bytes() {
    let original = pair();
    let originals: Vec<_> = original.iter().map(|t| t.encode().unwrap()).collect();
    let raw = encode(original);
    let decoded = Batch::read(&mut raw.as_slice()).unwrap();
    let mut second = Vec::new();
    decoded.write(&mut second).unwrap();
    assert_eq!(second, raw);
    for (t, expected) in decoded.0.iter().zip(originals) {
        assert_eq!(t.encode().unwrap(), expected);
    }
    let names = [String::from("REPORT")];
    assert!(
        check_mappings(
            [(b"a".as_slice(), b"Report".as_slice())].into_iter(),
            &names
        )
        .is_ok()
    );
    assert!(
        check_mappings(
            [(b"a".as_slice(), b"REPORT".as_slice())].into_iter(),
            &names
        )
        .is_err()
    );
}

#[test]
fn complete_frame_is_required_before_any_item_is_visited() {
    let raw = encode(pair());
    for cut in 0..raw.len() {
        assert!(Batch::read(&mut &raw[..cut]).is_err());
    }
    let mut trailing = raw.clone();
    trailing.push(0);
    assert!(Batch::read(&mut trailing.as_slice()).is_err());
    for index in [0, 8, 15, 16, 40, raw.len() - 1] {
        let mut corrupt = raw.clone();
        corrupt[index] ^= 0x80;
        assert!(Batch::read(&mut corrupt.as_slice()).is_err());
    }
    assert!(Batch::read(&mut encode(vec![]).as_slice()).is_err());
}

#[test]
fn valid_checksums_do_not_allow_mixed_scopes_duplicate_paths_names_or_seeds() {
    for field in 0..7 {
        let mut rows = pair();
        let (left, right) = rows.split_at_mut(1);
        let a = &left[0];
        let b = &mut right[0];
        match field {
            0 => b.pool = [8; 16],
            1 => b.token = [8; 32],
            2 => b.target = b"/other".to_vec(),
            3 => b.source_root = b"/other".to_vec(),
            4 => b.relative.clone_from(&a.relative),
            5 => b.name.clone_from(&a.name),
            _ => b.seed = a.seed,
        }
        assert!(Batch::read(&mut encode(rows).as_slice()).is_err());
    }
}

#[test]
fn uncertain_item_stops_and_the_same_batch_revisits_the_original_ids() {
    let mut rows = pair();
    rows.push(ticket(3, b"third", b"Third"));
    let raw = encode(rows);
    let batch = Batch::read(&mut raw.as_slice()).unwrap();
    let mut observed = Vec::new();
    let failed = batch.visit(|n, t| {
        observed.push((n, t.seed));
        if n == 1 {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "lost reply"))
        } else {
            Ok(())
        }
    });
    let error = failed.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert!(
        error
            .to_string()
            .contains("batch item 2 of 3 not confirmed: lost reply")
    );
    assert_eq!(observed, [(0, [1; 16]), (1, [2; 16])]);
    let mut retried = Vec::new();
    Batch::read(&mut raw.as_slice())
        .unwrap()
        .visit(|n, t| {
            retried.push((n, t.seed));
            Ok(())
        })
        .unwrap();
    assert_eq!(retried, [(0, [1; 16]), (1, [2; 16]), (2, [3; 16])]);
}
