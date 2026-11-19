// Build the Linux ABI and exactly the vendored upstream SQLite core. No
// download, sqlite shell, system sqlite pkg-config lookup or loadable module.
use std::{env, path::{Path,PathBuf}, process::Command};

fn main() {
    const CONTROL:&str="samba/debian-trixie/new/source3/param/zxs_smb_control.c";
    println!("cargo:rerun-if-changed=samba/debian-trixie/new/source3/param/zxs_smb_control.h");
    println!("cargo:rerun-if-changed=samba/debian-trixie/new/source3/param/zxs_smb_workers.inc");
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").unwrap(), "linux", "ZXS requires Linux");
    for p in [CONTROL,"src/linux.c","src/sqlite_read.c","vendor/sqlite/sqlite3.c","vendor/sqlite/sqlite3.h"]{
        println!("cargo:rerun-if-changed={p}");
    }
    for k in ["CC","AR"]{println!("cargo:rerun-if-env-changed={k}");}
    let cross=env::var_os("HOST")!=env::var_os("TARGET");
    let tool=|key,default:&str|env::var_os(key).unwrap_or_else(||{
        assert!(!cross,"set {key} to the target toolchain when cross-compiling");default.into()
    });
    let out=PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let objects=[out.join("linux.o"),out.join("sqlite-read.o"),out.join("sqlite3.o"),out.join("smb-control.o")];
    let cc=tool("CC","cc");
    let compile=|source:&str,object:&Path,flags:&[&str]|{
        assert!(Command::new(&cc).args(["-std=c11","-fPIC","-Ivendor/sqlite"]).args(flags)
            .arg("-c").arg(source).arg("-o").arg(object).status().expect("start target C compiler").success(),
            "compile {source}");
    };
    compile(CONTROL,&objects[3], &["-Wall","-Wextra","-Werror"]);
    compile("src/linux.c",&objects[0], &["-Wall","-Wextra","-Werror"]);
    compile("src/sqlite_read.c",&objects[1], &["-Wall","-Wextra","-Werror"]);
    compile("vendor/sqlite/sqlite3.c",&objects[2], &["-O2","-D_GNU_SOURCE","-DSQLITE_THREADSAFE=1",
        "-DSQLITE_OMIT_LOAD_EXTENSION","-DSQLITE_OMIT_WAL","-DSQLITE_OMIT_SHARED_CACHE","-DSQLITE_DQS=0","-DSQLITE_USE_URI=0"]);
    assert!(Command::new(tool("AR","ar")).arg("crs").arg(out.join("libzxs_linux.a")).args(&objects)
        .status().expect("start target archiver").success(),"archive Linux/SQLite core");
    println!("cargo:rustc-link-search=native={}",out.display());
    println!("cargo:rustc-link-lib=static=zxs_linux");
    println!("cargo:rustc-link-lib=pthread");
}
