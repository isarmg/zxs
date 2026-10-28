use super::*;

#[derive(Debug, PartialEq)]
enum Sent {
    Prepare { end: u64, bytes: u64 },
    Dirty,
    Flush,
}

fn record(sent: &mut Vec<Sent>, request: Request) -> io::Result<Value> {
    sent.push(match request {
        Request::PrepareWrite { handle, end, bytes } => {
            assert_eq!(handle, 7);
            Sent::Prepare { end, bytes }
        }
        Request::Dirty { handle } => {
            assert_eq!(handle, 7);
            Sent::Dirty
        }
        Request::Flush { handle } => {
            assert_eq!(handle, 7);
            Sent::Flush
        }
        other => panic!("write sequencing sent an unrelated request: {other:?}"),
    });
    Ok(json_codec::json!({}))
}

fn change(sequence: &mut WriteSequence, sent: &mut Vec<Sent>, end: u64) -> io::Result<u64> {
    sequence.mutate(
        7,
        end,
        4096,
        |request| record(sent, request),
        || Ok((end, Mutation::Changed)),
    )
}

#[test]
fn only_the_first_change_of_a_generation_is_announced_and_flush_reopens_it() {
    let mut sequence = WriteSequence::new(false);
    let mut sent = Vec::new();
    for end in [4096, 8192, 12288] {
        assert_eq!(change(&mut sequence, &mut sent, end).unwrap(), end);
    }
    sequence
        .flush(7, |request| record(&mut sent, request))
        .unwrap();
    change(&mut sequence, &mut sent, 16384).unwrap();
    change(&mut sequence, &mut sent, 20480).unwrap();
    assert_eq!(
        sent,
        [
            Sent::Prepare {
                end: 4096,
                bytes: 4096
            },
            Sent::Dirty,
            Sent::Prepare {
                end: 8192,
                bytes: 4096
            },
            Sent::Prepare {
                end: 12288,
                bytes: 4096
            },
            Sent::Flush,
            Sent::Prepare {
                end: 16384,
                bytes: 4096
            },
            Sent::Dirty,
            Sent::Prepare {
                end: 20480,
                bytes: 4096
            },
        ]
    );
}

#[test]
fn failed_flush_still_ends_the_announced_generation() {
    let mut sequence = WriteSequence::new(false);
    let mut sent = Vec::new();
    change(&mut sequence, &mut sent, 4096).unwrap();
    let error = sequence
        .flush(7, |request| {
            record(&mut sent, request)?;
            Err(io::Error::from_raw_os_error(libc::EIO))
        })
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EIO));
    change(&mut sequence, &mut sent, 8192).unwrap();
    assert_eq!(
        sent.iter().filter(|sent| **sent == Sent::Dirty).count(),
        2,
        "a failed Flush may already have published; the next change must reopen"
    );
}

#[test]
fn failed_or_unknown_dirty_is_announced_again_on_the_next_change() {
    let mut sequence = WriteSequence::new(false);
    let mut sent = Vec::new();
    let error = sequence
        .mutate(
            7,
            4096,
            4096,
            |request| {
                let dirty = matches!(request, Request::Dirty { .. });
                record(&mut sent, request)?;
                if dirty {
                    return Err(io::Error::from(io::ErrorKind::ConnectionReset));
                }
                Ok(json_codec::json!({}))
            },
            || Ok(((), Mutation::Changed)),
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    change(&mut sequence, &mut sent, 8192).unwrap();
    change(&mut sequence, &mut sent, 12288).unwrap();
    assert_eq!(sent.iter().filter(|sent| **sent == Sent::Dirty).count(), 2);
    assert_eq!(
        sent.last(),
        Some(&Sent::Prepare {
            end: 12288,
            bytes: 4096
        })
    );
}

#[test]
fn write_through_announces_every_change() {
    let mut sequence = WriteSequence::new(true);
    let mut sent = Vec::new();
    for end in [4096, 8192, 12288] {
        change(&mut sequence, &mut sent, end).unwrap();
    }
    assert_eq!(sent.iter().filter(|sent| **sent == Sent::Dirty).count(), 3);
    assert_eq!(sent.len(), 6);
}

#[test]
fn unchanged_or_failed_mutations_do_not_open_a_generation() {
    let mut sequence = WriteSequence::new(false);
    let mut sent = Vec::new();
    sequence
        .mutate(
            7,
            0,
            0,
            |request| record(&mut sent, request),
            || Ok(((), Mutation::Unchanged)),
        )
        .unwrap();
    let error = sequence
        .mutate(
            7,
            4096,
            4096,
            |request| record(&mut sent, request),
            || Err::<((), Mutation), _>(io::Error::from_raw_os_error(libc::ENOSPC)),
        )
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ENOSPC));
    assert!(!sent.contains(&Sent::Dirty));
    change(&mut sequence, &mut sent, 4096).unwrap();
    assert_eq!(sent.iter().filter(|sent| **sent == Sent::Dirty).count(), 1);
}

