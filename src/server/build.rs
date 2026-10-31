//! Explicit release-only Web embedding; ordinary development builds stay standalone.
use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    for key in [
        "ZXS_EMBED_WEB",
        "ZXS_EMBED_WEB_COMMIT",
        "ZXS_EMBED_WEB_TREE",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!(
        "cargo:rerun-if-changed={}",
        root.join(".github/scripts/embed-web-assets.py").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join(".github/scripts/verify-web-dist.py").display()
    );
    if env::var_os("ZXS_EMBED_WEB").is_none() {
        fs::write(out.join("embedded_web.rs"), "static BUNDLE: Bundle = Bundle { source_commit: \"\", source_tree: \"\", manifest_sha256: \"\", modified_seconds: 0, files: &[] };\n").unwrap();
        return;
    }
    assert_eq!(
        env::var("ZXS_EMBED_WEB").as_deref(),
        Ok("1"),
        "ZXS_EMBED_WEB must be 1 or absent"
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("build/web").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("tools/web-build/build-inputs.json").display()
    );
    let result = Command::new("python3")
        .arg(root.join(".github/scripts/embed-web-assets.py"))
        .arg("--root")
        .arg(&root)
        .arg("--out")
        .arg(&out)
        .status()
        .expect("run Web embedding verifier");
    assert!(result.success(), "Web embedding verification failed");
}
