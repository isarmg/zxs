#[test]
fn base64_known_vectors_and_rejected_padding() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    for (raw, encoded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v")] {
        assert_eq!(STANDARD.encode(raw), encoded);
        assert_eq!(STANDARD.decode(encoded).unwrap(), raw.as_bytes());
    }
    assert!(STANDARD.decode("Zg=").is_err());
    assert!(STANDARD.decode("Zh==").is_err());
    assert!(STANDARD.decode("!!!!").is_err());
}
#[test]
fn subtle_equality_and_selection_preserve_each_branch_value() {
    use subtle::{Choice, ConditionallySelectable, ConstantTimeEq, CtOption};
    assert_eq!(b"receipt".ct_eq(b"receipt").unwrap_u8(), 1);
    assert_eq!(b"receipt".ct_eq(b"receipT").unwrap_u8(), 0);
    assert_eq!(b"receipt".as_slice().ct_eq(b"receipts").unwrap_u8(), 0);
    assert_eq!(u64::conditional_select(&7, &19, Choice::from(0)), 7);
    assert_eq!(u64::conditional_select(&7, &19, Choice::from(1)), 19);
    assert_eq!(CtOption::new(23_u64, Choice::from(0)).unwrap_or(31), 31);
}
#[test]
fn wasm_encoder_output_is_valid_and_exports_the_expected_function() {
    use wasm_encoder::{
        CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction, Module,
        TypeSection, ValType,
    };
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I32]);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut exports = ExportSection::new();
    exports.export("answer", ExportKind::Func, 0);
    module.section(&exports);
    let mut code = CodeSection::new();
    let mut function = Function::new([]);
    function
        .instruction(&Instruction::I32Const(42))
        .instruction(&Instruction::End);
    code.function(&function);
    module.section(&code);
    let bytes = module.finish();
    wasmparser::Validator::new().validate_all(&bytes).unwrap();
    let mut observed = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
        if let wasmparser::Payload::ExportSection(section) = payload.unwrap() {
            for export in section {
                let e = export.unwrap();
                observed.push((e.name, e.kind, e.index));
            }
        }
    }
    assert_eq!(observed, [("answer", wasmparser::ExternalKind::Func, 0)]);
    assert!(
        wasmparser::Validator::new()
            .validate_all(&bytes[..bytes.len() - 1])
            .is_err()
    );
}
#[test]
fn wasm_parser_rejects_bad_magic_and_version() {
    assert!(
        wasmparser::Validator::new()
            .validate_all(b"nope\x01\0\0\0")
            .is_err()
    );
    assert!(
        wasmparser::Validator::new()
            .validate_all(b"\0asm\xff\0\0\0")
            .is_err()
    );
}

fn assert_wasm_locals_size_hint_bounds(bytes: &[u8]) {
    let locals = || {
        wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(bytes, 37))
            .get_locals_reader()
            .unwrap()
            .into_iter()
    };
    let items = locals().count();
    for consumed in 0..=items {
        let mut remaining = locals();
        for _ in 0..consumed {
            assert!(remaining.next().is_some());
        }
        let (lower, upper) = remaining.size_hint();
        let actual = remaining.count();
        assert!(
            lower <= actual,
            "lower {lower} exceeds {actual} remaining items"
        );
        assert!(upper.is_some_and(|upper| actual <= upper));
    }
}

#[test]
fn wasm_locals_size_hint_bounds_valid_progress() {
    for bytes in [&[0][..], &[2, 1, 0x7f, 2, 0x7e, 0x0b][..]] {
        assert_wasm_locals_size_hint_bounds(bytes);
    }
}

#[test]
fn wasm_locals_size_hint_bounds_error_termination() {
    for bytes in [&[5, 1, 0x7f][..], &[0xff, 0xff, 0xff, 0xff, 0x0f][..]] {
        assert_wasm_locals_size_hint_bounds(bytes);
    }
}

