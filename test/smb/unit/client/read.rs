use super::*;

#[test]
fn bounded_reads_continue_after_chunk_boundaries_and_stop_at_eof() {
    let (one, two) = UnixStream::pair().unwrap();
    let expected: Vec<_> = (0..MAX_READ_BYTES + 17).map(|n| (n % 251) as u8).collect();
    let source = expected.clone();
    let server = std::thread::spawn(move || {
        let mut stream = FrameStream::new(two).unwrap();
        let mut position = 0;
        loop {
            let (request, fd) = stream.receive::<Request>().unwrap();
            assert!(fd.is_none());
            let Request::Read {
                handle: 7,
                offset,
                length,
            } = request
            else {
                panic!("unexpected read")
            };
            assert_eq!(offset, position as u64);
            assert!(length as usize <= MAX_READ_BYTES);
            let count = (length as usize).min(source.len() - position).min(12345);
            stream
                .send(
                    &Response::Ok {
                        result: json_codec::json!({"bytes": &source[position..position + count]}),
                    },
                    None,
                )
                .unwrap();
            position += count;
            if count == 0 {
                break;
            }
        }
    });
    let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
    let mut output = vec![0; expected.len() + 10];
    assert_eq!(client.read(7, 0, &mut output).unwrap(), expected.len());
    assert_eq!(&output[..expected.len()], &expected);
    server.join().unwrap();
}

#[test]
fn malformed_read_disconnects_every_clone_and_business_failure_does_not() {
    for malformed in [true, false] {
        let (one, two) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = FrameStream::new(two).unwrap();
            assert!(matches!(
                stream.receive::<Request>().unwrap().0,
                Request::Read { .. }
            ));
            let response = if malformed {
                Response::Ok {
                    result: json_codec::json!({"bytes":[1,2]}),
                }
            } else {
                Response::Error {
                    code: storage_contract::ErrorCode::ContentUnavailable,
                    message: "late read failed".into(),
                    retryable: false,
                    operation_id: None,
                }
            };
            stream.send(&response, None).unwrap();
            if malformed {
                assert_eq!(
                    stream.receive::<Request>().unwrap_err().kind(),
                    io::ErrorKind::UnexpectedEof
                );
            } else {
                assert!(matches!(
                    stream.receive::<Request>().unwrap().0,
                    Request::Stat { .. }
                ));
                stream
                    .send(
                        &Response::Ok {
                            result: Value::Bool(true),
                        },
                        None,
                    )
                    .unwrap();
            }
        });
        let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
        let clone = client.clone();
        let error = client.read(7, 0, &mut [0]).unwrap_err();
        if malformed {
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(
                clone
                    .call(Request::Stat {
                        path: "file".into()
                    })
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::NotConnected
            );
        } else {
            assert_eq!(error_errno(&error), libc::EIO);
            assert_eq!(
                clone
                    .call(Request::Stat {
                        path: "file".into()
                    })
                    .unwrap(),
                true
            );
        }
        server.join().unwrap();
    }
}

fn old_read_error(payload: &[u8], with_fd: bool) -> io::Error {
    let result = (|| {
        let response: Response =
            storage_contract::decode_strict(payload).map_err(io::Error::other)?;
        if with_fd {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RPC response descriptor does not match the request",
            ));
        }
        let Response::Ok { result } = response else {
            panic!("test requires an invalid read or descriptor")
        };
        let result: ReadResult = json_codec::from_value(result)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if result.bytes.len() > 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RPC read exceeds requested extent",
            ));
        }
        Ok(())
    })();
    result.unwrap_err()
}

