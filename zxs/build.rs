// This shim needs only the target Linux C ABI. Internal password verification
// separately links the upstream target libargon2 from Rust. Let Linux headers
// supply flags and dirent layout rather than embedding architecture constants.
use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=src/linux.c");
    println!("cargo:rerun-if-env-changed=CC");
    println!("cargo:rerun-if-env-changed=AR");
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").unwrap(), "linux", "ZXS requires Linux");
    let cross = env::var_os("HOST") != env::var_os("TARGET");
    let tool = |key, default: &str| env::var_os(key).unwrap_or_else(|| {
        assert!(!cross, "set {key} to the target toolchain when cross-compiling");
        default.into()
    });
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let object = out.join("linux.o");
    assert!(Command::new(tool("CC","cc")).args(["-std=c11","-fPIC","-Wall","-Wextra","-Werror","-c","src/linux.c","-o"])
        .arg(&object).status().expect("start target C compiler").success(), "compile Linux ABI shim");
    assert!(Command::new(tool("AR","ar")).arg("crs").arg(out.join("libzxs_linux.a")).arg(object)
        .status().expect("start target archiver").success(), "archive Linux ABI shim");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=zxs_linux");
}
