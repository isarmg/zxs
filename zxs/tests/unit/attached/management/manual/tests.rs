use super::*;
#[test]
fn actual_stop_is_consulted_even_before_owner_mirrors_it(){
    let original=AtomicBool::new(false);
    let owner_mirror=AtomicBool::new(false);
    let session=AtomicBool::new(true);
    let expires=Instant::now()+std::time::Duration::from_secs(60);
    let stop=||original.load(Ordering::Acquire);
    assert!(live(&stop,&owner_mirror,&session,expires).is_ok());
    original.store(true,Ordering::Release);
    assert!(!owner_mirror.load(Ordering::Acquire));
    assert_eq!(live(&stop,&owner_mirror,&session,expires).unwrap_err().kind(),io::ErrorKind::PermissionDenied);
    assert!(owner_mirror.load(Ordering::Acquire));
}
