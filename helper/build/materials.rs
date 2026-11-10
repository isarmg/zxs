use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
};

pub const VARIABLES: &[&str] = &[
    "ZXS_HELPER_MATERIALS",
    "ZXS_HELPER_MATERIALS_SHA256",
    "ZXS_HELPER_MATERIALS_MANIFEST",
    "ZXS_HELPER_MATERIALS_MANIFEST_SHA256",
];

pub struct Binding<'a> {
    pub version: &'a str,
    pub commit: &'a str,
    pub tree: &'a str,
    pub target: &'a str,
    pub payload_sha256: &'a str,
}

pub fn absent(out: &Path) {
    for name in VARIABLES {
        assert!(
            env::var_os(name).is_none(),
            "legal materials cannot be configured without the embedded ZXS payload: {name}"
        );
    }
    fs::write(
        out.join("install_materials.rs"),
        "const MATERIALS: Option<Embedded> = None;\n",
    )
    .expect("write explicit development material absence");
}

pub(super) fn snapshot(
    out: &Path,
    variable: &str,
    checksum: &str,
    filename: &str,
    limit: u64,
) -> (String, Vec<u8>) {
    let path = PathBuf::from(
        env::var_os(variable)
            .unwrap_or_else(|| panic!("{variable} is required with an embedded payload")),
    );
    assert!(
        path.is_absolute(),
        "embedded material input path must be absolute"
    );
    let metadata = fs::symlink_metadata(&path).expect("read embedded material metadata");
    assert!(
        metadata.file_type().is_file() && metadata.len() > 0 && metadata.len() <= limit,
        "embedded material must be a bounded regular file, not a symlink"
    );
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .expect("open embedded material")
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .expect("read embedded material");
    assert_eq!(
        bytes.len() as u64,
        metadata.len(),
        "embedded material changed length while being read"
    );
    let digest = crate::lower_hex(&Sha256::digest(&bytes));
    assert_eq!(
        digest,
        env::var(checksum).unwrap_or_else(|_| panic!("{checksum} is required")),
        "embedded material differs from the exact native build result"
    );
    println!("cargo:rerun-if-changed={}", path.display());
    fs::write(out.join(filename), &bytes).expect("snapshot embedded material bytes");
    (digest, bytes)
}

pub fn prepare(out: &Path, binding: Binding<'_>) {
    let (archive_sha, archive) = snapshot(
        out,
        VARIABLES[0],
        VARIABLES[1],
        "license-materials.tar.gz",
        32 * 1024 * 1024,
    );
    assert!(
        archive.starts_with(b"\x1f\x8b\x08"),
        "embedded legal material archive must be gzip encoded"
    );
    let (manifest_sha, _) = snapshot(
        out,
        VARIABLES[2],
        VARIABLES[3],
        "license-materials.json",
        4 * 1024 * 1024,
    );
    let Binding {
        version,
        commit,
        tree,
        target,
        payload_sha256,
    } = binding;
    let generated = format!(
        "const MATERIALS: Option<Embedded> = Some(Embedded {{ archive: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/license-materials.tar.gz\")), manifest: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/license-materials.json\")), archive_sha256: {archive_sha:?}, manifest_sha256: {manifest_sha:?}, version: {version:?}, commit: {commit:?}, tree: {tree:?}, target: {target:?}, payload_sha256: {payload_sha256:?} }});\n"
    );
    fs::write(out.join("install_materials.rs"), generated)
        .expect("write embedded material identity");
}
