//! Prove that ignored evidence is optional in an exported source tree, without
//! weakening mandatory source links or changing the real repository's index.
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Component, Path},
    process::{Command, Stdio},
};

fn git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.args(["-c", "core.excludesFile=", "-C"]).arg(root);
    command
}
#[test]
fn source_export_checks_without_generated_evidence_and_still_rejects_broken_source_links() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let files = git(&root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()
        .expect("Git is required for source export verification");
    assert!(
        files.status.success(),
        "{}",
        String::from_utf8_lossy(&files.stderr)
    );
    let mut ignored = git(&root)
        .args(["check-ignore", "--no-index", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    ignored
        .stdin
        .take()
        .unwrap()
        .write_all(&files.stdout)
        .unwrap();
    let ignored = ignored.wait_with_output().unwrap();
    assert!(
        matches!(ignored.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&ignored.stderr)
    );
    let excluded: BTreeSet<_> = ignored
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .collect();
    for raw in files
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty() && !excluded.contains(p))
    {
        let name = std::str::from_utf8(raw).unwrap();
        let relative = Path::new(name);
        assert!(
            relative
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
            "{name}"
        );
        let source = root.join(relative);
        // A source export follows the current working tree while a refactor
        // may still have removed files recorded in the Git index.
        let metadata = match source.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("cannot inspect export source {name}: {error}"),
        };
        assert!(
            metadata.file_type().is_file(),
            "export requires regular source files: {name}"
        );
        let output = fixture.path().join(relative);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::copy(source, output).unwrap();
    }
    assert!(fixture.path().join("docs/contracts/vectors.json").is_file());
    assert!(
        fixture
            .path()
            .join(
                "test/tools/contract-check/fixtures/merge-tags-backup/file-merge-tag-history.json"
            )
            .is_file()
    );
    assert!(
        fixture
            .path()
            .join("docs/tools/contract-check.md")
            .is_file()
    );
    assert!(
        fixture
            .path()
            .join("test/evidence/lazy-branches/web-probe.mjs")
            .is_file()
    );
    assert!(
        !fixture
            .path()
            .join("test/evidence/merge-tags-backup/candidate-receipt.json")
            .exists()
    );
    assert!(
        !fixture
            .path()
            .join("test/evidence/lazy-branches/browser-result.json")
            .exists()
    );
    // Exercise optional local output with a self-contained fixture, rather
    // than requiring historical reports in maintained documentation.
    fs::write(
        fixture.path().join("docs/generated-output-regression.md"),
        "[Local output](../test/evidence/source-export/output.json)\n",
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_contract-check"))
        .arg("--root")
        .arg(fixture.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: json_codec::Value = json_codec::from_slice(&result.stdout).unwrap();
    assert_eq!(report["result"], "PASS");
    assert!(
        report["missing_local_generated_artifacts"]
            .as_u64()
            .unwrap()
            > 0
    );
    fs::write(
        fixture.path().join("docs/source-link-regression.md"),
        "[Required source](missing-required-source.rs)\n",
    )
    .unwrap();
    let failure = Command::new(env!("CARGO_BIN_EXE_contract-check"))
        .arg("--root")
        .arg(fixture.path())
        .output()
        .unwrap();
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("missing-required-source.rs"));
}
