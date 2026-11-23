use super::*;
use std::ffi::OsString;

fn files<'a>(paths: &'a [&'a [u8]]) -> impl Iterator<Item = (&'a OsStr, ())> + 'a {
    paths.iter().map(|&bytes| (OsStr::from_bytes(bytes), ()))
}

fn mapping(path: &[u8], name: &str) -> Mapping {
    (OsStr::from_bytes(path).to_owned(), OsString::from(name))
}

#[test]
fn top_level_names_are_exact_and_explicit_overrides_consume_each_nested_path() {
    let choices = [mapping(b"one/Report", "甲"), mapping(b"two/Report", "乙"), mapping(b"invalid\xff", "原件")];
    let paths: [&[u8]; 5] = [b"Report", b"invalid\xff", b"one/Report", b"report", b"two/Report"];
    let rows = resolve(files(&paths), &choices).unwrap();
    let names: Vec<_> = rows.iter().map(|(_, name, _)| *name).collect();
    assert_eq!(names, ["Report", "原件", "甲", "report", "乙"].map(OsStr::new));
}

#[test]
fn whole_source_rejects_missing_overrides_unknown_sources_and_unusable_names() {
    assert!(resolve(files(&[b"sub/file"]), &[]).is_err());
    assert!(resolve(files(&[b"bad\xff"]), &[]).is_err());
    assert!(resolve(files(&[]), &[]).is_err());
    assert!(resolve(files(&[b"left"]), &[mapping(b"Left", "a")]).is_err());
    assert!(resolve(files(&[b"left"]), &[mapping(b"left", "a/")]).is_err());
    assert!(resolve(files(&[b"left"]), &[mapping(b"left", "A"), mapping(b"left", "B")]).is_err());
}

#[test]
fn default_and_mapped_names_must_also_agree_with_the_final_catalog_check() {
    let paths: [&[u8]; 2] = [b"Report", b"sub/file"];
    let choices = [mapping(b"sub/file", "Report")];
    let rows = resolve(files(&paths), &choices).unwrap();
    let result = check_mappings(rows.iter().map(|(p, n, _)| (p.as_bytes(), n.as_bytes())), &[]);
    assert!(result.is_err());
    let choices = [mapping(b"sub/file", "report")];
    let rows = resolve(files(&paths), &choices).unwrap();
    let entries = || rows.iter().map(|(p, n, _)| (p.as_bytes(), n.as_bytes()));
    assert!(check_mappings(entries(), &[]).is_ok());
    assert!(check_mappings(entries(), &[String::from("report")]).is_err());
}
