use super::*;
#[test]
fn parked_ordinary_buffers_have_fixed_count_and_fifo(){
    fn record(n:u8)->CapturedRecord{CapturedRecord{origin:super::super::Weak::new(),bytes:vec![n]}}
    let mut control=MoveControl::default();
    for n in 0..BACKLOG_RECORDS{control.remember(record(n as u8)).unwrap();}
    assert_eq!(control.remember(record(77)),Err(Fault::Resources));
    assert_eq!(control.deferred_bytes,BACKLOG_RECORDS);
    for n in 0..BACKLOG_RECORDS{assert_eq!(control.oldest().unwrap().bytes,[n as u8]);}
    assert!(control.empty());
    let wide=CapturedRecord{origin:super::super::Weak::new(),bytes:vec![0;BACKLOG_BYTES+1]};
    assert_eq!(control.remember(wide),Err(Fault::Resources));
    assert!(control.empty());
}
