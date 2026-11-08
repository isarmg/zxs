use super::*;
#[test]
fn canonical_semver_only() {
    for tag in [
        "v0.1.0",
        "v1.2.3-rc.1",
        "v2.0.0+build.3",
        "v1.0.0-beta.2+abc",
    ] {
        assert!(tag_version(tag).is_ok(), "{tag}");
    }
    for tag in [
        "1.0.0",
        "v1.0",
        "v01.0.0",
        "v1.0.0-01",
        "v1.0.0\n",
        "v1.0.0;echo bad",
        "v1.0.0-",
    ] {
        assert!(tag_version(tag).is_err(), "{tag}");
    }
}
#[test]
fn real_manifests_match_workspace() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workspace = toml_file(&root.join("Cargo.toml")).unwrap();
    let version = workspace["workspace"]["package"]["version"]
        .as_str()
        .unwrap();
    validate(&root, &format!("v{version}"))
        .expect("every current product manifest must match the workspace release version");
    assert!(validate(&root, "v999999.0.0").is_err());
}

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "zxs-release-manifests-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let fixture = Self(path);
        fs::write(
            fixture.0.join("Cargo.toml"),
            "[workspace.package]\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        for relative in PRODUCT_MANIFESTS {
            let manifest = fixture.0.join(relative);
            fs::create_dir_all(manifest.parent().unwrap()).unwrap();
            fs::write(manifest, "[package]\nversion.workspace = true\n").unwrap();
        }
        fixture
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn root_helper_is_the_only_current_helper_manifest() {
    assert!(PRODUCT_MANIFESTS.contains(&"helper/Cargo.toml"));
    assert!(!PRODUCT_MANIFESTS.contains(&"src/helper/Cargo.toml"));
    assert!(!PRODUCT_MANIFESTS.contains(&"rescue/Cargo.toml"));
    let fixture = Fixture::new();
    assert!(!fixture.0.join("src/rescue").exists());
    assert_eq!(validate(&fixture.0, "v1.2.3").unwrap().to_string(), "1.2.3");
}

#[test]
fn a_retired_rescue_location_cannot_replace_the_missing_helper_manifest() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join("rescue")).unwrap();
    fs::rename(
        fixture.0.join("helper/Cargo.toml"),
        fixture.0.join("rescue/Cargo.toml"),
    )
    .unwrap();
    let error = validate(&fixture.0, "v1.2.3").unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn every_current_product_manifest_is_required() {
    let fixture = Fixture::new();
    for relative in PRODUCT_MANIFESTS {
        let manifest = fixture.0.join(relative);
        let bytes = fs::read(&manifest).unwrap();
        fs::remove_file(&manifest).unwrap();
        let error = validate(&fixture.0, "v1.2.3").unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::NotFound,
            "{relative}"
        );
        fs::write(manifest, bytes).unwrap();
    }
}

#[test]
fn root_helper_explicit_version_must_match_the_workspace() {
    let fixture = Fixture::new();
    let manifest = fixture.0.join("helper/Cargo.toml");
    fs::write(&manifest, "[package]\nversion = \"1.2.3\"\n").unwrap();
    validate(&fixture.0, "v1.2.3").unwrap();
    fs::write(&manifest, "[package]\nversion = \"2.0.0\"\n").unwrap();
    let error = validate(&fixture.0, "v1.2.3").unwrap_err().to_string();
    assert!(
        error.contains("helper/Cargo.toml") && error.contains("expected 1.2.3"),
        "{error}"
    );
}
