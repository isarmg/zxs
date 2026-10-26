use super::*;
use std::time::{SystemTime, UNIX_EPOCH};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zxs-link-{}-{id}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn excludes_only_generated_output_in_declared_evidence_directories() {
    for path in [
        "test/evidence/run/result.json",
        "tests/evidence/run/result.json",
        "test/evidence/lazy-branches/shot.png",
        "test/evidence/recovery/run/build.log",
    ] {
        assert!(generated(Path::new(path)), "{path}");
    }
    for path in [
        "test/fixtures/result.json",
        "docs/contracts/vectors.json",
        "docs/images/web-file-manager.png",
        "test/evidence/run/README.md",
        "test/evidence/run/probe.mjs",
        "tests/evidence/run/probe.mjs",
        "test/evidence/run/seed.rs",
        "test/evidence/run/helper.py",
        "test/evidence/run/check.sh",
        "docs/tools/contract-check.md",
        "compliance/validation/foundation/proof.json",
    ] {
        assert!(!generated(Path::new(path)), "{path}");
    }
}
#[test]
fn missing_outputs_are_optional_but_missing_documents_and_scripts_fail() {
    let f = Fixture::new();
    let doc = f.0.join("README.md");
    let target = resolve(&f.0, &doc, "test/evidence/run/result.json").unwrap();
    assert!(target.path.is_none() && target.generated);
    for path in [
        "docs/missing.md",
        "test/fixtures/missing.json",
        "test/evidence/run/README.md",
        "test/evidence/run/probe.mjs",
    ] {
        assert!(resolve(&f.0, &doc, path).is_err(), "{path}");
    }
    fs::create_dir_all(f.0.join("test/evidence/run")).unwrap();
    fs::write(f.0.join("test/evidence/run/result.json"), b"{}").unwrap();
    let target = resolve(&f.0, &doc, "test/evidence/run/result.json").unwrap();
    assert!(target.path.is_some() && target.generated);
}
#[test]
fn normalize_before_applying_generated_output_exception() {
    let f = Fixture::new();
    let doc = f.0.join("README.md");
    assert!(resolve(&f.0, &doc, "test/evidence/../../docs/missing.json").is_err());
    assert!(resolve(&f.0, &doc, "tests/evidence/../../docs/missing.json").is_err());
    assert!(resolve(&f.0, &doc, "tests/evidence/run/probe.mjs").is_err());
    assert!(resolve(&f.0, &doc, "../outside.json").is_err());
}
#[cfg(unix)]
#[test]
fn symlinks_cannot_turn_an_external_path_into_optional_local_output() {
    let f = Fixture::new();
    let outside = Fixture::new();
    fs::create_dir(f.0.join("test")).unwrap();
    std::os::unix::fs::symlink(&outside.0, f.0.join("test/evidence")).unwrap();
    assert!(resolve(&f.0, &f.0.join("README.md"), "test/evidence/missing.json").is_err());
}

#[test]
fn missing_document_retains_io_error_source() {
    let fixture = Fixture::new();
    let error = resolve(&fixture.0, &fixture.0.join("README.md"), "missing.md")
        .err()
        .unwrap();
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert!(error.to_string().contains("missing.md"));
}
