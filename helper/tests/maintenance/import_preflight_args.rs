use super::*;

#[test]
fn only_explicit_source_and_ordered_current_roles_are_accepted() {
    let good = [
        "/old",
        "/zxs/disks/system",
        "/zxs/disks/recycle",
        "/zxs/disks/data-a",
        "/zxs/disks/data-b",
    ];
    let q = Preflight::parse(good.map(OsString::from).into_iter()).unwrap();
    assert_eq!(q.source, Path::new("/old"));
    assert!(!q.show_names && !q.check_target);
    let show_names = ["--show-names"].into_iter().chain(good).map(OsString::from);
    let names_only = Preflight::parse(show_names).unwrap();
    assert!(names_only.show_names && !names_only.check_target);
    let check_target = ["--check-target"]
        .into_iter()
        .chain(good)
        .map(OsString::from);
    let comparison = Preflight::parse(check_target).unwrap();
    assert!(comparison.show_names && comparison.check_target);
    let both = ["--show-names", "--check-target"]
        .into_iter()
        .chain(good)
        .map(OsString::from);
    assert!(Preflight::parse(both).is_err());
    assert_eq!(
        q.roles,
        good[1..]
            .iter()
            .copied()
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    );
    assert!(Preflight::parse(good[..3].iter().copied().map(OsString::from)).is_err());
    assert!(Preflight::parse(good[..4].iter().copied().map(OsString::from)).is_ok());
    for wrong in ["/", "../source", "/x/../source", "/x//source", "/x/"] {
        let mut args = good;
        args[0] = wrong;
        assert!(Preflight::parse(args.map(OsString::from).into_iter()).is_err());
    }
    let obsolete_proxy_ids = good.into_iter().chain(["2010", "2020"]).map(OsString::from);
    assert!(Preflight::parse(obsolete_proxy_ids).is_err());

    let appended = good.into_iter().chain(["--execute"]).map(OsString::from);
    assert!(Preflight::parse(appended).is_err());
}

#[test]
fn data_roster_length_is_bounded() {
    let args = |count| {
        ["/old", "/system", "/recycle"]
            .into_iter()
            .map(OsString::from)
            .chain((0..count).map(|i| OsString::from(format!("/data-{i}"))))
    };
    assert!(Preflight::parse(args(62)).is_ok());
    assert!(Preflight::parse(args(63)).is_err());
}

#[test]
fn same_name_candidates_follow_server_spelling_not_workspace_alias_rules() {
    assert!(same_name_candidate(&[b'a'; 255]));
    assert!(!same_name_candidate(&[b'a'; 256]));
    for allowed in ["方案.xlsx", "NUL.txt", "e\u{301}.txt", "REPORT", "report"] {
        assert!(same_name_candidate(allowed.as_bytes()));
    }
    for denied in [
        b"".as_slice(),
        b".",
        b"..",
        b"name.",
        b"name ",
        b"a/b",
        b"a\\b",
        b"a\x7fb",
        b"\xff",
    ] {
        assert!(!same_name_candidate(denied));
    }
}

#[test]
fn preview_keeps_source_bytes_and_does_not_flatten_nested_names() {
    let mut rows = Vec::new();
    write_preview_row(
        &mut rows,
        std::iter::empty(),
        c"File",
        4,
        PreviewName::SameName,
    )
    .unwrap();
    write_preview_row(
        &mut rows,
        [c"first"].into_iter(),
        c"File",
        10,
        PreviewName::MappingRequired,
    )
    .unwrap();
    write_preview_row(
        &mut rows,
        [c"other"].into_iter(),
        c"File",
        10,
        PreviewName::MappingRequired,
    )
    .unwrap();
    write_preview_row(
        &mut rows,
        std::iter::empty(),
        c"odd\n\xff",
        5,
        PreviewName::MappingRequired,
    )
    .unwrap();
    assert_eq!(
        rows,
        b"File\tsame-name\tFile\n\
          first/File\tmapping-required\t\n\
          other/File\tmapping-required\t\n\
          odd\\n\\xff\tmapping-required\t\n"
    );
}

#[test]
fn target_check_compares_exact_candidate_bytes_and_never_renames() {
    let occupied = [String::from("Report"), String::from("draft")];
    assert_eq!(
        name_status(b"Report", true, Some(&occupied)),
        PreviewName::Occupied
    );
    assert_eq!(
        name_status(b"report", true, Some(&occupied)),
        PreviewName::NotFound
    );
    assert_eq!(
        name_status(b"Report", false, Some(&occupied)),
        PreviewName::MappingRequired
    );
    assert_eq!(name_status(b"Report", true, None), PreviewName::SameName);
    let mut out = Vec::new();
    for name in [c"Report", c"report"] {
        write_preview_row(
            &mut out,
            std::iter::empty(),
            name,
            6,
            name_status(name.to_bytes(), true, Some(&occupied)),
        )
        .unwrap();
    }
    assert_eq!(
        out,
        b"Report\ttarget-name-exists\tReport\nreport\tno-exact-target-name\treport\n"
    );
}
