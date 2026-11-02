use super::IoResultExt;
use std::{error::Error, io};

#[test]
fn path_context_preserves_original_io_error() {
    let original = io::Error::from_raw_os_error(13);
    let original_kind = original.kind();
    let result: io::Result<()> = Err(original);
    let contextual = result.with_err_path(|| "temporary-file").unwrap_err();

    assert_eq!(contextual.kind(), original_kind);
    assert!(contextual.to_string().contains("temporary-file"));
    let original = contextual
        .source()
        .and_then(|source| source.downcast_ref::<io::Error>())
        .expect("path context retains the original I/O error");
    assert_eq!(original.raw_os_error(), Some(13));
}

#[test]
fn nested_path_contexts_preserve_error_chain() {
    let result: io::Result<()> = Err(io::Error::from_raw_os_error(13));
    let contextual = result
        .with_err_path(|| "payload")
        .with_err_path(|| "temporary-directory")
        .unwrap_err();

    let first = contextual
        .source()
        .and_then(|source| source.downcast_ref::<io::Error>())
        .expect("outer path context retains the inner I/O error");
    assert!(first.to_string().contains("payload"));
    let original = first
        .source()
        .and_then(|source| source.downcast_ref::<io::Error>())
        .expect("inner path context retains the original I/O error");
    assert_eq!(original.raw_os_error(), Some(13));
}
