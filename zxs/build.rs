// Build the Linux ABI and exact vendored SQLite and Argon2 sources. No
// download, system crypto/SQLite fallback, package discovery or CLI tools.
use std::{env, path::{Path,PathBuf}, process::Command};

const ARGON2:&str="vendor/argon2";
const ARGON2_C:[(&str,&str);6]=[
    ("src/argon2.c","argon2.o"),("src/core.c","core.o"),("src/encoding.c","encoding.o"),
    ("src/ref.c","ref.o"),("src/thread.c","thread.o"),("src/blake2/blake2b.c","blake2b.o"),
];
const ARGON2_HEADERS:[&str;7]=["include/argon2.h","src/core.h","src/encoding.h","src/thread.h",
    "src/blake2/blake2.h","src/blake2/blake2-impl.h","src/blake2/blamka-round-ref.h"];

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").unwrap(), "linux", "ZXS requires Linux");
    for p in ["src/linux.c","src/sqlite_read.c","vendor/sqlite/sqlite3.c","vendor/sqlite/sqlite3.h"]{
        println!("cargo:rerun-if-changed={p}");
    }
    for p in ARGON2_C.iter().map(|&(p,_)|p).chain(ARGON2_HEADERS){
        println!("cargo:rerun-if-changed={ARGON2}/{p}");
    }
    for k in ["CC","AR"]{println!("cargo:rerun-if-env-changed={k}");}
    let cross=env::var_os("HOST")!=env::var_os("TARGET");
    let tool=|key,default:&str|env::var_os(key).unwrap_or_else(||{
        assert!(!cross,"set {key} to the target toolchain when cross-compiling");default.into()
    });
    let out=PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let objects=[out.join("linux.o"),out.join("sqlite-read.o"),out.join("sqlite3.o")];
    let cc=tool("CC","cc");
    let compile=|source:&str,object:&Path,flags:&[&str]|{
        assert!(Command::new(&cc).args(["-std=c11","-fPIC","-Ivendor/sqlite"]).args(flags)
            .arg("-c").arg(source).arg("-o").arg(object).status().expect("start target C compiler").success(),
            "compile {source}");
    };
    compile("src/linux.c",&objects[0], &["-Wall","-Wextra","-Werror"]);
    compile("src/sqlite_read.c",&objects[1], &["-Wall","-Wextra","-Werror"]);
    compile("vendor/sqlite/sqlite3.c",&objects[2], &["-O2","-D_GNU_SOURCE","-DSQLITE_THREADSAFE=1",
        "-DSQLITE_OMIT_LOAD_EXTENSION","-DSQLITE_OMIT_WAL","-DSQLITE_OMIT_SHARED_CACHE","-DSQLITE_DQS=0","-DSQLITE_USE_URI=0"]);
    let argon_dir=out.join("argon2");
    std::fs::create_dir_all(&argon_dir).expect("Argon2 private build directory");
    let argon_headers=format!("-I{ARGON2}/include");
    let argon_objects=ARGON2_C.map(|(source,name)|{
        let dst=argon_dir.join(name);
        // Upstream reference (no CPU-specific code or generated tools).
        // Keep upstream p=4 worker threads and default memory clearing.
        compile(&format!("{ARGON2}/{source}"),&dst,&["-O2","-fvisibility=hidden","-pthread",&argon_headers]);
        dst
    });
    let ar=tool("AR","ar");
    let archive=|name:&str, members:&[PathBuf]|{
        let output=out.join(name);
        // ar replaces entries but does not drop omitted members. Always
        // rebuild this private archive so an old target directory cannot
        // retain a member from the retired transport.
        match std::fs::remove_file(&output){
            Ok(())=>{},
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
            Err(e)=>panic!("remove old build archive {name}: {e}"),
        }
        assert!(Command::new(&ar).arg("crs").arg(&output).args(members)
            .status().expect("start target archiver").success(),"archive {name}");
    };
    archive("libzxs_linux.a",&objects);
    archive("libzxs_argon2.a",&argon_objects);
    println!("cargo:rustc-link-search=native={}",out.display());
    println!("cargo:rustc-link-lib=static=zxs_linux");
    println!("cargo:rustc-link-lib=static=zxs_argon2");
    println!("cargo:rustc-link-lib=pthread");
}
