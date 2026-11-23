// Build the internal Linux ABI and password verifier.
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

const PASSWORD: &str = "src/password/verification";
const PASSWORD_C: [(&str, &str); 6] = [
    ("argon2.c", "argon2.o"),
    ("core.c", "core.o"),
    ("encoding.c", "encoding.o"),
    ("ref.c", "ref.o"),
    ("thread.c", "thread.o"),
    ("blake2/blake2b.c", "blake2b.o"),
];
const PASSWORD_HEADERS: [&str; 7] = [
    "argon2.h",
    "core.h",
    "encoding.h",
    "thread.h",
    "blake2/blake2.h",
    "blake2/blake2-impl.h",
    "blake2/blamka-round-ref.h",
];

fn main() {
    assert_eq!(
        env::var("CARGO_CFG_TARGET_OS").unwrap(),
        "linux",
        "ZXS requires Linux"
    );
    println!("cargo:rerun-if-changed=src/linux.c");
    for p in PASSWORD_C.iter().map(|&(p, _)| p).chain(PASSWORD_HEADERS) {
        println!("cargo:rerun-if-changed={PASSWORD}/{p}");
    }
    for k in ["CC", "AR"] {
        println!("cargo:rerun-if-env-changed={k}");
    }
    let cross = env::var_os("HOST") != env::var_os("TARGET");
    let tool = |key, default: &str| {
        env::var_os(key).unwrap_or_else(|| {
            assert!(
                !cross,
                "set {key} to the target toolchain when cross-compiling"
            );
            default.into()
        })
    };
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let linux = out.join("linux.o");
    let cc = tool("CC", "cc");
    let compile = |source: &str, object: &Path, flags: &[&str]| {
        assert!(
            Command::new(&cc)
                .args(["-std=c11", "-fPIC"])
                .args(flags)
                .arg("-c")
                .arg(source)
                .arg("-o")
                .arg(object)
                .status()
                .expect("start target C compiler")
                .success(),
            "compile {source}"
        );
    };
    compile("src/linux.c", &linux, &["-Wall", "-Wextra", "-Werror"]);
    let password_dir = out.join("password");
    std::fs::create_dir_all(&password_dir).expect("password verifier build directory");
    let password_headers = format!("-I{PASSWORD}");
    let password_objects = PASSWORD_C.map(|(source, name)| {
        let dst = password_dir.join(name);
        // Keep the reference implementation's p=4 worker threads and memory clearing.
        compile(
            &format!("{PASSWORD}/{source}"),
            &dst,
            &["-O2", "-fvisibility=hidden", "-pthread", &password_headers],
        );
        dst
    });
    let ar = tool("AR", "ar");
    let archive = |name: &str, members: &[PathBuf]| {
        let output = out.join(name);
        // Rebuild the archive; ar updates present members but does not drop old ones.
        match std::fs::remove_file(&output) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("remove old build archive {name}: {e}"),
        }
        assert!(
            Command::new(&ar)
                .arg("crs")
                .arg(&output)
                .args(members)
                .status()
                .expect("start target archiver")
                .success(),
            "archive {name}"
        );
    };
    archive("libzxs_linux.a", &[linux]);
    archive("libzxs_password.a", &password_objects);
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=zxs_linux");
    println!("cargo:rustc-link-lib=static=zxs_password");
    println!("cargo:rustc-link-lib=pthread");
}
