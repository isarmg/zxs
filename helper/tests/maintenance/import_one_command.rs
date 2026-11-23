use super::*;

fn parse(words: &[&str]) -> Result<ImportOne, &'static str> {
    ImportOne::parse(words.iter().copied().map(OsString::from))
}

#[test]
fn prepare_requires_explicit_name_placement_and_roles() {
    let full = [
        "prepare",
        "/source",
        "inner/file",
        "name",
        "/a",
        "/system",
        "/recycle",
        "/a",
    ];
    assert!(matches!(parse(&full), Ok(ImportOne::Prepare { .. })));
    assert!(parse(&full[..7]).is_err());
    let mut relative_source = full;
    relative_source[1] = "source";
    assert!(parse(&relative_source).is_err());
    assert!(parse(&["apply", "/system", "/recycle"]).is_err());
    assert!(matches!(
        parse(&["apply", "/system", "/recycle", "/a"]),
        Ok(ImportOne::Apply(_))
    ));
}

#[test]
fn malformed_ticket_line_fails_before_disk_selection() {
    for malformed in [
        b"\n".as_slice(),
        b"00",
        b"0\n",
        b"AB\n",
        b"00\n00\n",
        b"zz\n",
    ] {
        assert!(read_ticket(malformed).is_err());
    }
    assert_eq!(hex_line(&[0, 0x1f, 0xab]).as_slice(), b"001fab");
}