#[test]
fn wasm_locals_iteration_preserves_numeric_error_and_operator_position() {
    let overflow = [2, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x7f, 1, 0x7f];
    let mut locals = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(&overflow, 37))
        .get_locals_reader()
        .unwrap()
        .into_iter();
    assert_eq!(
        locals.next().unwrap().unwrap(),
        (u32::MAX, wasmparser::ValType::I32)
    );
    let error = locals.next().unwrap().unwrap_err();
    assert_eq!(error.message(), "too many locals");
    assert_eq!(error.offset(), 45);
    assert!(locals.next().is_none());

    let body = [1, 2, 0x7f, 0x0b];
    let mut locals = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(&body, 37))
        .get_locals_reader()
        .unwrap()
        .into_iter();
    assert_eq!(
        locals.next().unwrap().unwrap(),
        (2, wasmparser::ValType::I32)
    );
    assert!(locals.next().is_none());
    let mut operators = locals.into_binary_reader_for_operators();
    assert_eq!(operators.read_u8().unwrap(), 0x0b);
    assert!(operators.eof());
}

#[test]
fn dwarf_reader_decodes_leb_and_reports_truncation() {
    use gimli::{EndianSlice, LittleEndian, Reader};
    let mut reader = EndianSlice::new(&[0xe5, 0x8e, 0x26, 0x7e], LittleEndian);
    assert_eq!(reader.read_uleb128().unwrap(), 624_485);
    assert_eq!(reader.read_sleb128().unwrap(), -2);
    assert!(reader.is_empty());
    assert!(
        EndianSlice::new(&[0x80], LittleEndian)
            .read_uleb128()
            .is_err()
    );
    assert!(EndianSlice::new(&[1], LittleEndian).read_u32().is_err());
}

mod component_builder_name_regressions {
    use wasm_encoder::{
        ComponentBuilder, ComponentTypeRef, ComponentValType, InstanceType, PrimitiveValType,
    };
    use wasmparser::{ComponentName, KnownCustom, Parser, Payload};