#[test]
fn read_admission_preserves_error_priority_and_releases_descriptors_and_all_clones() {
    use std::{io::Read, os::fd::AsFd, time::Duration};
    for payload in [
        br#"{"status":"ok","result":{"bytes":[],"b\u0079tes":1e400}}"#.as_slice(),
        br#"{"status":"ok","result":{"bytes":[],"\ud800":0}}"#,
        br#"{"status":"ok","result":{"bytes":[]}} trailing"#,
        br#"{"status":"ok","result":{"bytes":[]},"unknown":0}"#,
        br#"{"status":"error","code":"FILE_WRITE_BUSY","message":"busy","retryable":true,"operation_id":0}"#,
        br#"{"status":"ok","result":{"bytes":[256],"a":0}}"#,
        br#"{"status":"ok","result":{"a":0,"bytes":[256]}}"#,
        br#"{"status":"ok","result":{"bytes":[256],"z":0}}"#,
        br#"{"status":"ok","result":{"z":0,"bytes":[256]}}"#,
        br#"{"status":"ok","result":{"bytes":[1,2]}}"#,
        br#"{"status":"ok","result":{"bytes":[1]}}"#,
        br#"{"status":"error","code":"FILE_WRITE_BUSY","message":"busy","retryable":true,"operation_id":"op_read"}"#,
    ] {
        for with_fd in [false, true] {
            if !with_fd && (payload == br#"{"status":"ok","result":{"bytes":[1]}}"# || payload.ends_with(br#""operation_id":"op_read"}"#)) { continue; }
            let expected = super::super::response::tests::signature(old_read_error(payload,with_fd));
            let (one,two)=UnixStream::pair().unwrap();
            let (descriptor,mut peer)=UnixStream::pair().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let server=std::thread::spawn(move || {
                let mut stream=FrameStream::with_timeout(two,Duration::from_secs(2)).unwrap();
                assert!(matches!(stream.receive::<Request>().unwrap().0,Request::Read{..}));
                stream.send_serialized(payload,with_fd.then(||descriptor.as_fd())).unwrap();
                drop(descriptor);
                assert_eq!(stream.receive::<Request>().unwrap_err().kind(),io::ErrorKind::UnexpectedEof);
            });
            let client=Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
            let clones=[client.clone(),client.clone()];
            let error=client.read(7,0,&mut [0]).unwrap_err();
            assert_eq!(super::super::response::tests::signature(error),expected,"payload={payload:?}; fd={with_fd}");
            for caller in [&client,&clones[0],&clones[1]] {
                assert_eq!(caller.read(7,0,&mut [0]).unwrap_err().kind(),io::ErrorKind::NotConnected);
                assert_eq!(caller.call(Request::Stat{path:"next".into()}).unwrap_err().kind(),io::ErrorKind::NotConnected);
            }
            server.join().unwrap();
            if with_fd {assert_eq!(peer.read(&mut [0]).unwrap(),0,"Read leaked a received descriptor");}
        }
    }
}

#[test]
fn read_business_error_keeps_operation_and_allows_the_next_read() {
    let (one, two) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = FrameStream::new(two).unwrap();
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Read { .. }
        ));
        stream
            .send(
                &Response::Error {
                    code: ErrorCode::FileWriteBusy,
                    message: "busy".into(),
                    retryable: true,
                    operation_id: Some("op_read".into()),
                },
                None,
            )
            .unwrap();
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Read { .. }
        ));
        stream
            .send(
                &Response::Ok {
                    result: json_codec::json!({"bytes":[7]}),
                },
                None,
            )
            .unwrap();
    });
    let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
    let clone = client.clone();
    let error = client.read(7, 0, &mut [0]).unwrap_err();
    assert_eq!(error_errno(&error), libc::EBUSY);
    let remote = error
        .get_ref()
        .unwrap()
        .downcast_ref::<RemoteError>()
        .unwrap();
    assert_eq!(
        remote.code,
        RemoteErrorCode::Server(ErrorCode::FileWriteBusy)
    );
    assert_eq!(remote.message, "busy");
    assert_eq!(remote.operation_id.as_deref(), Some("op_read"));
    assert!(remote.retryable);
    assert!(remote.source.is_none());
    assert_eq!(
        remote.to_string(),
        "FILE_WRITE_BUSY: busy; operation_id=op_read"
    );
    let mut output = [0];
    assert_eq!(clone.read(7, 0, &mut output).unwrap(), 1);
    assert_eq!(output, [7]);
    server.join().unwrap();
}

#[test]
fn failed_read_does_not_consume_a_queued_success_on_another_clone() {
    use std::io::Write;
    for invalid in [
        br#"{"status":"ok","result":{"bytes":[],"bytes":[1]}}"#.as_slice(),
        br#"{"status":"other","result":{"bytes":[1]}}"#,
        br#"{"status":"ok","result":{"bytes":[256]}}"#,
    ] {
        let (one, mut two) = UnixStream::pair().unwrap();
        let stale = br#"{"status":"ok","result":{"bytes":[9]}}"#;
        let mut frames = Vec::new();
        for payload in [invalid, stale] {
            frames.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            frames.extend_from_slice(payload);
        }
        two.write_all(&frames).unwrap();
        let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
        let clone = client.clone();
        let mut output = [77];
        assert!(client.read(7, 0, &mut output).is_err());
        assert_eq!(output, [77]);
        assert_eq!(
            clone.read(7, 0, &mut output).unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
        assert_eq!(output, [77]);
    }
}

#[test]
fn failed_later_read_retains_only_the_verified_prefix_and_returns_an_error() {
    let (one, two) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = FrameStream::new(two).unwrap();
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Read {
                offset: 0,
                length: 3,
                ..
            }
        ));
        stream
            .send(
                &Response::Ok {
                    result: json_codec::json!({"bytes":[7]}),
                },
                None,
            )
            .unwrap();
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Read {
                offset: 1,
                length: 2,
                ..
            }
        ));
        stream
            .send_serialized(br#"{"status":"ok","result":{"bytes":[256]}}"#, None)
            .unwrap();
    });
    let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
    let clone = client.clone();
    let mut output = [77; 3];
    assert_eq!(
        client.read(7, 0, &mut output).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(output, [7, 77, 77]);
    assert_eq!(
        clone.read(7, 0, &mut output).unwrap_err().kind(),
        io::ErrorKind::NotConnected
    );
    server.join().unwrap();
}
