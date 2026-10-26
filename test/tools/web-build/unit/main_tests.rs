#[test]
fn embedded_command_reference_is_a_declared_build_input() {
    let specification: json_codec::Value = json_codec::from_str(include_str!(
        "../../../../tools/web-build/build-inputs.json"
    ))
    .unwrap();
    assert!(
        specification["directories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path.as_str() == Some("docs/architecture/command-line-reference"))
    );
}

#[test]
fn sha256_hex_preserves_canonical_checksum_bytes() {
    assert_eq!(
        super::sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        super::sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        super::sha256_hex([0; 64]),
        "f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b"
    );
}

use super::*;

#[test]
fn build_inventory_uses_descriptive_fields_without_format_numbering() {
    let files = BTreeMap::from([("boot.js".to_owned(), "output-hash".to_owned())]);
    let inputs = BTreeMap::from([("src/web/src/lib.rs".to_owned(), "input-hash".to_owned())]);
    let manifest = build_manifest(&files, &inputs);
    assert_eq!(manifest.as_object().unwrap().len(), 5);
    assert_eq!(manifest["runtime"], "web-interface");
    assert_eq!(manifest["files"]["boot.js"], "output-hash");
    assert_eq!(manifest["inputs"]["src/web/src/lib.rs"], "input-hash");
    assert_eq!(manifest["target"], "wasm32-unknown-unknown");
    assert_eq!(manifest["wasm_bindgen"], "0.2.129");
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "zxs-build-inputs-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn distribution_retains_central_notices_fonts_and_source_receipts() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    let output = fixture.0.join("output");
    fs::create_dir_all(source.join("compliance/licenses/fonts")).unwrap();
    fs::create_dir_all(&output).unwrap();
    for relative in [
        "licenses/fonts/OFL.txt",
        "README.md",
        "NOTICE.md",
        "rust-sources.json",
        "provenance/origins.json",
        "provenance/document-migration.json",
        "provenance/shared/vendor/rust-notices/provenance.json",
    ] {
        let input = source.join("compliance").join(relative);
        fs::create_dir_all(input.parent().unwrap()).unwrap();
        fs::write(input, format!("canonical {relative} bytes")).unwrap();
    }
    copy_distribution_material(&source, &output).unwrap();
    assert_eq!(
        files_for_test(&source.join("compliance")),
        files_for_test(&output.join("compliance"))
    );
}

fn files_for_test(root: &Path) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    inventory_tree(root, root, &mut result).unwrap();
    result
}

#[test]
fn inventory_tracks_cargo_configuration_and_toolchain() {
    let fixture = Fixture::new();
    let spec: json_codec::Value = json_codec::from_str(INPUT_SPEC).unwrap();
    for directory in spec["directories"].as_array().unwrap() {
        fs::create_dir_all(fixture.0.join(directory.as_str().unwrap())).unwrap();
    }
    for file in spec["files"].as_array().unwrap() {
        let file = fixture.0.join(file.as_str().unwrap());
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "initial").unwrap();
    }
    fs::write(fixture.0.join(INPUT_SPEC_PATH), INPUT_SPEC).unwrap();
    for file in [
        "rust-toolchain.toml",
        ".cargo/config.toml",
        "compliance/rust-sources.json",
        "docs/THIRD_PARTY_RUST_SOURCES.md",
    ] {
        let before = source_inventory(&fixture.0).unwrap();
        fs::write(fixture.0.join(file), "changed").unwrap();
        let after = source_inventory(&fixture.0).unwrap();
        assert_ne!(before, after, "{file} must invalidate the build inventory");
    }
    let dependency = fixture
        .0
        .join("src/foundation/web-runtime/reactive-state/src/lib.rs");
    fs::create_dir_all(dependency.parent().unwrap()).unwrap();
    fs::write(&dependency, "upstream").unwrap();
    let before = source_inventory(&fixture.0).unwrap();
    fs::write(&dependency, "project customization").unwrap();
    assert_ne!(
        before,
        source_inventory(&fixture.0).unwrap(),
        "Nested framework dependency changes must invalidate production assets"
    );
    assert!(
        source_inventory(&fixture.0)
            .unwrap()
            .contains_key(INPUT_SPEC_PATH)
    );
    fs::write(fixture.0.join(INPUT_SPEC_PATH), format!("{INPUT_SPEC}\n")).unwrap();
    assert!(
        source_inventory(&fixture.0)
            .unwrap_err()
            .to_string()
            .contains("rebuild web-build")
    );
}

