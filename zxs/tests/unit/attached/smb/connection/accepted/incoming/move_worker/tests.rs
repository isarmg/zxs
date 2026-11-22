use super::*;
#[test]
fn placement_and_original_cancel_are_single_winner(){
    let cancelled=Gate::new();
    assert_eq!(cancelled.cancel(),ControlTurn::CancelRequested);
    assert_eq!(cancelled.cancel(),ControlTurn::CancelRequested);
    assert!(cancelled.select().is_err());
    assert!(cancelled.require(SELECTING).is_err());
    assert_eq!(cancelled.finish(),CANCELLED);

    let deciding=Gate::new();
    assert!(deciding.require(SELECTING).is_err());
    deciding.select().unwrap();
    assert!(deciding.require(WORKING).is_err());
    assert!(deciding.require(SELECTING).is_ok());
    assert!(deciding.select().is_err());
    assert_eq!(deciding.cancel(),ControlTurn::TooLate);
    deciding.abandon();
    assert_eq!(deciding.finish(),SELECTING);

    let lost=Gate::new();
    lost.abandon();
    assert!(lost.select().is_err());
    assert_eq!(lost.cancel(),ControlTurn::TooLate);
    assert_eq!(lost.finish(),LOST);

    let finished=Gate::new();
    assert_eq!(finished.finish(),WORKING);
    assert_eq!(finished.cancel(),ControlTurn::TooLate);
}
#[test]
fn uncertain_existing_disk_state_is_not_a_cancellation_result(){
    use io::ErrorKind::{ConnectionAborted,Interrupted};
    assert!(matches!(WorkerOutcome::from_core(CANCELLED,Err(Interrupted.into())),WorkerOutcome::CancelledBeforeSelection));
    assert!(matches!(WorkerOutcome::from_core(CANCELLED,Err(ConnectionAborted.into())),WorkerOutcome::Unknown));
    assert!(matches!(WorkerOutcome::from_core(SELECTING,Err(Interrupted.into())),WorkerOutcome::Unknown));
}
#[test]
fn private_completion_hint_is_drained_between_reports(){
    use crate::smb::readiness::{self,Interest,Watch};
    let (wake,post)=CompletionWake::pair().unwrap();
    let mut ready=[Watch::new(wake.0.as_fd(),Interest::Read)];
    assert_eq!(readiness::wait(&mut ready,0).unwrap(),0);
    post.notify();
    assert_eq!(readiness::wait(&mut ready,0).unwrap(),1);
    assert!(ready[0].events().readable());
    wake.clear();
    assert_eq!(readiness::wait(&mut ready,0).unwrap(),0);
    post.notify();
    assert_eq!(readiness::wait(&mut ready,0).unwrap(),1);
}
