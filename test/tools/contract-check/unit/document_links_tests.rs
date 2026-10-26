use super::*;
fn fixture() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

#[test]
fn fixture_directories_are_unique_while_created_concurrently() {
    let workers: Vec<_> = (0..32).map(|_| std::thread::spawn(fixture)).collect();
    let fixtures: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    let paths: std::collections::BTreeSet<_> =
        fixtures.iter().map(|fixture| fixture.path()).collect();
    assert_eq!(paths.len(), fixtures.len());
    for fixture in fixtures {
        fixture.close().unwrap();
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
    let f = fixture();
    let doc = f.path().join("README.md");
    let target = resolve(f.path(), &doc, "test/evidence/run/result.json").unwrap();
    assert!(target.path.is_none() && target.generated);
    for path in [
        "docs/missing.md",
        "test/fixtures/missing.json",
        "test/evidence/run/README.md",
        "test/evidence/run/probe.mjs",
    ] {
        assert!(resolve(f.path(), &doc, path).is_err(), "{path}");
    }
    fs::create_dir_all(f.path().join("test/evidence/run")).unwrap();
    fs::write(f.path().join("test/evidence/run/result.json"), b"{}").unwrap();
    let target = resolve(f.path(), &doc, "test/evidence/run/result.json").unwrap();
    assert!(target.path.is_some() && target.generated);
}
#[test]
fn normalize_before_applying_generated_output_exception() {
    let f = fixture();
    let doc = f.path().join("README.md");
    assert!(resolve(f.path(), &doc, "test/evidence/../../docs/missing.json").is_err());
    assert!(resolve(f.path(), &doc, "tests/evidence/../../docs/missing.json").is_err());
    assert!(resolve(f.path(), &doc, "tests/evidence/run/probe.mjs").is_err());
    assert!(resolve(f.path(), &doc, "../outside.json").is_err());
}
#[cfg(unix)]
#[test]
fn symlinks_cannot_turn_an_external_path_into_optional_local_output() {
    let f = fixture();
    let outside = fixture();
    fs::create_dir(f.path().join("test")).unwrap();
    std::os::unix::fs::symlink(outside.path(), f.path().join("test/evidence")).unwrap();
    assert!(
        resolve(
            f.path(),
            &f.path().join("README.md"),
            "test/evidence/missing.json"
        )
        .is_err()
    );
}

#[test]
fn missing_document_retains_io_error_source() {
    let fixture = fixture();
    let error = resolve(
        fixture.path(),
        &fixture.path().join("README.md"),
        "missing.md",
    )
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
