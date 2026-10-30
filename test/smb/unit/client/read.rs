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
            let Request::Read { handle: 7, offset, length } = request else { panic!("unexpected read") };
            assert_eq!(offset, position as u64);
            assert!(length as usize <= MAX_READ_BYTES);
            let count = (length as usize).min(source.len() - position).min(12345);
            stream.send(&Response::Ok { result: json_codec::json!({"bytes": &source[position..position + count]}) }, None).unwrap();
            position += count;
            if count == 0 { break; }
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
            assert!(matches!(stream.receive::<Request>().unwrap().0, Request::Read { .. }));
            let response = if malformed {
                Response::Ok { result: json_codec::json!({"bytes":[1,2]}) }
            } else {
                Response::Error { code: storage_contract::ErrorCode::ContentUnavailable, message: "late read failed".into(), retryable: false, operation_id: None }
            };
            stream.send(&response, None).unwrap();
            if malformed {
                assert_eq!(stream.receive::<Request>().unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
            } else {
                assert!(matches!(stream.receive::<Request>().unwrap().0, Request::Stat { .. }));
                stream.send(&Response::Ok { result: Value::Bool(true) }, None).unwrap();
            }
        });
        let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
        let clone = client.clone();
        let error = client.read(7, 0, &mut [0]).unwrap_err();
        if malformed {
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(clone.call(Request::Stat { path: "file".into() }).unwrap_err().kind(), io::ErrorKind::NotConnected);
        } else {
            assert_eq!(error_errno(&error), libc::EIO);
            assert_eq!(clone.call(Request::Stat { path: "file".into() }).unwrap(), true);
        }
        server.join().unwrap();
    }
}
