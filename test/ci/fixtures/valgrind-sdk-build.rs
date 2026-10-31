use std::{env, path::PathBuf};

fn main() {
    let fixtures = PathBuf::from(env::var_os("ZXS_VALGRIND_FIXTURES").expect("fixture directory"));
    let include = PathBuf::from(env::var_os("ZXS_VALGRIND_INCLUDE").expect("Valgrind SDK include"));
    cc::Build::new()
        .file(fixtures.join("valgrind-sdk.c"))
        .include(&fixtures)
        .include(&include)
        .warnings_into_errors(true)
        .compile("zxs_valgrind_sdk");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let bindings = output.join("valgrind-sdk.rs");
    bindgen::Builder::default()
        .header(
            fixtures
                .join("valgrind-sdk.h")
                .to_str()
                .expect("UTF-8 fixture path"),
        )
        .allowlist_function("zxs_valgrind_.*")
        .generate()
        .expect("actual SDK wrapper bindings")
        .write_to_file(&bindings)
        .expect("write generated bindings");
    println!(
        "cargo:rustc-env=ZXS_PKI_VALGRIND_BINDINGS={}",
        bindings.display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        fixtures.join("valgrind-sdk.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        fixtures.join("valgrind-sdk.c").display()
    );
    println!("cargo:rerun-if-env-changed=ZXS_VALGRIND_INCLUDE");
}