#[test]
fn inventory_and_copy_share_regular_file_bytes_and_nested_paths() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    let output = fixture.0.join("output");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir(&output).unwrap();
    fs::write(source.join("nested/file.css"), b"body {}").unwrap();
    copy_tree(&source, &output).unwrap();
    let mut inventory = BTreeMap::new();
    inventory_tree(&source, &source, &mut inventory).unwrap();
    assert_eq!(
        inventory["nested/file.css"],
        "62368a1a29259b30bac235c0e75dc700c9b3bacf1513ad5708e4fe4a6c0d6560"
    );
    assert_eq!(
        fs::read(output.join("nested/file.css")).unwrap(),
        b"body {}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_rejects_linked_roots_children_and_nonregular_inputs() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    fs::create_dir(&root).unwrap();
    symlink(&root, fixture.0.join("linked-root")).unwrap();
    assert!(
        inventory_tree(
            &fixture.0,
            &fixture.0.join("linked-root"),
            &mut BTreeMap::new()
        )
        .is_err()
    );
    fs::write(root.join("real.css"), b"body {}").unwrap();
    symlink(root.join("real.css"), root.join("linked.css")).unwrap();
    assert!(inventory_tree(&root, &root, &mut BTreeMap::new()).is_err());
    assert!(read_input(&root.join("linked.css")).is_err());
    fs::remove_file(root.join("linked.css")).unwrap();
    let fifo = root.join("blocked.css");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        read_input(&fifo)
            .unwrap_err()
            .to_string()
            .contains("special files")
    );
    assert!(inventory_tree(&root, &root, &mut BTreeMap::new()).is_err());
    let output = fixture.0.join("output");
    fs::create_dir(&output).unwrap();
    assert!(copy_tree(&root, &output).is_err());
}

#[test]
fn gzip_assets_are_reproducible_and_roundtrip() {
    use std::io::Read;
    for bytes in [b"".as_slice(), b"\0asm\x01\0\0\0", b"release web asset"] {
        let encoded = super::gzip_asset(bytes).unwrap();
        assert_eq!(encoded, super::gzip_asset(bytes).unwrap());
        assert_eq!(&encoded[4..8], &[0; 4]);
        let mut restored = Vec::new();
        compression::streams::read::GzDecoder::new(encoded.as_slice())
            .read_to_end(&mut restored)
            .unwrap();
        assert_eq!(restored, bytes);
    }
}

#[cfg(unix)]
#[test]
fn inputs_reject_symbolic_links_in_ancestors() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let source = fixture.0.join("source/nested");
    let destination = fixture.0.join("output");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(source.join("file.css"), "body {}").unwrap();
    let linked = fixture.0.join("linked");
    symlink(fixture.0.join("source"), &linked).unwrap();
    assert!(read_input(&linked.join("nested/file.css")).is_err());
    assert!(inventory_tree(&fixture.0, &linked.join("nested"), &mut BTreeMap::new()).is_err());
    assert!(copy_tree(&linked.join("nested"), &destination).is_err());
    assert!(fs::read_dir(destination).unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn inventory_rejects_lossy_and_alternate_path_spellings() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    for name in [
        OsString::from_vec(b"invalid-\xff.css".to_vec()),
        OsString::from("nested\\file.css"),
        OsString::from("file:stream"),
        OsString::from("file\n.css"),
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.0.join(&name), "body {}").unwrap();
        assert!(
            inventory_tree(&fixture.0, &fixture.0, &mut BTreeMap::new()).is_err(),
            "{name:?}"
        );
        fs::remove_file(fixture.0.join(&name)).unwrap();
        fs::create_dir(fixture.0.join(&name)).unwrap();
        assert!(
            inventory_tree(&fixture.0, &fixture.0, &mut BTreeMap::new()).is_err(),
            "empty directory {name:?}"
        );
    }
}

fn test_lock(fixture: &Fixture) -> BuildLock {
    let path = fixture.0.join(".web-build-lock");
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    BuildLock {
        path: Some(path),
        _file: file,
    }
}

#[test]
fn build_lock_finish_releases_on_success_and_build_error() {
    let fixture = Fixture::new();
    test_lock(&fixture).finish(Ok(())).unwrap();
    assert!(!fixture.0.join(".web-build-lock").exists());
    let error = test_lock(&fixture)
        .finish(Err(
            std::io::Error::from(std::io::ErrorKind::Interrupted).into()
        ))
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::Interrupted
    );
    assert!(!fixture.0.join(".web-build-lock").exists());
}

#[test]
fn build_lock_finish_preserves_cleanup_and_primary_errors() {
    let fixture = Fixture::new();
    let lock = test_lock(&fixture);
    fs::remove_file(fixture.0.join(".web-build-lock")).unwrap();
    let error = lock.finish(Ok(())).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::NotFound
    );
    let lock = test_lock(&fixture);
    fs::remove_file(fixture.0.join(".web-build-lock")).unwrap();
    let error = lock
        .finish(Err(
            std::io::Error::from(std::io::ErrorKind::Interrupted).into()
        ))
        .unwrap_err();
    let both = error.downcast_ref::<BuildAndCleanupError>().unwrap();
    assert_eq!(both.cleanup.kind(), std::io::ErrorKind::NotFound);
    assert_eq!(
        both.build.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::Interrupted
    );
}

#[test]
fn build_lock_unwind_fallback_releases_lock() {
    let fixture = Fixture::new();
    drop(test_lock(&fixture));
    assert!(!fixture.0.join(".web-build-lock").exists());
}
