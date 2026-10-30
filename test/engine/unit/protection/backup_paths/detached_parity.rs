use super::*;
use std::cell::Cell;

pub(super) fn check(
    engine: &Engine,
    repository: &str,
    path: &str,
    expected: &storage_contract::BackupConfigurationSnapshot,
) {
    let observed = Cell::new(0usize);
    let observe = || {
        observed.set(observed.get() + 1);
        Ok(())
    };
    let view = engine.backup_configuration_read_view();
    let actual = view.read(repository, path, &observe).unwrap();
    assert_eq!(&actual, expected);
    assert!(observed.get() > 0);
}
