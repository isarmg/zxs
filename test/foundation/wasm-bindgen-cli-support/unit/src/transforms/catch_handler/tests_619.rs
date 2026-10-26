#[cfg(test)]
mod tests {
    use super::*;
    use walrus::ModuleConfig;

    fn parse_wat(wat: &str) -> walrus::Module {
        let wasm = crate::transforms::parse_wat_text(wat, None).unwrap();
        ModuleConfig::new()
            .generate_producers_section(false)
            .parse(&wasm)
            .unwrap()
    }

    /// Common WAT fragments needed by all wrapper tests.
    const TERMINATED_WAT: &str = r#"
                (memory 1)
                (global $__instance_terminated i32 (i32.const 1048576))
                (export "__instance_terminated" (global $__instance_terminated))
    "#;

    const TERMINATED_ADDR: i32 = 1048576;

    fn get_test_memory(module: &walrus::Module) -> MemoryId {
        module.memories.iter().next().unwrap().id()
    }

    #[test]
    fn test_import_js_tag() {
        let wat = r#"
            (module
                (func $foo)
                (export "foo" (func $foo))
            )
        "#;
        let mut module = parse_wat(wat);

        // Should have no tags initially
        assert_eq!(module.tags.iter().count(), 0);

        // Import the JS tag
        let tag_id = import_js_tag(&mut module);

        // Should now have one tag
        assert_eq!(module.tags.iter().count(), 1);

        // The tag should be an import
        let tag = module.tags.get(tag_id);
        assert!(matches!(tag.kind, walrus::TagKind::Import(_)));

        // Check the import exists
        let import = module.imports.iter().find(|i| i.name == "__wbindgen_jstag");
        assert!(import.is_some());
        let import = import.unwrap();
        assert_eq!(import.module, crate::PLACEHOLDER_MODULE);
    }

