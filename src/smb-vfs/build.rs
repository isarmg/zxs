fn main() {
    assert!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
            && std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64")
            && std::env::var("CARGO_CFG_TARGET_POINTER_WIDTH").as_deref() == Ok("64"),
        "the loadable ZXS VFS module requires Linux amd64 (64-bit x86_64)"
    );
    println!("cargo:rerun-if-env-changed=ZXS_SMB_SDK_SOURCE");
    println!("cargo:rerun-if-env-changed=ZXS_SMB_SDK_BUILD");
    println!("cargo:rerun-if-env-changed=ZXS_SMB_SDK_CLANG_ARGS_JSON");
    #[cfg(feature = "smb-sdk")]
    build_sdk();
}

#[cfg(feature = "smb-sdk")]
fn build_sdk() {
    use std::{env, fs, path::PathBuf};
    let source =
        PathBuf::from(env::var_os("ZXS_SMB_SDK_SOURCE").expect(
            "smb-sdk requires the configured target 4.23.0 source tree in ZXS_SMB_SDK_SOURCE",
        ));
    let build = PathBuf::from(
        env::var_os("ZXS_SMB_SDK_BUILD")
            .unwrap_or_else(|| source.join("bin/default").into_os_string()),
    );
    let version = fs::read_to_string(source.join("VERSION")).expect("SDK VERSION file missing");
    for expected in [
        "SAMBA_VERSION_MAJOR=4",
        "SAMBA_VERSION_MINOR=23",
        "SAMBA_VERSION_RELEASE=0",
    ] {
        assert!(
            version.lines().any(|line| line.trim() == expected),
            "this module is pinned to SDK 4.23.0"
        );
    }
    let header = fs::read_to_string(source.join("source3/include/vfs.h"))
        .expect("official VFS header missing");
    assert!(
        header.contains("#define SMB_VFS_INTERFACE_VERSION 51"),
        "unexpected VFS SDK ABI"
    );
    assert!(
        build.join("include/config.h").exists(),
        "configure the target SDK first; generated include/config.h is required"
    );
    let mut generator=bindgen::Builder::default()
        .header_contents("zxs-vfs-wrapper.h", "#include \"includes.h\"\n#include \"smbd/smbd.h\"\n#include \"smbd/globals.h\"\n#include \"libcli/security/security.h\"\n#include \"librpc/gen_ndr/auth.h\"\n")
        .derive_default(true)
        .generate_comments(false)
        .layout_tests(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .allowlist_type("vfs_.*|files_struct|smb_filename|stat_ex|SMB_STRUCT_STAT|NTSTATUS|TALLOC_CTX|mode_t|off_t|tevent_.*|file_id|auth_session_info|auth_user_info|security_token|dom_sid")
        .allowlist_function("smb_register_vfs|lp_parm_const_string|talloc_strdup|fsp_get_.*fd|__tevent_req_create|_tevent_req_data|_tevent_req_done|tevent_req_post|tevent_req_received|_tevent_add_fd|tevent_fd_set_auto_close|_talloc_set_destructor|_talloc_free|tevent_context_init|_tevent_loop_once|tevent_req_is_in_progress|security_token_is_anonymous|security_token_has_builtin_guests|security_token_has_nt_authenticated_users")
        .allowlist_var("SMB_VFS_INTERFACE_VERSION|VFS_OPEN_HOW_.*|VFS_RENAME_HOW_.*|TEVENT_FD_READ|global_sid_Anonymous|global_sid_Authenticated_Users|global_sid_Builtin_Guests")
        .clang_arg("-D_GNU_SOURCE=1")
        .clang_arg("-D_XOPEN_SOURCE_EXTENDED=1")
        .clang_arg("-D__STDC_WANT_LIB_EXT1__=1")
        .clang_arg("-D_SAMBA_BUILD_=4")
        .clang_arg("-DHAVE_CONFIG_H=1");
    for directory in [
        source.clone(),
        source.join("source3"),
        source.join("source3/include"),
        source.join("lib/replace"),
        source.join("lib/talloc"),
        source.join("lib/tevent"),
        source.join("lib/tdb/include"),
        source.join("librpc"),
        build.clone(),
        build.join("include"),
        build.join("source3"),
        build.join("source3/include"),
        build.join("librpc"),
    ] {
        generator = generator.clang_arg(format!("-I{}", directory.display()));
    }
    for library in ["lib/tevent", "lib/talloc", "libcli/security", "source3"] {
        println!(
            "cargo:rustc-link-search=native={}",
            build.join(library).display()
        );
    }
    let extra = match env::var("ZXS_SMB_SDK_CLANG_ARGS_JSON") {
        Ok(extra) => Some(extra),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => {
            panic!("ZXS_SMB_SDK_CLANG_ARGS_JSON must be valid UTF-8")
        }
    };
    if let Some(extra) = extra {
        let arguments: Vec<String> = json_codec::from_str(&extra)
            .expect("ZXS_SMB_SDK_CLANG_ARGS_JSON must be a JSON array of clang arguments");
        generator = generator.clang_args(arguments);
    }
    generator
        .generate()
        .expect("failed to generate bindings against the configured target SDK")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("sdk.rs"))
        .unwrap();
}