#[test]
fn rejected_admission_never_runs_the_mutation() {
    let mut sequence = WriteSequence::new(false);
    let mut mutated = false;
    let error = sequence
        .mutate(
            7,
            4096,
            4096,
            |_| Err(io::Error::from_raw_os_error(libc::EDQUOT)),
            || {
                mutated = true;
                Ok(((), Mutation::Changed))
            },
        )
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EDQUOT));
    assert!(!mutated);
}

#[test]
fn lost_dirty_or_flush_reply_disconnects_clones_before_any_further_descriptor_change() {
    use std::io::{Read, Write};
    use std::time::Duration;

    for lose_flush in [false, true] {
        let (one, two) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = FrameStream::with_timeout(two, Duration::from_secs(2)).unwrap();
            assert!(matches!(
                stream.receive::<Request>().unwrap().0,
                Request::PrepareWrite { handle: 7, .. }
            ));
            stream
                .send(
                    &Response::Ok {
                        result: Value::Null,
                    },
                    None,
                )
                .unwrap();
            assert!(matches!(
                stream.receive::<Request>().unwrap().0,
                Request::Dirty { handle: 7 }
            ));
            if lose_flush {
                stream
                    .send(
                        &Response::Ok {
                            result: Value::Null,
                        },
                        None,
                    )
                    .unwrap();
                assert!(matches!(
                    stream.receive::<Request>().unwrap().0,
                    Request::PrepareWrite { handle: 7, .. }
                ));
                stream
                    .send(
                        &Response::Ok {
                            result: Value::Null,
                        },
                        None,
                    )
                    .unwrap();
                // The second change is in the same announced generation: no Dirty.
                assert!(matches!(
                    stream.receive::<Request>().unwrap().0,
                    Request::Flush { handle: 7 }
                ));
            }
            // Consume the real request, then close without its response frame.
        });
        let client = Client(Arc::new(Mutex::new(Some(
            FrameStream::with_timeout(one, Duration::from_secs(2)).unwrap(),
        ))));
        let clone = client.clone();
        let mut sequence = WriteSequence::new(false);
        // Actual descriptor I/O makes a later accidental mutation observable.
        let (mut descriptor, mut reader) = UnixStream::pair().unwrap();
        let first = sequence.mutate(
            7,
            3,
            3,
            |request| client.call(request),
            || {
                descriptor.write_all(b"one")?;
                Ok(((), Mutation::Changed))
            },
        );
        let error = if lose_flush {
            first.unwrap();
            sequence
                .mutate(
                    7,
                    6,
                    3,
                    |request| clone.call(request),
                    || {
                        descriptor.write_all(b"two")?;
                        Ok(((), Mutation::Changed))
                    },
                )
                .unwrap();
            sequence
                .flush(7, |request| client.call(request))
                .unwrap_err()
        } else {
            first.unwrap_err()
        };
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        for caller in [&client, &clone] {
            let mut mutated = false;
            let error = sequence
                .mutate(
                    7,
                    9,
                    3,
                    |request| caller.call(request),
                    || {
                        mutated = true;
                        descriptor.write_all(b"bad")?;
                        Ok(((), Mutation::Changed))
                    },
                )
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::NotConnected);
            assert!(!mutated);
        }
        server.join().unwrap();
        drop(descriptor);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(
            bytes.as_slice(),
            if lose_flush {
                b"onetwo".as_slice()
            } else {
                b"one"
            }
        );
    }
}
