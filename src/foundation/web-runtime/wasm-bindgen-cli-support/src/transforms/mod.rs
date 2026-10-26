pub mod catch_handler;
pub mod export_sp_restore;
pub mod externref;
pub mod jspi;
pub mod multi_value;
pub mod threads;

use walrus::ir::{Try, TryTable, Visitor};

/// Returns true if the module has local exception tags (indicating exception handling support).
pub fn has_local_exception_tags(module: &walrus::Module) -> bool {
    module
        .tags
        .iter()
        .any(|t| matches!(t.kind, walrus::TagKind::Local))
}

/// The version of exception handling used in the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionHandlingVersion {
    /// No exception handling instructions found
    None,
    /// Legacy EH (phase 1): try/catch/catch_all instructions
    Legacy,
    /// Modern EH (phase 4): try_table instruction
    Modern,
    ModernButWithPanicAbort,
}

impl ExceptionHandlingVersion {
    /// Whether a Rust panic can unwind out of an export. `Modern` and
    /// `ModernButWithPanicAbort` share a dialect but not this answer, so ask
    /// here rather than matching on the dialect at the call site.
    pub fn unwinds(self) -> bool {
        matches!(self, Self::Legacy | Self::Modern)
    }
}

/// Detect which exception handling version is used in the module.
///
/// Scans all functions for `Try` (legacy) vs `TryTable` (modern) instructions.
/// If both are present, returns `Modern` as the module likely supports both.
pub fn detect_exception_handling_version(
    module: &walrus::Module,
    enable_abort_handler: bool,
) -> ExceptionHandlingVersion {
    struct EhDetector {
        has_try: bool,
        has_try_table: bool,
    }

    impl<'instr> Visitor<'instr> for EhDetector {
        fn visit_try(&mut self, _: &Try) {
            self.has_try = true;
        }

        fn visit_try_table(&mut self, _: &TryTable) {
            self.has_try_table = true;
        }
    }

    let mut detector = EhDetector {
        has_try: false,
        has_try_table: false,
    };

    for func in module.funcs.iter() {
        if let walrus::FunctionKind::Local(local) = &func.kind {
            walrus::ir::dfs_in_order(&mut detector, local, local.entry_block());
        }
        // Early exit if we've found both
        if detector.has_try && detector.has_try_table {
            break;
        }
    }

    match (
        detector.has_try_table,
        detector.has_try,
        enable_abort_handler,
    ) {
        (true, _, _) => ExceptionHandlingVersion::Modern,
        (false, true, _) => ExceptionHandlingVersion::Legacy,
        (false, false, true) => ExceptionHandlingVersion::ModernButWithPanicAbort,
        (false, false, false) => ExceptionHandlingVersion::None,
    }
}

/// If a start function is present, it removes it from the `start` section
/// of the Wasm module and then moves it to an exported function, named
/// `__wbindgen_start`.
pub(crate) fn unstart_start_function(module: &mut walrus::Module) -> bool {
    let start = match module.start.take() {
        Some(id) => id,
        None => return false,
    };
    module.exports.add("__wbindgen_start", start);
    true
}

#[cfg(test)]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../test/foundation/wasm-bindgen-cli-support/unit/src/transforms/mod/tests_110.rs"
));
