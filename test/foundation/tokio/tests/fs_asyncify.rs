#[test]
fn filesystem_task_cancellation_retains_join_error() {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let handle = runtime.handle().clone();
    drop(runtime);

    let error = handle.block_on(tokio::fs::metadata(".")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    let source = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<tokio::task::JoinError>())
        .expect("filesystem cancellation must retain its JoinError");
    assert!(source.is_cancelled());
}
