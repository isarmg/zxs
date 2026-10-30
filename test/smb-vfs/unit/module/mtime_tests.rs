use super::*;
use smb_protocol::protocol::{FrameStream, PROTOCOL, REQUIRED_CAPABILITIES, Response, SavedMtime};
use std::{
    fs::File,
    os::unix::{fs::FileExt, net::UnixListener},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[link(name = "samba-util")]
unsafe extern "C" {}

fn saved(seconds: u64) -> FileMtime {
    FileMtime::Saved {
        unix_seconds: SavedMtime::from_unix_seconds(seconds).unwrap(),
    }
}

#[test]
fn mtime_registered_sdk_callbacks_project_save_time_and_keep_read_handles_fixed() {
    for writable in [false, true] {
        let stem = format!(
            "zxs-vfs-mtime-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let socket = std::env::temp_dir().join(format!("{stem}.sock"));
        let work = std::env::temp_dir().join(format!("{stem}.work"));
        let listener = UnixListener::bind(&socket).unwrap();
        let worker = std::thread::spawn(move || {
            let mut wire =
                FrameStream::with_timeout(listener.accept().unwrap().0, Duration::from_secs(5))
                    .unwrap();
            assert!(matches!(
                wire.receive::<Request>().unwrap().0,
                Request::Hello { .. }
            ));
            wire.send(&Response::Ok { result: json_codec::json!({"protocol":PROTOCOL,"capabilities":REQUIRED_CAPABILITIES}) }, None).unwrap();
            for _ in 0..2 {
                assert!(
                    matches!(wire.receive::<Request>().unwrap().0, Request::Stat { path } if path == "file")
                );
                wire.send(
                    &Response::Ok {
                        result: json_codec::to_value(StatResult {
                            entry_id: "file".into(),
                            kind: NamespaceEntryKind::File,
                            path: "file".into(),
                            name: "file".into(),
                            size: 3,
                            mtime: saved(1_600_000_000),
                        })
                        .unwrap(),
                    },
                    None,
                )
                .unwrap();
            }
            for mtime in [
                FileMtime::Unpublished {},
                saved(1_600_000_000),
                saved(2_147_483_648),
            ] {
                assert!(matches!(
                    wire.receive::<Request>().unwrap().0,
                    Request::AuthorizeRead { handle: 7 }
                ));
                wire.send(
                    &Response::Ok {
                        result: json_codec::to_value(AuthorizedMetadata { mtime }).unwrap(),
                    },
                    None,
                )
                .unwrap();
            }
            for code in [
                storage_contract::ErrorCode::Forbidden,
                storage_contract::ErrorCode::SmbRebindRequired,
            ] {
                assert!(matches!(
                    wire.receive::<Request>().unwrap().0,
                    Request::AuthorizeRead { handle: 7 }
                ));
                wire.send(
                    &Response::Error {
                        code,
                        message: "metadata admission changed".into(),
                        retryable: false,
                        operation_id: None,
                    },
                    None,
                )
                .unwrap();
            }
        });
        let file = if writable {
            let file = std::fs::OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(&work)
                .unwrap();
            file.write_all_at(b"new", 0).unwrap();
            file.set_times(
                std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(42)),
            )
            .unwrap();
            file
        } else {
            File::open(std::env::temp_dir()).unwrap()
        };
        let client = Client::connect(&socket, "alice", "repository").unwrap();
        let mut fsp: sdk::files_struct = Default::default();
        let opened = Open {
            info: Arc::new(OpenResult {
                entry_id: "file".into(),
                kind: NamespaceEntryKind::File,
                path: "file".into(),
                handle: 7,
                size: 3,
                max_file_bytes: 4096,
                version_id: Some("version".into()),
                workspace_id: writable.then(|| "workspace".into()),
                mtime: saved(1_500_000_000),
            }),
            fd: file.try_clone().unwrap().into(),
            writable,
            io: Arc::new(Mutex::new(WriteSequence::new(false))),
        };
        let context = Box::new(Context {
            samba_username: "alice".into(),
            client,
            files: [(&mut fsp as *mut _ as usize, opened)]
                .into_iter()
                .collect(),
        });
        let mut authenticated = authentication::tests::AuthenticatedSession::new();
        let mut host = authenticated.handle();
        host.data = Box::into_raw(context).cast();
        let mut name = sdk::smb_filename {
            base_name: c"file".as_ptr().cast_mut(),
            ..Default::default()
        };
        // SAFETY: All callbacks come from the registered, generated SDK table.
        // SDK host/session/name/output storage stays live on this event thread;
        // fsp is our exact owned lookup key and Context is reclaimed once below.
        unsafe {
            assert_eq!(samba_init_module().v, 0);
            let callbacks = FUNCTIONS.get().unwrap();
            let mut output: sdk::SMB_STRUCT_STAT = Default::default();
            assert_eq!(callbacks.stat_fn.unwrap()(&mut host, &mut name), 0);
            assert_eq!(name.st.st_ex_mtime.tv_sec, 1_600_000_000);
            assert_eq!(name.st.st_ex_mtime.tv_nsec, 0);
            assert_eq!(
                callbacks.fstatat_fn.unwrap()(&mut host, std::ptr::null(), &name, &mut output, 0),
                0
            );
            assert_eq!(output.st_ex_mtime.tv_sec, 1_600_000_000);
            for seconds in [0, 1_600_000_000, 2_147_483_648] {
                assert_eq!(
                    callbacks.fstat_fn.unwrap()(&mut host, &mut fsp, &mut output),
                    0
                );
                assert_eq!(output.st_ex_size, 3);
                assert_eq!(
                    output.st_ex_mtime.tv_sec,
                    if writable { seconds } else { 1_500_000_000 }
                );
                assert_eq!(
                    output.st_ex_mtime.tv_nsec,
                    if writable && seconds == 0 {
                        i64::from(sdk::SAMBA_UTIME_OMIT)
                    } else {
                        0
                    }
                );
                assert_eq!(output.st_ex_atime.tv_sec, 0);
                assert_eq!(output.st_ex_ctime.tv_sec, 0);
                assert_eq!(output.st_ex_btime.tv_sec, 0);
            }
            for expected in [libc::EACCES, libc::ESTALE] {
                output.st_ex_mtime.tv_sec = 99;
                assert_eq!(
                    callbacks.fstat_fn.unwrap()(&mut host, &mut fsp, &mut output),
                    -1
                );
                assert_eq!(*libc::__errno_location(), expected);
                assert_eq!(output.st_ex_mtime.tv_sec, 99);
            }
            drop(Box::from_raw(host.data.cast::<Context>()));
            host.data = std::ptr::null_mut();
        }
        worker.join().unwrap();
        std::fs::remove_file(socket).unwrap();
        if writable {
            std::fs::remove_file(work).unwrap();
        }
    }
}

#[test]
fn mtime_sdk_conversion_handles_full_utc_range_and_rejects_kind_mismatches() {
    for seconds in [0, 2_147_483_648, SavedMtime::MAX_UNIX_SECONDS] {
        let mut stat: sdk::SMB_STRUCT_STAT = Default::default();
        // SAFETY: stat is live exclusive SDK output storage.
        unsafe {
            fill_stat_fields(
                NamespaceEntryKind::File,
                "file",
                3,
                saved(seconds),
                &mut stat,
            )
            .unwrap();
        }
        assert_eq!(u64::try_from(stat.st_ex_mtime.tv_sec).unwrap(), seconds);
        assert_eq!(stat.st_ex_mtime.tv_nsec, 0);
        // SAFETY: the real SDK conversion borrows this live generated timespec.
        let encoded = unsafe { sdk::full_timespec_to_nt_time(&stat.st_ex_mtime) };
        assert_eq!(encoded, (seconds + 11_644_473_600) * 10_000_000);
    }
    let mut output: sdk::SMB_STRUCT_STAT = Default::default();
    // SAFETY: output is live exclusive SDK storage; invalid pairs fail before writing it.
    unsafe {
        fill_stat_fields(
            NamespaceEntryKind::File,
            "new",
            3,
            FileMtime::Unpublished {},
            &mut output,
        )
        .unwrap();
        assert_eq!(output.st_ex_mtime.tv_nsec, i64::from(sdk::SAMBA_UTIME_OMIT));
        assert_eq!(
            sdk::full_timespec_to_nt_time(&output.st_ex_mtime),
            u64::from(sdk::NTTIME_OMIT)
        );
        assert!(
            fill_stat_fields(
                NamespaceEntryKind::File,
                "file",
                3,
                FileMtime::NotFile {},
                &mut output
            )
            .is_err()
        );
        assert!(
            fill_stat_fields(
                NamespaceEntryKind::Directory,
                "dir",
                0,
                saved(1),
                &mut output
            )
            .is_err()
        );
    }
}