    type NameEntries = Vec<(u32, String)>;
    type NameMaps = Vec<(&'static str, NameEntries)>;

    fn name_maps(bytes: &[u8]) -> (usize, NameMaps) {
        let mut section_count = 0;
        let mut result = Vec::new();
        for payload in Parser::new(0).parse_all(bytes) {
            let payload =
                payload.expect("invariant: fixture component encoding is structurally readable");
            if let Payload::CustomSection(section) = payload {
                if section.name() != "component-name" {
                    continue;
                }
                section_count += 1;
                let KnownCustom::ComponentName(subsections) = section.as_known() else {
                    panic!("invariant: component-name fixture must have a known parser");
                };
                for subsection in subsections {
                    let subsection = subsection.expect(
                        "invariant: fixture naming subsection must be structurally readable",
                    );
                    let (kind, names) = match subsection {
                        ComponentName::CoreFuncs(names) => ("core-funcs", names),
                        ComponentName::CoreGlobals(names) => ("core-globals", names),
                        ComponentName::CoreMemories(names) => ("core-memories", names),
                        ComponentName::CoreTables(names) => ("core-tables", names),
                        ComponentName::CoreTags(names) => ("core-tags", names),
                        ComponentName::CoreModules(names) => ("core-modules", names),
                        ComponentName::CoreInstances(names) => ("core-instances", names),
                        ComponentName::CoreTypes(names) => ("core-types", names),
                        ComponentName::Types(names) => ("types", names),
                        ComponentName::Instances(names) => ("instances", names),
                        ComponentName::Components(names) => ("components", names),
                        ComponentName::Funcs(names) => ("funcs", names),
                        ComponentName::Values(names) => ("values", names),
                        ComponentName::Component { .. } | ComponentName::Unknown { .. } => {
                            panic!("invariant: fixtures register namespace names only");
                        }
                    };
                    let entries = names
                        .into_iter()
                        .map(|entry| {
                            let entry =
                                entry.expect("invariant: fixture name map entry must be readable");
                            (entry.index, entry.name.to_owned())
                        })
                        .collect();
                    result.push((kind, entries));
                }
            }
        }
        (section_count, result)
    }

    fn entries(values: &[(u32, &str)]) -> NameEntries {
        values
            .iter()
            .map(|&(index, name)| (index, name.to_owned()))
            .collect()
    }

    #[test]
    fn registered_values_have_one_complete_name_map() {
        let mut builder = ComponentBuilder::default();
        assert_eq!(
            builder.import("v0", ComponentTypeRef::Value(PrimitiveValType::U32.into())),
            0
        );
        assert_eq!(
            builder.import(
                "v1",
                ComponentTypeRef::Value(PrimitiveValType::String.into())
            ),
            1
        );
        assert_eq!(builder.value_count(), 2);
        assert_eq!(builder.instance_count(), 0);
        builder.append_names();
        let bytes = builder.finish();
        assert_eq!(
            name_maps(&bytes),
            (1, vec![("values", entries(&[(0, "v0"), (1, "v1")]))])
        );
    }

    #[test]
    fn instance_map_is_not_duplicated_when_no_values_exist() {
        let mut builder = ComponentBuilder::default();
        let ty = builder.type_instance(Some("instance-type"), &InstanceType::new());
        assert_eq!(ty, 0);
        assert_eq!(builder.import("i0", ComponentTypeRef::Instance(ty)), 0);
        assert_eq!(builder.import("i1", ComponentTypeRef::Instance(ty)), 1);
        assert_eq!(builder.instance_count(), 2);
        assert_eq!(builder.value_count(), 0);
        builder.append_names();
        let bytes = builder.finish();
        assert_eq!(
            name_maps(&bytes),
            (
                1,
                vec![
                    ("instances", entries(&[(0, "i0"), (1, "i1")])),
                    ("types", entries(&[(0, "instance-type")])),
                ]
            )
        );
    }

    #[test]
    fn mixed_maps_preserve_indices_and_existing_subsection_order() {
        let mut builder = ComponentBuilder::default();
        let instance_ty = builder.type_instance(Some("instance-type"), &InstanceType::new());
        let function_ty = {
            let (index, mut encoder) = builder.type_function(Some("function-type"));
            encoder
                .params(core::iter::empty::<(&str, ComponentValType)>())
                .result(None);
            index
        };
        assert_eq!((instance_ty, function_ty), (0, 1));
        assert_eq!(
            builder.import("v0", ComponentTypeRef::Value(PrimitiveValType::U32.into())),
            0
        );
        assert_eq!(
            builder.import("i0", ComponentTypeRef::Instance(instance_ty)),
            0
        );
        assert_eq!(builder.import("f0", ComponentTypeRef::Func(function_ty)), 0);
        assert_eq!(
            builder.import("v1", ComponentTypeRef::Value(PrimitiveValType::Bool.into())),
            1
        );
        assert_eq!(
            builder.import("i1", ComponentTypeRef::Instance(instance_ty)),
            1
        );
        assert_eq!(
            (
                builder.value_count(),
                builder.instance_count(),
                builder.func_count(),
                builder.type_count()
            ),
            (2, 2, 1, 2)
        );
        builder.append_names();
        let bytes = builder.finish();
        assert_eq!(
            name_maps(&bytes),
            (
                1,
                vec![
                    ("instances", entries(&[(0, "i0"), (1, "i1")])),
                    ("funcs", entries(&[(0, "f0")])),
                    (
                        "types",
                        entries(&[(0, "instance-type"), (1, "function-type")])
                    ),
                    ("values", entries(&[(0, "v0"), (1, "v1")])),
                ]
            )
        );
    }

    #[test]
    fn ordinary_function_names_are_unchanged_without_instances_or_values() {
        let mut builder = ComponentBuilder::default();
        let ty = {
            let (index, mut encoder) = builder.type_function(Some("signature"));
            encoder
                .params(core::iter::empty::<(&str, ComponentValType)>())
                .result(None);
            index
        };
        assert_eq!(builder.import("f", ComponentTypeRef::Func(ty)), 0);
        builder.append_names();
        let bytes = builder.finish();
        assert_eq!(
            name_maps(&bytes),
            (
                1,
                vec![
                    ("funcs", entries(&[(0, "f")])),
                    ("types", entries(&[(0, "signature")])),
                ]
            )
        );
    }

    #[test]
    fn unnamed_empty_builder_emits_no_name_section() {
        let mut builder = ComponentBuilder::default();
        builder.append_names();
        let bytes = builder.finish();
        assert_eq!(name_maps(&bytes), (0, vec![]));
    }
}
