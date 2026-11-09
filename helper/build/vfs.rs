//! Snapshot the matching offline VFS product. No target-host code is executed.
use super::materials;
use std::{env, fs, path::Path};

pub const VARIABLES: &[&str] = &[
    "ZXS_HELPER_VFS",
    "ZXS_HELPER_VFS_SHA256",
    "ZXS_HELPER_VFS_RECEIPT",
    "ZXS_HELPER_VFS_RECEIPT_SHA256",
];

pub fn absent(out: &Path) {
    for name in VARIABLES {
        assert!(
            env::var_os(name).is_none(),
            "partial embedded VFS configuration: {name}"
        );
    }
    fs::write(out.join("install_vfs.rs"), "const EMBEDDED: Option<Payload> = None;\n")
        .expect("write explicit development VFS absence");
}

pub fn prepare(out: &Path, binding: &materials::Binding<'_>) {
    if env::var_os(VARIABLES[0]).is_none() {
        absent(out);
        return;
    }
    let (sha256, bytes) = materials::snapshot(
        out, VARIABLES[0], VARIABLES[1], "payload-zxs.so", 64 * 1024 * 1024,
    );
    assert!(
        bytes.len() >= 64
            && bytes[..7] == *b"\x7fELF\x02\x01\x01"
            && u16::from_le_bytes([bytes[16], bytes[17]]) == 3
            && u16::from_le_bytes([bytes[18], bytes[19]]) == 62
            && u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]) == 1
            && u16::from_le_bytes([bytes[52], bytes[53]]) == 64,
        "embedded VFS must be one ELF64 little-endian x86_64 shared object"
    );
    let (receipt_sha256, _) = materials::snapshot(
        out, VARIABLES[2], VARIABLES[3], "vfs-build.json", 64 * 1024,
    );
    let materials::Binding { version, commit, tree, target, .. } = binding;
    let generated = format!(
        "const EMBEDDED: Option<Payload> = Some(Payload {{ bytes: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/payload-zxs.so\")), receipt: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/vfs-build.json\")), version: {version:?}, commit: {commit:?}, tree: {tree:?}, target: {target:?}, sha256: {sha256:?}, receipt_sha256: {receipt_sha256:?} }});\n"
    );
    fs::write(out.join("install_vfs.rs"), generated).expect("write matching offline VFS identity");
}
