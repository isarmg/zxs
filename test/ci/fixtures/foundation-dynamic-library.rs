#[test]
fn dynamic_library_symbol_borrow_calls_real_c_abi() {
    let library = libloading::os::unix::Library::this();
    // SAFETY: strlen is the C runtime function with this ABI. The symbol remains
    // borrowed from the live process library and the input is NUL-terminated.
    let length = unsafe {
        let symbol = library
            .get::<unsafe extern "C" fn(*const std::ffi::c_char) -> usize>(b"strlen\0")
            .unwrap();
        symbol(c"retained-receipt".as_ptr())
    };
    assert_eq!(length, 16);
}
#[test]
fn dynamic_library_reports_missing_symbol_without_calling_it() {
    let library = libloading::os::unix::Library::this();
    // SAFETY: lookup alone does not dereference the function pointer; no symbol
    // is invoked and the requested ABI would be used only after successful lookup.
    let result = unsafe {
        library.get::<unsafe extern "C" fn()>(b"zxs_missing_symbol_coverage_boundary_20261031\0")
    };
    assert!(result.is_err());
}