    #[test]
    fn test_generate_catch_wrapper_modern() {
        let wat = &format!(
            r#"
            (module
                ;; A simple imported function
                (import "env" "my_import" (func $my_import (param i32) (result i32)))

                ;; Externref table for storing caught exceptions
                (table $externrefs 128 externref)

                ;; Heap alloc function (returns index)
                (func $__externref_table_alloc (result i32)
                    i32.const 42
                )

                ;; Exception store function
                (func $exn_store (param i32))

                {TERMINATED_WAT}

                (export "my_import" (func $my_import))
                (export "__externref_table" (table $externrefs))
                (export "__externref_table_alloc" (func $__externref_table_alloc))
                (export "exn_store" (func $exn_store))
            )
        "#
        );
        let mut module = parse_wat(wat);

        // Get the import function
        let import_func = module
            .exports
            .iter()
            .find(|e| e.name == "my_import")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        // Get the table
        let table = module
            .exports
            .iter()
            .find(|e| e.name == "__externref_table")
            .and_then(|e| match e.item {
                walrus::ExportItem::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();

        // Get the helper functions
        let table_alloc = module
            .exports
            .iter()
            .find(|e| e.name == "__externref_table_alloc")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let exn_store = module
            .exports
            .iter()
            .find(|e| e.name == "exn_store")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        // Import JSTag
        let js_tag = import_js_tag(&mut module);

        // Count functions before
        let func_count_before = module.funcs.iter().count();

        // Generate the catch wrapper
        let memory = get_test_memory(&module);

        let wrapper_id = generate_catch_wrapper(
            &mut module,
            import_func,
            js_tag,
            WrapperKind::CatchWrapper,
            Some(table),
            Some(table_alloc),
            Some(exn_store),
            TERMINATED_ADDR,
            memory,
            super::super::ExceptionHandlingVersion::Modern,
        );

        // Should have created a new function
        assert_eq!(module.funcs.iter().count(), func_count_before + 1);

        // The wrapper should have a name
        let wrapper = module.funcs.get(wrapper_id);
        assert!(wrapper.name.as_ref().unwrap().contains("catch wrapper"));
    }

    #[test]
    fn test_generate_catch_wrapper_legacy() {
        let wat = &format!(
            r#"
            (module
                ;; A simple imported function that returns nothing
                (import "env" "my_void_import" (func $my_void_import))

                ;; Externref table
                (table $externrefs 128 externref)

                ;; Heap alloc function
                (func $__externref_table_alloc (result i32)
                    i32.const 42
                )

                ;; Exception store function
                (func $exn_store (param i32))

                {TERMINATED_WAT}

                (export "my_void_import" (func $my_void_import))
                (export "__externref_table" (table $externrefs))
                (export "__externref_table_alloc" (func $__externref_table_alloc))
                (export "exn_store" (func $exn_store))
            )
        "#
        );
        let mut module = parse_wat(wat);

        // Get the import function
        let import_func = module
            .exports
            .iter()
            .find(|e| e.name == "my_void_import")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        // Get the table
        let table = module
            .exports
            .iter()
            .find(|e| e.name == "__externref_table")
            .and_then(|e| match e.item {
                walrus::ExportItem::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();

        // Get the helper functions
        let table_alloc = module
            .exports
            .iter()
            .find(|e| e.name == "__externref_table_alloc")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let exn_store = module
            .exports
            .iter()
            .find(|e| e.name == "exn_store")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        // Import JSTag
        let js_tag = import_js_tag(&mut module);
        let memory = get_test_memory(&module);

        // Count functions before
        let func_count_before = module.funcs.iter().count();

        // Generate the catch wrapper using legacy EH
        let wrapper_id = generate_catch_wrapper(
            &mut module,
            import_func,
            js_tag,
            WrapperKind::CatchWrapper,
            Some(table),
            Some(table_alloc),
            Some(exn_store),
            TERMINATED_ADDR,
            memory,
            super::super::ExceptionHandlingVersion::Legacy,
        );

        // Should have created a new function
        assert_eq!(module.funcs.iter().count(), func_count_before + 1);

        // The wrapper should have a name
        let wrapper = module.funcs.get(wrapper_id);
        assert!(wrapper.name.as_ref().unwrap().contains("catch wrapper"));
    }

    #[test]
    fn test_run_with_imports_with_catch() {
        use crate::wit::{AdapterId, NonstandardWitSection, WasmBindgenAux};
        use std::collections::HashSet;

        // Create a module with an import, EH instructions, and required intrinsics
        let wat = &format!(
            r#"
            (module
                ;; Import that we want to wrap with catch
                (import "env" "might_throw" (func $might_throw (result i32)))

                ;; Externref table
                (table $externrefs 128 externref)

                ;; Heap alloc function
                (func $__externref_table_alloc (result i32)
                    i32.const 42
                )

                ;; Exception store function
                (func $exn_store (param i32))

                ;; A function that uses legacy EH (so we detect EH is available)
                (func $uses_eh
                    try
                        i32.const 1
                        drop
                    catch_all
                    end
                )

                ;; A function that calls the import
                (func $caller (result i32)
                    call $might_throw
                )

                {TERMINATED_WAT}

                (export "__externref_table_alloc" (func $__externref_table_alloc))
                (export "__wbindgen_exn_store" (func $exn_store))
                (export "caller" (func $caller))
            )
        "#
        );
        let mut module = parse_wat(wat);

        // Find the import
        let import_id = module
            .imports
            .iter()
            .find(|i| i.name == "might_throw")
            .map(|i| i.id())
            .unwrap();

        let import_func_id = match module.imports.get(import_id).kind {
            walrus::ImportKind::Function(f) => f,
            _ => panic!("expected function import"),
        };

        // Get the table
        let table = module.tables.iter().next().unwrap().id();

        // Get table_alloc and exn_store
        let table_alloc = module
            .exports
            .iter()
            .find(|e| e.name == "__externref_table_alloc")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let exn_store = module
            .exports
            .iter()
            .find(|e| e.name == "__wbindgen_exn_store")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        // Create aux with imports_with_catch
        let adapter_id = AdapterId(0);
        let mut imports_with_catch = HashSet::new();
        imports_with_catch.insert(adapter_id);

        let mut aux = WasmBindgenAux {
            imports_with_catch,
            externref_table: Some(table),
            externref_alloc: Some(table_alloc),
            exn_store: Some(exn_store),
            ..Default::default()
        };

        // Create wit section with implements mapping
        let mut wit = NonstandardWitSection::default();
        wit.implements.push((import_id, import_func_id, adapter_id));

        // Verify EH is detected
        let eh_version = super::super::detect_exception_handling_version(&module, false);
        assert_eq!(eh_version, super::super::ExceptionHandlingVersion::Legacy);

        // Run the transform
        run(&mut module, &mut aux, &wit, eh_version).unwrap();

        // JSTag should be set in aux
        assert!(aux.js_tag.is_some());

        // JSTag should be imported
        let jstag_import = module.imports.iter().find(|i| i.name == "__wbindgen_jstag");
        assert!(jstag_import.is_some());
    }

    #[test]
    fn test_wrapper_contains_try_instruction_legacy() {
        use walrus::ir::Visitor;

        let wat = &format!(
            r#"
            (module
                (import "env" "my_import" (func $my_import (param i32) (result i32)))
                (table $externrefs 128 externref)
                (func $__externref_table_alloc (result i32) i32.const 42)
                (func $exn_store (param i32))
                {TERMINATED_WAT}
                (export "my_import" (func $my_import))
                (export "__externref_table" (table $externrefs))
                (export "__externref_table_alloc" (func $__externref_table_alloc))
                (export "exn_store" (func $exn_store))
            )
        "#
        );
        let mut module = parse_wat(wat);

        let import_func = module
            .exports
            .iter()
            .find(|e| e.name == "my_import")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let table = module
            .exports
            .iter()
            .find(|e| e.name == "__externref_table")
            .and_then(|e| match e.item {
                walrus::ExportItem::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();

        let table_alloc = module
            .exports
            .iter()
            .find(|e| e.name == "__externref_table_alloc")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let exn_store = module
            .exports
            .iter()
            .find(|e| e.name == "exn_store")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let js_tag = import_js_tag(&mut module);
        let memory = get_test_memory(&module);

        let wrapper_id = generate_catch_wrapper(
            &mut module,
            import_func,
            js_tag,
            WrapperKind::CatchWrapper,
            Some(table),
            Some(table_alloc),
            Some(exn_store),
            TERMINATED_ADDR,
            memory,
            super::super::ExceptionHandlingVersion::Legacy,
        );

        // Verify the wrapper contains a Try instruction
        struct TryFinder {
            found_try: bool,
        }
        impl<'a> Visitor<'a> for TryFinder {
            fn visit_try(&mut self, _: &Try) {
                self.found_try = true;
            }
        }

        let wrapper = module.funcs.get(wrapper_id);
        if let walrus::FunctionKind::Local(local) = &wrapper.kind {
            let mut finder = TryFinder { found_try: false };
            walrus::ir::dfs_in_order(&mut finder, local, local.entry_block());
            assert!(finder.found_try, "wrapper should contain a Try instruction");
        } else {
            panic!("wrapper should be a local function");
        }
    }

    #[test]
    fn test_wrapper_contains_try_table_instruction_modern() {
        use walrus::ir::Visitor;

        let wat = &format!(
            r#"
            (module
                (import "env" "my_import" (func $my_import (param i32) (result i32)))
                (table $externrefs 128 externref)
                (func $__externref_table_alloc (result i32) i32.const 42)
                (func $exn_store (param i32))
                {TERMINATED_WAT}
                (export "my_import" (func $my_import))
                (export "__externref_table" (table $externrefs))
                (export "__externref_table_alloc" (func $__externref_table_alloc))
                (export "exn_store" (func $exn_store))
            )
        "#
        );
        let mut module = parse_wat(wat);

        let import_func = module
            .exports
            .iter()
            .find(|e| e.name == "my_import")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let table = module
            .exports
            .iter()
            .find(|e| e.name == "__externref_table")
            .and_then(|e| match e.item {
                walrus::ExportItem::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();

        let table_alloc = module
            .exports
            .iter()
            .find(|e| e.name == "__externref_table_alloc")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let exn_store = module
            .exports
            .iter()
            .find(|e| e.name == "exn_store")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let js_tag = import_js_tag(&mut module);
        let memory = get_test_memory(&module);

        let wrapper_id = generate_catch_wrapper(
            &mut module,
            import_func,
            js_tag,
            WrapperKind::CatchWrapper,
            Some(table),
            Some(table_alloc),
            Some(exn_store),
            TERMINATED_ADDR,
            memory,
            super::super::ExceptionHandlingVersion::Modern,
        );

        // Verify the wrapper contains a TryTable instruction
        struct TryTableFinder {
            found_try_table: bool,
        }
        impl<'a> Visitor<'a> for TryTableFinder {
            fn visit_try_table(&mut self, _: &TryTable) {
                self.found_try_table = true;
            }
        }

        let wrapper = module.funcs.get(wrapper_id);
        if let walrus::FunctionKind::Local(local) = &wrapper.kind {
            let mut finder = TryTableFinder {
                found_try_table: false,
            };
            walrus::ir::dfs_in_order(&mut finder, local, local.entry_block());
            assert!(
                finder.found_try_table,
                "wrapper should contain a TryTable instruction"
            );
        } else {
            panic!("wrapper should be a local function");
        }
    }

    /// Build a test module with an imported function, generate a catch wrapper,
    /// emit to WAT via wasmprinter, and return the full module text.
    fn build_wrapper_wat(eh_version: super::super::ExceptionHandlingVersion) -> String {
        let wat = &format!(
            r#"
            (module
                (import "env" "my_import" (func $my_import (param i32) (result i32)))
                (table $externrefs 128 externref)
                (func $__externref_table_alloc (result i32) i32.const 42)
                (func $exn_store (param i32))
                {TERMINATED_WAT}
                (export "my_import" (func $my_import))
                (export "__externref_table" (table $externrefs))
                (export "__externref_table_alloc" (func $__externref_table_alloc))
                (export "exn_store" (func $exn_store))
            )
        "#
        );
        let mut module = parse_wat(wat);

        let import_func = module
            .exports
            .iter()
            .find(|e| e.name == "my_import")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();
        let table = module
            .exports
            .iter()
            .find(|e| e.name == "__externref_table")
            .and_then(|e| match e.item {
                walrus::ExportItem::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();
        let table_alloc = module
            .exports
            .iter()
            .find(|e| e.name == "__externref_table_alloc")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();
        let exn_store = module
            .exports
            .iter()
            .find(|e| e.name == "exn_store")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let js_tag = import_js_tag(&mut module);
        let memory = get_test_memory(&module);

        generate_catch_wrapper(
            &mut module,
            import_func,
            js_tag,
            WrapperKind::CatchWrapper,
            Some(table),
            Some(table_alloc),
            Some(exn_store),
            TERMINATED_ADDR,
            memory,
            eh_version,
        );

        let wasm = module.emit_wasm();
        wasm_text_printer::print_bytes(&wasm).unwrap()
    }

    #[test]
    fn test_legacy_wrapper_body() {
        let wat = build_wrapper_wat(super::super::ExceptionHandlingVersion::Legacy);
        let expected = include_str!("../../../../fixtures/catch_handler_legacy.wat");
        assert_eq!(
            wat.trim(),
            expected.trim(),
            "Legacy wrapper WAT mismatch.\n\
             If intentional, update crates/cli-support/src/transforms/test-data/catch_handler_legacy.wat"
        );
    }

    #[test]
    fn test_modern_wrapper_body() {
        let wat = build_wrapper_wat(super::super::ExceptionHandlingVersion::Modern);
        let expected = include_str!("../../../../fixtures/catch_handler_modern.wat");
        assert_eq!(
            wat.trim(),
            expected.trim(),
            "Modern wrapper WAT mismatch.\n\
             If intentional, update crates/cli-support/src/transforms/test-data/catch_handler_modern.wat"
        );
    }

    #[test]
    fn test_rewrite_calls() {
        let wat = r#"
            (module
                (func $original (result i32)
                    i32.const 1
                )
                (func $wrapper (result i32)
                    i32.const 2
                )
                (func $caller (result i32)
                    call $original
                )
                (export "original" (func $original))
                (export "wrapper" (func $wrapper))
                (export "caller" (func $caller))
            )
        "#;
        let mut module = parse_wat(wat);

        let original = module
            .exports
            .iter()
            .find(|e| e.name == "original")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let wrapper = module
            .exports
            .iter()
            .find(|e| e.name == "wrapper")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        let caller = module
            .exports
            .iter()
            .find(|e| e.name == "caller")
            .and_then(|e| match e.item {
                walrus::ExportItem::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();

        // Create wrapper map
        let mut wrappers = HashMap::new();
        wrappers.insert(original, wrapper);

        // Rewrite calls
        rewrite_calls(&mut module, &wrappers).unwrap();

        // Check that the caller now calls wrapper instead of original
        let caller_func = module.funcs.get(caller);
        if let walrus::FunctionKind::Local(local) = &caller_func.kind {
            let mut found_call = false;
            for (instr, _) in local.block(local.entry_block()).instrs.iter() {
                if let Instr::Call(Call { func }) = instr {
                    assert_eq!(*func, wrapper, "call should be rewritten to wrapper");
                    found_call = true;
                }
            }
            assert!(found_call, "should have found a call instruction");
        } else {
            panic!("expected local function");
        }
    }
}
