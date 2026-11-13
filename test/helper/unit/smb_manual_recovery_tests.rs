use super::*;
use command_line::Parser;

fn parse(extra: &[&str]) -> crate::Args {
    let mut args = vec![
        "zxs-helper", "smb-manual-recovery", "--socket", "/run/zxs/smb.sock",
        "--username", "alice", "--operation", "op_00000000000000000000000000000001",
    ];
    args.extend_from_slice(extra);
    crate::Args::try_parse_from(args).unwrap()
}

#[test]
fn default_and_explicit_query_are_read_only_and_keep_original_operation() {
    for extra in [vec![], vec!["query"]] {
        let args = parse(&extra);
        assert!(!args.requires_host_lease());
        let crate::Command::SmbManualRecovery(options) = args.command.unwrap() else {
            panic!("explicit online recovery command");
        };
        assert!(!options.resumes());
        assert_eq!(options.operation.to_string(), "op_00000000000000000000000000000001");
        assert_eq!(options.username, "alice");
        assert_eq!(options.socket, PathBuf::from("/run/zxs/smb.sock"));
    }
}

#[test]
fn only_explicit_resume_can_write_and_all_offline_commands_keep_host_lease() {
    let args = parse(&["resume"]);
    assert!(!args.requires_host_lease());
    let crate::Command::SmbManualRecovery(options) = args.command.unwrap() else {
        panic!("explicit online recovery command");
    };
    assert!(options.resumes());
    assert!(crate::Args::try_parse_from(["zxs-helper"]).unwrap().requires_host_lease());
    assert!(crate::Args::try_parse_from(["zxs-helper", "workspace-inventory"])
        .unwrap().requires_host_lease());
    assert!(crate::Args::try_parse_from(["zxs-helper", "scrub"])
        .unwrap().requires_host_lease());
}

#[test]
fn online_recovery_requires_original_typed_operation_socket_and_account() {
    assert!(crate::Args::try_parse_from(["zxs-helper", "smb-manual-recovery"]).is_err());
    assert!(crate::Args::try_parse_from([
        "zxs-helper", "smb-manual-recovery", "--socket", "/run/zxs/smb.sock",
        "--username", "alice", "--operation", "new-operation", "resume",
    ]).is_err());
}
