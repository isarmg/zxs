use super::*;
use std::{error::Error, os::unix::process::ExitStatusExt};

#[test]
fn worker_io_failures_keep_the_actual_source_and_context_without_recategorizing_it() {
    for stage in [
        Stage::Spawn,
        Stage::WriteRequest,
        Stage::ReadReply,
        Stage::DrainReply,
        Stage::Wait,
    ] {
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::NotFound,
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::TimedOut,
        ] {
            let error = Failure::io(stage, io::Error::new(kind, "original native source"));
            let source = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
            assert_eq!(source.kind(), kind);
            assert_eq!(source.to_string(), "original native source");
            assert!(error.to_string().contains(&stage.to_string()));
            assert!(error.to_string().contains("preserve original intent"));
            assert!(!error.to_string().contains("corrupt"));
        }
    }
}

#[test]
fn unsuccessful_exit_and_signal_remain_distinct_from_oversized_successful_reply() {
    for status in [ExitStatus::from_raw(7 << 8), ExitStatus::from_raw(9)] {
        let expected = status.to_string();
        let error = admit_reply(status, 65, 64).unwrap_err();
        assert!(matches!(&error, Failure::Exit(_)));
        assert!(error.to_string().contains(&expected));
        assert!(!error.to_string().contains("exceeded"));
    }
    let error = admit_reply(ExitStatus::from_raw(0), 65, 64).unwrap_err();
    assert!(matches!(error, Failure::ReplyBudget { maximum: 64 }));
    admit_reply(ExitStatus::from_raw(0), 64, 64).unwrap();
}
