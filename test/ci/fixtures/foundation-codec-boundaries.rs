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
