use super::*;
#[test]
fn original_peer_probe_does_not_guess_on_missing_owner_or_loss(){
    let fixture=DirectStream::Fixture(Box::new(io::Cursor::new(Vec::<u8>::new())));
    assert!(matches!(fixture.original_peer_probe(),Err(Fault::Transport)));
    assert_eq!(OriginalPeerProbe(Weak::new()).observe(),Err(Fault::Closed));
    assert_eq!(pending_events(Events::NONE),Ok(()));
    assert_eq!(pending_events(Events::READABLE),Ok(()));
    assert_eq!(pending_events(Events::READABLE.with(Events::REMOTE_END)),Err(Fault::Closed));
    assert_eq!(pending_events(Events::HUNG_UP),Err(Fault::Closed));
    assert_eq!(pending_events(Events::KERNEL_ERROR.with(Events::REMOTE_END)),Err(Fault::Transport));
    assert_eq!(pending_events(Events::UNAVAILABLE),Err(Fault::Transport));
}
#[test]
fn resuming_keeps_the_partial_record_and_original_deadline(){
    let a=[0,0,0,2,11,12];
    let b=[0,0,0,2,21,22];
    let mut stream=DirectStream::Fixture(Box::new(io::Cursor::new([&a[2..],&b].concat())));
    let mut input=PendingRead{prefix:[a[0],a[1],0,0],prefix_used:2,end:Some(deadline().unwrap()),
        ..PendingRead::default()};
    assert_eq!(input.finish_pending(&mut stream,256).unwrap().unwrap(),a);
    assert!(!input.has_partial());
    assert_eq!(stream.read_record(256,deadline().unwrap()).unwrap(),b);

    let mut stream=DirectStream::Fixture(Box::new(io::Cursor::new(a)));
    let mut input=PendingRead::default();
    assert_eq!(input.advance(&mut stream,256).unwrap(),None);
    assert!(input.has_partial());
    assert_eq!(input.finish_pending(&mut stream,256).unwrap().unwrap(),a);
    input.end=Instant::now().checked_sub(std::time::Duration::from_secs(1));
    assert_eq!(input.finish_pending(&mut stream,256),Err(Fault::Transport));
}
