#[cfg(test)]
mod tests {
    use super::*;

    fn flags_build(vars: &[(&str, &str)]) -> Build {
        let mut build = Build::new();
        build
            .host("x86_64-unknown-linux-gnu")
            .target("x86_64-unknown-linux-gnu")
            .cargo_metadata(false)
            .shell_escaped_flags(true)
            .set_envs_snapshot(vars.iter().copied());
        build
    }

    #[test]
    fn shell_escaped_flags_preserve_quoted_arguments() {
        let build = flags_build(&[("CFLAGS", "-DVALUE='two words' -I\"include dir\" ''")]);
        assert_eq!(
            build.envflags("CFLAGS").unwrap().unwrap(),
            ["-DVALUE=two words", "-Iinclude dir", ""]
        );
    }

    #[test]
    fn shell_escaped_flags_reject_malformed_quoting() {
        for name in ["CFLAGS", "CXXFLAGS", "ARFLAGS", "RANLIBFLAGS"] {
            for value in ["-O2 'unclosed", "-O2 \"unclosed", "-O2 trailing\\"] {
                let build = flags_build(&[(name, value)]);
                let error = build.envflags(name).unwrap_err();
                assert!(matches!(error.kind, ErrorKind::InvalidArgument));
                assert!(error.message.contains(name));
                assert!(!error.message.contains(value));
            }
        }
    }

    #[test]
    fn shell_escaped_flags_report_the_target_variable() {
        for name in [
            "HOST_CFLAGS",
            "CFLAGS_x86_64_unknown_linux_gnu",
            "CFLAGS_x86_64-unknown-linux-gnu",
        ] {
            let build = flags_build(&[("CFLAGS", "-O2"), (name, "'unclosed")]);
            let error = build.envflags("CFLAGS").unwrap_err();
            assert!(matches!(error.kind, ErrorKind::InvalidArgument));
            assert!(error.message.contains(name));
        }
    }

    #[test]
    fn malformed_environment_flags_do_not_add_partial_arguments() {
        let mut build = flags_build(&[("CFLAGS", "-O2 'unclosed")]);
        build.flag("-DBEFORE");
        assert!(build.try_flags_from_environment("CFLAGS").is_err());
        assert_eq!(build.flags.as_slice(), [Arc::<OsStr>::from(OsStr::new("-DBEFORE"))]);

        build.shell_escaped_flags(false);
        assert_eq!(
            build.envflags("CFLAGS").unwrap().unwrap(),
            ["-O2", "'unclosed"]
        );
    }

    #[test]
    fn test_android_clang_compiler_uses_target_arg_internally() {
        for version in 16..21 {
            assert!(android_clang_compiler_uses_target_arg_internally(
                &PathBuf::from(format!("armv7a-linux-androideabi{}-clang", version))
            ));
            assert!(android_clang_compiler_uses_target_arg_internally(
                &PathBuf::from(format!("armv7a-linux-androideabi{}-clang++", version))
            ));
        }
        assert!(!android_clang_compiler_uses_target_arg_internally(
            &PathBuf::from("clang-i686-linux-android")
        ));
        assert!(!android_clang_compiler_uses_target_arg_internally(
            &PathBuf::from("clang")
        ));
        assert!(!android_clang_compiler_uses_target_arg_internally(
            &PathBuf::from("clang++")
        ));
    }
}
