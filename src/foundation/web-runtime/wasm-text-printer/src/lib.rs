//! A crate to convert a WebAssembly binary to its textual representation in the
//! WebAssembly Text Format (WAT).
//!
//! This crate is intended for developer toolchains and debugging, supporting
//! human-readable versions of a wasm binary. This can also be useful when
//! developing wasm toolchain support in Rust for various purposes like testing
//! and debugging and such.

#![deny(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

use std::fmt::Write as _;

mod error;
pub use error::Error;
type Result<T> = std::result::Result<T, Error>;
use operator::{OperatorSeparator, OperatorState, PrintOperator};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::marker;
use std::mem;
use wasmparser::*;

const MAX_LOCALS: u32 = 50000;
const MAX_NESTING_TO_PRINT: u32 = 50;
const MAX_WASM_FUNCTIONS: u32 = 1_000_000;
const MAX_WASM_FUNCTION_SIZE: u32 = 128 * 1024;

mod operator;

/// Prints an in-memory `wasm` binary blob into an in-memory `String` which is
/// its textual representation.
pub fn print_bytes(wasm: &[u8]) -> Result<String> {
    let mut dst = String::new();
    Printer {
        result: &mut dst,
        code_section_hints: Vec::new(),
        group_lines: Vec::new(),
        line: 0,
        nesting: 0,
    }
    .print_contents(wasm)?;
    Ok(dst)
}

/// This structure is the actual structure that prints WebAssembly binaries.
struct Printer<'cfg> {
    result: &'cfg mut String,
    nesting: u32,
    line: usize,
    group_lines: Vec<usize>,
    code_section_hints: Vec<(u32, Vec<BranchHint>)>,
}

#[derive(Default)]
struct CoreState {
    types: Vec<Option<SubType>>,
    funcs: u32,
    func_to_type: Vec<Option<u32>>,
    memories: u32,
    tags: u32,
    tag_to_type: Vec<Option<u32>>,
    globals: u32,
    tables: u32,

    func_names: NamingMap<u32, NameFunc>,
    local_names: NamingMap<(u32, u32), NameLocal>,
    label_names: NamingMap<(u32, u32), NameLabel>,
    type_names: NamingMap<u32, NameType>,
    field_names: NamingMap<(u32, u32), NameField>,
    tag_names: NamingMap<u32, NameTag>,
    table_names: NamingMap<u32, NameTable>,
    memory_names: NamingMap<u32, NameMemory>,
    global_names: NamingMap<u32, NameGlobal>,
    element_names: NamingMap<u32, NameElem>,
    data_names: NamingMap<u32, NameData>,
}

/// A map of index-to-name for tracking what are the contents of the name
/// section.
///
/// The type parameter `T` is either `u32` for most index-based maps or a `(u32,
/// u32)` for label/local maps where there are two levels of indices.
///
/// The type parameter `K` is a static description/namespace for what kind of
/// item is contained within this map. That's used by some helper methods to
/// synthesize reasonable names automatically.
struct NamingMap<T, K> {
    index_to_name: HashMap<T, Naming>,
    _marker: marker::PhantomData<K>,
}

impl<T, K> Default for NamingMap<T, K> {
    fn default() -> NamingMap<T, K> {
        NamingMap {
            index_to_name: HashMap::new(),
            _marker: marker::PhantomData,
        }
    }
}

struct State {
    encoding: Encoding,
    name: Option<Naming>,
    core: CoreState,

    custom_section_place: Option<(&'static str, usize)>,
    // `custom_section_place` stores the text representation of the location where
    // a custom section should be serialized in the binary format.
    // The tuple elements are a str (e.g. "after elem") and the line number
    // where the custom section place was set. `update_custom_section_place` won't
    // update the custom section place unless the line number changes; this prevents
    // printing a place "after xxx" where the xxx section doesn't appear in the text format
    // (e.g. because it was present but empty in the binary format).
}

impl State {
    fn new(encoding: Encoding) -> Self {
        Self {
            encoding,
            name: None,
            core: CoreState::default(),

            custom_section_place: None,
        }
    }
}

struct Naming {
    name: String,
    kind: NamingKind,
}

enum NamingKind {
    DollarName,
    DollarQuotedName,
    SyntheticPrefix(String),
}

impl Printer<'_> {
    fn read_names(
        &mut self,
        mut bytes: &[u8],
        mut parser: Parser,
        state: &mut State,
    ) -> Result<()> {
        loop {
            let payload = match parser.parse(bytes, true)? {
                Chunk::NeedMoreData(_) => unreachable!(),
                Chunk::Parsed { payload, consumed } => {
                    bytes = &bytes[consumed..];
                    payload
                }
            };

            match payload {
                Payload::CodeSectionStart { size, .. } => {
                    if size as usize > bytes.len() {
                        return Err(Error::InvalidCodeSectionSize);
                    }
                    bytes = &bytes[size as usize..];
                    parser.skip_section();
                }

                Payload::CustomSection(c) => {
                    // Ignore any error associated with the name sections.
                    match c.as_known() {
                        KnownCustom::Name(reader) => {
                            drop(self.register_names(state, reader));
                        }

                        KnownCustom::BranchHints(reader) => {
                            drop(self.register_branch_hint_section(reader));
                        }
                        _ => {}
                    }
                }

                Payload::End(_) => break,
                _ => {}
            }
        }

        Ok(())
    }

    fn print_contents(&mut self, mut bytes: &[u8]) -> Result<()> {
        let mut states: Vec<State> = Vec::new();
        let mut parser = Parser::new(0);

        loop {
            let payload = match parser.parse(bytes, true)? {
                Chunk::NeedMoreData(_) => unreachable!(),
                Chunk::Parsed { payload, consumed } => {
                    bytes = &bytes[consumed..];
                    payload
                }
            };

            match payload {
                Payload::Version { encoding, .. } => {
                    assert!(states.last().map(|s| s.encoding) != Some(Encoding::Module));

                    match encoding {
                        Encoding::Module => {
                            states.push(State::new(Encoding::Module));
                            states.last_mut().unwrap().custom_section_place =
                                Some(("before first", self.line));
                            if states.len() > 1 {
                                self.start_group("core module");
                            } else {
                                self.start_group("module");
                            }
                        }
                        Encoding::Component => {
                            return Err(Error::UnsupportedComponent);
                        }
                    }

                    let len = states.len();
                    let state = states.last_mut().unwrap();

                    // First up try to find the `name` subsection which we'll use to print
                    // pretty names everywhere.
                    self.read_names(bytes, parser.clone(), state)?;

                    if len == 1 {
                        if let Some(name) = state.name.as_ref() {
                            self.result.push(' ');
                            name.write(self);
                        }
                    }
                }
                Payload::CustomSection(c) => {
                    // Malformed known custom sections retain their raw bytes and a diagnostic.
                    let state = states.last().unwrap();
                    let start = self.nesting;
                    match c.as_known() {
                        KnownCustom::Unknown => self.print_raw_custom_section(state, c.clone()),
                        _ => {
                            match (Printer {
                                result: &mut String::new(),
                                nesting: 0,
                                line: 0,
                                group_lines: Vec::new(),
                                code_section_hints: Vec::new(),
                            })
                            .print_known_custom_section(c.clone())
                            {
                                Ok(true) => {
                                    self.print_known_custom_section(c.clone())?;
                                }
                                Ok(false) => self.print_raw_custom_section(state, c.clone()),
                                Err(e) if !matches!(e, Error::Parse(_)) => return Err(e),
                                Err(e) => {
                                    let msg = format!(
                                        "failed to parse custom section `{}`: {e}",
                                        c.name()
                                    );
                                    for line in msg.lines() {
                                        self.newline();
                                        write!(self.result, ";; {line}")
                                            .expect("formatting into String cannot fail");
                                    }
                                    self.print_raw_custom_section(state, c.clone())
                                }
                            }
                        }
                    }
                    assert!(self.nesting == start);
                    self.update_custom_section_line(&mut states);
                }
                Payload::TypeSection(s) => {
                    self.print_types(states.last_mut().unwrap(), s)?;
                    self.update_custom_section_place(&mut states, "after type");
                }
                Payload::ImportSection(s) => {
                    self.print_imports(states.last_mut().unwrap(), s)?;
                    self.update_custom_section_place(&mut states, "after import");
                }
                Payload::FunctionSection(reader) => {
                    if reader.count() > MAX_WASM_FUNCTIONS {
                        return Err(Error::TooManyFunctions(reader.count()));
                    }
                    for ty in reader {
                        states.last_mut().unwrap().core.func_to_type.push(Some(ty?))
                    }
                    self.update_custom_section_place(&mut states, "after func");
                }
                Payload::TableSection(s) => {
                    self.print_tables(states.last_mut().unwrap(), s)?;
                    self.update_custom_section_place(&mut states, "after table");
                }
                Payload::MemorySection(s) => {
                    self.print_memories(states.last_mut().unwrap(), s)?;
                    self.update_custom_section_place(&mut states, "after memory");
                }
                Payload::TagSection(s) => {
                    self.print_tags(states.last_mut().unwrap(), s)?;
                    self.update_custom_section_place(&mut states, "after tag");
                }
                Payload::GlobalSection(s) => {
                    self.print_globals(states.last_mut().unwrap(), s)?;
                    self.update_custom_section_place(&mut states, "after global");
                }
                Payload::ExportSection(s) => {
                    self.print_exports(states.last().unwrap(), s)?;
                    self.update_custom_section_place(&mut states, "after export");
                }
                Payload::StartSection { func, .. } => {
                    self.newline();
                    self.start_group("start ");
                    self.print_idx(&states.last().unwrap().core.func_names, func);
                    self.end_group();
                    self.update_custom_section_place(&mut states, "after start");
                }
                Payload::ElementSection(s) => {
                    self.print_elems(states.last_mut().unwrap(), s)?;
                    self.update_custom_section_place(&mut states, "after elem");
                }
                Payload::CodeSectionStart { .. } => {}
                Payload::CodeSectionEntry(body) => {
                    self.print_code_section_entry(states.last_mut().unwrap(), &body)?;
                    self.update_custom_section_place(&mut states, "after code");
                }
                Payload::DataCountSection { .. } => {

                    // not part of the text format
                }
                Payload::DataSection(s) => {
                    self.print_data(states.last_mut().unwrap(), s)?;
                    self.update_custom_section_place(&mut states, "after data");
                }

                Payload::End(_) => {
                    self.end_group(); // close the `module` or `component` group

                    self.result.push('\n');
                    break;
                }

                other => match other.as_section() {
                    Some((id, _)) => return Err(Error::UnknownSection(id)),
                    None => return Err(Error::UnknownPayload),
                },
            }
        }

        Ok(())
    }

    fn update_custom_section_place(&self, states: &mut [State], place: &'static str) {
        if let Some(last) = states.last_mut() {
            if let Some((prev, prev_line)) = &mut last.custom_section_place {
                if *prev_line != self.line {
                    *prev = place;
                    *prev_line = self.line;
                }
            }
        }
    }

    fn update_custom_section_line(&self, states: &mut [State]) {
        if let Some(last) = states.last_mut() {
            if let Some((_, prev_line)) = &mut last.custom_section_place {
                *prev_line = self.line;
            }
        }
    }

    fn start_group(&mut self, name: &str) {
        write!(self.result, "(").expect("formatting into String cannot fail");

        write!(self.result, "{name}").expect("formatting into String cannot fail");

        self.nesting += 1;
        self.group_lines.push(self.line);
    }

    fn end_group(&mut self) {
        self.nesting -= 1;
        if let Some(line) = self.group_lines.pop() {
            if line != self.line {
                self.newline();
            }
        }
        self.result.push(')');
    }

    fn register_names(&mut self, state: &mut State, names: NameSectionReader<'_>) -> Result<()> {
        fn indirect_name_map<K>(
            into: &mut NamingMap<(u32, u32), K>,
            names: IndirectNameMap<'_>,
            name: &str,
        ) -> Result<()> {
            for indirect in names {
                let indirect = indirect?;
                let mut used = match name {
                    // labels can be shadowed, so maintaining the used names is not useful.
                    "label" => None,
                    "local" | "field" => Some(HashSet::new()),
                    _ => unimplemented!("{name} is an unknown type of indirect names"),
                };
                for naming in indirect.names {
                    let naming = naming?;
                    into.index_to_name.insert(
                        (indirect.index, naming.index),
                        Naming::new(naming.name, naming.index, name, used.as_mut()),
                    );
                }
            }
            Ok(())
        }

        for section in names {
            match section? {
                Name::Module { name, .. } => {
                    let name = Naming::new(name, 0, "module", None);
                    state.name = Some(name);
                }
                Name::Function(n) => name_map(&mut state.core.func_names, n, "func")?,
                Name::Local(n) => indirect_name_map(&mut state.core.local_names, n, "local")?,
                Name::Label(n) => indirect_name_map(&mut state.core.label_names, n, "label")?,
                Name::Type(n) => name_map(&mut state.core.type_names, n, "type")?,
                Name::Table(n) => name_map(&mut state.core.table_names, n, "table")?,
                Name::Memory(n) => name_map(&mut state.core.memory_names, n, "memory")?,
                Name::Global(n) => name_map(&mut state.core.global_names, n, "global")?,
                Name::Element(n) => name_map(&mut state.core.element_names, n, "elem")?,
                Name::Data(n) => name_map(&mut state.core.data_names, n, "data")?,
                Name::Field(n) => indirect_name_map(&mut state.core.field_names, n, "field")?,
                Name::Tag(n) => name_map(&mut state.core.tag_names, n, "tag")?,
                Name::Unknown { .. } => (),
            }
        }
        Ok(())
    }

    fn print_rec(&mut self, state: &mut State, rec: RecGroup) {
        if rec.is_explicit_rec_group() {
            self.start_group("rec");

            for ty in rec.into_types() {
                self.newline();
                self.print_type(state, ty);
            }
            self.end_group(); // `rec`
        } else {
            assert_eq!(rec.types().len(), 1);
            let ty = rec.into_types().next().unwrap();
            self.print_type(state, ty);
        }
    }

    fn print_type(&mut self, state: &mut State, ty: SubType) {
        self.start_group("type ");

        let ty_idx = state.core.types.len() as u32;
        self.print_name(&state.core.type_names, ty_idx);
        self.result.push(' ');
        self.print_sub(state, &ty, ty_idx);
        self.end_group(); // `type`
        state.core.types.push(Some(ty));
    }

    fn print_sub(&mut self, state: &State, ty: &SubType, ty_idx: u32) {
        if !ty.is_final || !ty.supertype_idx.is_none() {
            self.start_group("sub");
            self.print_sub_type(state, ty);
            self.print_composite(state, &ty.composite_type, ty_idx);
            self.end_group(); // `sub`
        } else {
            self.print_composite(state, &ty.composite_type, ty_idx);
        }
    }

    fn print_composite(&mut self, state: &State, ty: &CompositeType, ty_idx: u32) {
        if ty.shared {
            self.start_group("shared");
            self.result.push(' ');
        }
        if let Some(idx) = ty.describes_idx {
            self.start_group("describes");
            self.result.push(' ');
            self.print_idx(&state.core.type_names, idx.as_module_index().unwrap());
            self.end_group();
            self.result.push(' ');
        }
        if let Some(idx) = ty.descriptor_idx {
            self.start_group("descriptor");
            self.result.push(' ');
            self.print_idx(&state.core.type_names, idx.as_module_index().unwrap());
            self.end_group();
            self.result.push(' ');
        }
        match &ty.inner {
            CompositeInnerType::Func(ty) => {
                self.start_group("func");
                self.print_func_type(state, ty, None);
                self.end_group(); // `func`
            }
            CompositeInnerType::Array(ty) => {
                self.start_group("array");
                self.print_array_type(state, ty);
                self.end_group(); // `array`
            }
            CompositeInnerType::Struct(ty) => {
                self.start_group("struct");
                self.print_struct_type(state, ty, ty_idx);
                self.end_group(); // `struct`
            }
            CompositeInnerType::Cont(ty) => {
                self.start_group("cont");
                self.print_cont_type(state, ty);
                self.end_group(); // `cont`
            }
        };
        if ty.shared {
            self.end_group(); // `shared`
        }
    }

    fn print_types(&mut self, state: &mut State, parser: TypeSectionReader<'_>) -> Result<()> {
        for ty in parser.into_iter() {
            let rec_group = ty?;
            self.newline();
            self.print_rec(state, rec_group);
        }
        Ok(())
    }

    fn print_core_functype_idx(
        &mut self,
        state: &State,
        idx: u32,
        names_for: Option<u32>,
    ) -> Option<u32> {
        self.print_core_type_ref(state, idx);

        match state.core.types.get(idx as usize) {
            Some(Some(SubType {
                composite_type:
                    CompositeType {
                        inner: CompositeInnerType::Func(ty),
                        shared: false,
                        descriptor_idx: None,
                        describes_idx: None,
                    },
                ..
            })) => Some(self.print_func_type(state, ty, names_for)),
            Some(Some(_)) | Some(None) | None => None,
        }
    }

    /// Returns the number of parameters, useful for local index calculations
    /// later.
    fn print_func_type(&mut self, state: &State, ty: &FuncType, names_for: Option<u32>) -> u32 {
        if !ty.params().is_empty() {
            self.result.push(' ');
        }

        let mut params = NamedLocalPrinter::new("param");
        // Note that named parameters must be alone in a `param` block, so
        // we need to be careful to terminate previous param blocks and open
        // a new one if that's the case with a named parameter.
        for (i, param) in ty.params().iter().enumerate() {
            params.start_local(names_for, i as u32, self, state);
            self.print_valtype(state, *param);
            params.end_local(self);
        }
        params.finish(self);
        if !ty.results().is_empty() {
            self.result.push(' ');
            self.start_group("result");
            for result in ty.results().iter() {
                self.result.push(' ');
                self.print_valtype(state, *result);
            }
            self.end_group();
        }
        ty.params().len() as u32
    }

    fn print_field_type(
        &mut self,
        state: &State,
        ty: &FieldType,
        ty_field_idx: Option<(u32, u32)>,
    ) {
        self.result.push(' ');
        if let Some(idxs) = ty_field_idx {
            if let Some(name) = state.core.field_names.index_to_name.get(&idxs) {
                name.write_identifier(self);
                self.result.push(' ');
            }
        }
        if ty.mutable {
            self.result.push_str("(mut ");
        }
        self.print_storage_type(state, ty.element_type);
        if ty.mutable {
            self.result.push(')');
        }
    }

    fn print_array_type(&mut self, state: &State, ty: &ArrayType) {
        self.print_field_type(state, &ty.0, None)
    }

    fn print_struct_type(&mut self, state: &State, ty: &StructType, ty_idx: u32) {
        for (field_index, field) in ty.fields.iter().enumerate() {
            self.result.push_str(" (field");
            self.print_field_type(state, field, Some((ty_idx, field_index as u32)));
            self.result.push(')');
        }
    }

    fn print_cont_type(&mut self, state: &State, ct: &ContType) {
        self.result.push(' ');
        self.print_idx(&state.core.type_names, ct.0.as_module_index().unwrap());
    }

    fn print_sub_type(&mut self, state: &State, ty: &SubType) {
        self.result.push(' ');
        if ty.is_final {
            self.result.push_str("final ");
        }
        if let Some(idx) = ty.supertype_idx {
            self.print_idx(&state.core.type_names, idx.as_module_index().unwrap());
            self.result.push(' ');
        }
    }

    fn print_storage_type(&mut self, state: &State, ty: StorageType) {
        match ty {
            StorageType::I8 => self.result.push_str("i8"),
            StorageType::I16 => self.result.push_str("i16"),
            StorageType::Val(val_type) => self.print_valtype(state, val_type),
        }
    }

    fn print_valtype(&mut self, state: &State, ty: ValType) {
        match ty {
            ValType::I32 => self.print_type_keyword("i32"),
            ValType::I64 => self.print_type_keyword("i64"),
            ValType::F32 => self.print_type_keyword("f32"),
            ValType::F64 => self.print_type_keyword("f64"),
            ValType::V128 => self.print_type_keyword("v128"),
            ValType::Ref(rt) => self.print_reftype(state, rt),
        }
    }

    fn print_valtypes(&mut self, state: &State, tys: Vec<ValType>) {
        for ty in tys {
            self.result.push(' ');
            self.print_valtype(state, ty);
        }
    }

    fn print_reftype(&mut self, state: &State, ty: RefType) {
        if ty.is_nullable() {
            match ty.as_non_null() {
                RefType::FUNC => self.print_type_keyword("funcref"),
                RefType::EXTERN => self.print_type_keyword("externref"),
                RefType::I31 => self.print_type_keyword("i31ref"),
                RefType::ANY => self.print_type_keyword("anyref"),
                RefType::NONE => self.print_type_keyword("nullref"),
                RefType::NOEXTERN => self.print_type_keyword("nullexternref"),
                RefType::NOFUNC => self.print_type_keyword("nullfuncref"),
                RefType::EQ => self.print_type_keyword("eqref"),
                RefType::STRUCT => self.print_type_keyword("structref"),
                RefType::ARRAY => self.print_type_keyword("arrayref"),
                RefType::EXN => self.print_type_keyword("exnref"),
                RefType::NOEXN => self.print_type_keyword("nullexnref"),
                _ => {
                    self.start_group("ref");
                    self.result.push_str(" null ");
                    self.print_heaptype(state, ty.heap_type());
                    self.end_group();
                }
            }
        } else {
            self.start_group("ref ");
            self.print_heaptype(state, ty.heap_type());
            self.end_group();
        }
    }

    fn print_heaptype(&mut self, state: &State, ty: HeapType) {
        match ty {
            HeapType::Concrete(i) => {
                self.print_idx(&state.core.type_names, i.as_module_index().unwrap());
            }
            HeapType::Exact(i) => {
                self.start_group("exact ");
                self.print_idx(&state.core.type_names, i.as_module_index().unwrap());
                self.end_group();
            }
            HeapType::Abstract { shared, ty } => {
                use AbstractHeapType::*;
                if shared {
                    self.start_group("shared ");
                }
                match ty {
                    Func => self.print_type_keyword("func"),
                    Extern => self.print_type_keyword("extern"),
                    Any => self.print_type_keyword("any"),
                    None => self.print_type_keyword("none"),
                    NoExtern => self.print_type_keyword("noextern"),
                    NoFunc => self.print_type_keyword("nofunc"),
                    Eq => self.print_type_keyword("eq"),
                    Struct => self.print_type_keyword("struct"),
                    Array => self.print_type_keyword("array"),
                    I31 => self.print_type_keyword("i31"),
                    Exn => self.print_type_keyword("exn"),
                    NoExn => self.print_type_keyword("noexn"),
                    Cont => self.print_type_keyword("cont"),
                    NoCont => self.print_type_keyword("nocont"),
                }
                if shared {
                    self.end_group();
                }
            }
        }
    }

    fn print_type_keyword(&mut self, keyword: &str) {
        self.result.push_str(keyword);
    }

    fn print_imports(&mut self, state: &mut State, parser: ImportSectionReader<'_>) -> Result<()> {
        let update_state = |state: &mut State, ty: TypeRef| match ty {
            TypeRef::Func(idx) | TypeRef::FuncExact(idx) => {
                debug_assert!(state.core.func_to_type.len() == state.core.funcs as usize);
                state.core.funcs += 1;
                state.core.func_to_type.push(Some(idx))
            }
            TypeRef::Table(_) => state.core.tables += 1,
            TypeRef::Memory(_) => state.core.memories += 1,
            TypeRef::Tag(TagType {
                kind: _,
                func_type_idx: idx,
            }) => {
                debug_assert!(state.core.tag_to_type.len() == state.core.tags as usize);
                state.core.tags += 1;
                state.core.tag_to_type.push(Some(idx))
            }
            TypeRef::Global(_) => state.core.globals += 1,
        };

        for imports in parser.into_iter() {
            let imports = imports?;
            self.newline();
            match imports {
                Imports::Single(_, import) => {
                    self.print_import(state, &import, true)?;
                    update_state(state, import.ty);
                }
                Imports::Compact1 { module, items } => {
                    self.start_group("import ");
                    self.print_str(module);
                    for res in items.into_iter() {
                        let item = res?;
                        self.newline();
                        self.start_group("item ");
                        self.print_str(item.name);
                        self.result.push(' ');
                        self.print_import_ty(state, &item.ty, true)?;
                        self.end_group();
                        update_state(state, item.ty);
                    }
                    self.end_group();
                }
                Imports::Compact2 { module, ty, names } => {
                    self.start_group("import ");
                    self.print_str(module);
                    for res in names.into_iter() {
                        let item = res?;
                        self.newline();
                        self.start_group("item ");
                        self.print_str(item);
                        self.end_group();
                        update_state(state, ty);
                    }
                    self.newline();
                    self.print_import_ty(state, &ty, false)?;
                    self.end_group();
                }
            }
        }
        Ok(())
    }

    fn print_import(&mut self, state: &State, import: &Import<'_>, index: bool) -> Result<()> {
        self.start_group("import ");
        self.print_str(import.module);
        self.result.push(' ');
        self.print_str(import.name);
        self.result.push(' ');
        self.print_import_ty(state, &import.ty, index)?;
        self.end_group();
        Ok(())
    }

    fn print_import_ty(&mut self, state: &State, ty: &TypeRef, index: bool) -> Result<()> {
        match ty {
            TypeRef::Func(f) => {
                self.start_group("func ");
                if index {
                    self.print_name(&state.core.func_names, state.core.funcs);
                    self.result.push(' ');
                }
                self.print_core_type_ref(state, *f);
            }
            TypeRef::FuncExact(f) => {
                self.start_group("func ");
                if index {
                    self.print_name(&state.core.func_names, state.core.funcs);
                    self.result.push(' ');
                }
                self.start_group("exact ");
                self.print_core_type_ref(state, *f);
                self.end_group();
            }
            TypeRef::Table(f) => self.print_table_type(state, f, index),
            TypeRef::Memory(f) => self.print_memory_type(state, f, index)?,
            TypeRef::Tag(f) => self.print_tag_type(state, f, index),
            TypeRef::Global(f) => self.print_global_type(state, f, index),
        }
        self.end_group();
        Ok(())
    }

    fn print_table_type(&mut self, state: &State, ty: &TableType, index: bool) {
        self.start_group("table ");
        if index {
            self.print_name(&state.core.table_names, state.core.tables);
            self.result.push(' ');
        }
        if ty.shared {
            self.print_type_keyword("shared ");
        }
        if ty.table64 {
            self.print_type_keyword("i64 ");
        }
        self.print_limits(ty.initial, ty.maximum);
        self.result.push(' ');
        self.print_reftype(state, ty.element_type);
    }

    fn print_memory_type(&mut self, state: &State, ty: &MemoryType, index: bool) -> Result<()> {
        self.start_group("memory ");
        if index {
            self.print_name(&state.core.memory_names, state.core.memories);
            self.result.push(' ');
        }
        if ty.memory64 {
            self.print_type_keyword("i64 ");
        }
        self.print_limits(ty.initial, ty.maximum);
        if ty.shared {
            self.print_type_keyword(" shared");
        }
        if let Some(p) = ty.page_size_log2 {
            let p = 1_u64.checked_shl(p).ok_or(Error::InvalidPageSize(p))?;

            self.result.push(' ');
            self.start_group("pagesize ");
            write!(self.result, "{p:#x}").expect("formatting into String cannot fail");
            self.end_group();
        }
        Ok(())
    }

    fn print_tag_type(&mut self, state: &State, ty: &TagType, index: bool) {
        self.start_group("tag ");
        if index {
            self.print_name(&state.core.tag_names, state.core.tags);
            self.result.push(' ');
        }
        self.print_core_functype_idx(state, ty.func_type_idx, None);
    }

    fn print_limits<T>(&mut self, initial: T, maximum: Option<T>)
    where
        T: fmt::Display,
    {
        write!(self.result, "{initial}").expect("formatting into String cannot fail");
        if let Some(max) = maximum {
            write!(self.result, " {max}").expect("formatting into String cannot fail");
        }
    }

    fn print_global_type(&mut self, state: &State, ty: &GlobalType, index: bool) {
        self.start_group("global ");
        if index {
            self.print_name(&state.core.global_names, state.core.globals);
            self.result.push(' ');
        }
        if ty.shared || ty.mutable {
            self.result.push('(');
            if ty.shared {
                self.print_type_keyword("shared ");
            }
            if ty.mutable {
                self.print_type_keyword("mut ");
            }
            self.print_valtype(state, ty.content_type);
            self.result.push(')');
        } else {
            self.print_valtype(state, ty.content_type);
        }
    }

    fn print_tables(&mut self, state: &mut State, parser: TableSectionReader<'_>) -> Result<()> {
        for table in parser.into_iter() {
            let table = table?;
            self.newline();
            self.print_table_type(state, &table.ty, true);
            match &table.init {
                TableInit::RefNull => {}
                TableInit::Expr(expr) => {
                    self.result.push(' ');
                    self.print_const_expr(state, expr)?;
                }
            }
            self.end_group();
            state.core.tables += 1;
        }
        Ok(())
    }

    fn print_memories(&mut self, state: &mut State, parser: MemorySectionReader<'_>) -> Result<()> {
        for memory in parser.into_iter() {
            let memory = memory?;
            self.newline();
            self.print_memory_type(state, &memory, true)?;
            self.end_group();
            state.core.memories += 1;
        }
        Ok(())
    }

    fn print_tags(&mut self, state: &mut State, parser: TagSectionReader<'_>) -> Result<()> {
        for tag in parser.into_iter() {
            let tag = tag?;
            self.newline();
            self.print_tag_type(state, &tag, true);
            self.end_group();
            debug_assert!(state.core.tag_to_type.len() == state.core.tags as usize);
            state.core.tags += 1;
            state.core.tag_to_type.push(Some(tag.func_type_idx));
        }
        Ok(())
    }

    fn print_globals(&mut self, state: &mut State, parser: GlobalSectionReader<'_>) -> Result<()> {
        for global in parser.into_iter() {
            let global = global?;
            self.newline();
            self.print_global_type(state, &global.ty, true);
            self.result.push(' ');
            self.print_const_expr(state, &global.init_expr)?;
            self.end_group();
            state.core.globals += 1;
        }
        Ok(())
    }

    fn print_code_section_entry(
        &mut self,
        state: &mut State,
        body: &FunctionBody<'_>,
    ) -> Result<()> {
        self.newline();
        self.start_group("func ");
        let func_idx = state.core.funcs;
        self.print_name(&state.core.func_names, func_idx);
        self.result.push(' ');
        // Parser checks function/code counts before CodeSectionStart; imports
        // and declared functions each append their type before any body arrives.
        let ty = match state.core.func_to_type.get(func_idx as usize) {
            Some(Some(x)) => *x,
            _ => panic!("invalid function type"),
        };
        let params = self
            .print_core_functype_idx(state, ty, Some(func_idx))
            .unwrap_or(0);

        // Hints are stored on `self` in reverse order of function index so
        // check the last one and see if it matches this function.
        let hints = match self.code_section_hints.last() {
            Some((f, _)) if *f == func_idx => {
                let (_, hints) = self.code_section_hints.pop().unwrap();
                hints
            }
            _ => Vec::new(),
        };

        self.print_func_body(state, func_idx, params, body, &hints)?;
        self.end_group();

        state.core.funcs += 1;
        Ok(())
    }

    fn print_func_body(
        &mut self,
        state: &mut State,
        func_idx: u32,
        params: u32,
        body: &FunctionBody<'_>,
        branch_hints: &[BranchHint],
    ) -> Result<()> {
        let mut first = true;
        let mut local_idx = 0;
        let mut locals = NamedLocalPrinter::new("local");
        let mut reader = body.get_binary_reader();
        let func_start = reader.original_position();
        for _ in 0..reader.read_var_u32()? {
            let cnt = reader.read_var_u32()?;
            let ty = reader.read()?;
            if MAX_LOCALS
                .checked_sub(local_idx)
                .and_then(|s| s.checked_sub(cnt))
                .is_none()
            {
                return Err(Error::TooManyLocals);
            }
            for _ in 0..cnt {
                if first {
                    self.newline();
                    first = false;
                }
                locals.start_local(Some(func_idx), params + local_idx, self, state);
                self.print_valtype(state, ty);
                locals.end_local(self);
                local_idx += 1;
            }
        }
        locals.finish(self);

        let nesting_start = self.nesting;
        let mut operator_state = OperatorState::new(self, OperatorSeparator::Newline);

        {
            let mut flat_printer = PrintOperator::new(self, state, &mut operator_state);
            Self::print_operators(&mut reader, branch_hints, func_start, &mut flat_printer)?
        };

        // If this was an invalid function body then the nesting may not
        // have reset back to normal. Fix that up here and forcibly insert
        // a newline as well in case the last instruction was something
        // like an `if` which has a comment after it which could interfere
        // with the closing paren printed for the func.
        if self.nesting != nesting_start {
            self.nesting = nesting_start;
            self.newline();
        }

        Ok(())
    }

    fn print_operators<'a>(
        body: &mut BinaryReader<'a>,
        mut branch_hints: &[BranchHint],
        func_start: usize,
        op_printer: &mut PrintOperator<'_, '_, '_>,
    ) -> Result<()> {
        let mut ops = OperatorsReader::new(body.clone());
        while !ops.eof() {
            if ops.is_end_then_eof() {
                ops.read()?; // final "end" opcode terminates instruction sequence
                ops.finish()?;
                return Ok(());
            }

            // Branch hints are stored in increasing order of their body offset
            // so print them whenever their instruction comes up.
            if let Some((hint, rest)) = branch_hints.split_first() {
                if hint.func_offset == (ops.original_position() - func_start) as u32 {
                    branch_hints = rest;
                    op_printer.branch_hint(hint.taken);
                }
            }

            op_printer.visit_operator(&mut ops)?;
        }
        ops.finish()?; // for the error message
        Err(Error::UnexpectedEndOfOperators)
    }

    fn newline(&mut self) {
        self.result.push('\n');

        self.line += 1;

        // Clamp the maximum nesting size that we print at something somewhat
        // reasonable to avoid generating hundreds of megabytes of whitespace
        // for small-ish modules that have deep-ish nesting.
        for _ in 0..self.nesting.min(MAX_NESTING_TO_PRINT) {
            self.result.push_str("  ");
        }
    }

    fn print_exports(&mut self, state: &State, data: ExportSectionReader) -> Result<()> {
        for export in data.into_iter() {
            let export = export?;
            self.newline();
            self.print_export(state, &export);
        }
        Ok(())
    }

    fn print_export(&mut self, state: &State, export: &Export) {
        self.start_group("export ");
        self.print_str(export.name);
        self.result.push(' ');
        self.print_external_kind(state, export.kind, export.index);
        self.end_group(); // export
    }

    fn print_external_kind(&mut self, state: &State, kind: ExternalKind, index: u32) {
        match kind {
            ExternalKind::Func | ExternalKind::FuncExact => {
                self.start_group("func ");
                self.print_idx(&state.core.func_names, index);
            }
            ExternalKind::Table => {
                self.start_group("table ");
                self.print_idx(&state.core.table_names, index);
            }
            ExternalKind::Global => {
                self.start_group("global ");
                self.print_idx(&state.core.global_names, index);
            }
            ExternalKind::Memory => {
                self.start_group("memory ");
                self.print_idx(&state.core.memory_names, index);
            }
            ExternalKind::Tag => {
                self.start_group("tag ");
                write!(self.result, "{index}").expect("formatting into String cannot fail");
            }
        }
        self.end_group();
    }

    fn print_core_type_ref(&mut self, state: &State, idx: u32) {
        self.start_group("type ");
        self.print_idx(&state.core.type_names, idx);
        self.end_group();
    }

    // Core module names may refer to identifiers defined later in the text.
    fn print_idx<K>(&mut self, names: &NamingMap<u32, K>, idx: u32) {
        self._print_idx(&names.index_to_name, idx)
    }

    fn _print_idx(&mut self, names: &HashMap<u32, Naming>, idx: u32) {
        match names.get(&idx) {
            Some(name) => name.write_identifier(self),

            None => write!(self.result, "{idx}").expect("formatting into String cannot fail"),
        }
    }

    fn print_local_idx(&mut self, state: &State, func: u32, idx: u32) {
        match state.core.local_names.index_to_name.get(&(func, idx)) {
            Some(name) => name.write_identifier(self),

            None => write!(self.result, "{idx}").expect("formatting into String cannot fail"),
        }
    }

    fn print_field_idx(&mut self, state: &State, ty: u32, idx: u32) {
        match state.core.field_names.index_to_name.get(&(ty, idx)) {
            Some(name) => name.write_identifier(self),

            None => write!(self.result, "{idx}").expect("formatting into String cannot fail"),
        }
    }

    fn print_name<K>(&mut self, names: &NamingMap<u32, K>, cur_idx: u32) {
        self._print_name(&names.index_to_name, cur_idx)
    }

    fn _print_name(&mut self, names: &HashMap<u32, Naming>, cur_idx: u32) {
        if let Some(name) = names.get(&cur_idx) {
            name.write(self);
            self.result.push(' ');
        }
        write!(self.result, "(;{cur_idx};)").expect("formatting into String cannot fail");
    }

    fn print_elems(&mut self, state: &mut State, data: ElementSectionReader) -> Result<()> {
        for (i, elem) in data.into_iter().enumerate() {
            let mut elem = elem?;
            self.newline();
            self.start_group("elem ");
            self.print_name(&state.core.element_names, i as u32);
            match &mut elem.kind {
                ElementKind::Passive => {}
                ElementKind::Declared => self.result.push_str(" declare"),
                ElementKind::Active {
                    table_index,
                    offset_expr,
                } => {
                    if let Some(table_index) = *table_index {
                        self.result.push(' ');
                        self.start_group("table ");
                        self.print_idx(&state.core.table_names, table_index);
                        self.end_group();
                    }
                    self.result.push(' ');
                    self.print_const_expr_sugar(state, offset_expr, "offset")?;
                }
            }
            self.result.push(' ');

            match elem.items {
                ElementItems::Functions(reader) => {
                    self.result.push_str("func");
                    for idx in reader {
                        self.result.push(' ');
                        self.print_idx(&state.core.func_names, idx?)
                    }
                }
                ElementItems::Expressions(ty, reader) => {
                    self.print_reftype(state, ty);
                    for expr in reader {
                        self.result.push(' ');
                        self.print_const_expr_sugar(state, &expr?, "item")?
                    }
                }
            }

            self.end_group();
        }
        Ok(())
    }

    fn print_data(&mut self, state: &mut State, data: DataSectionReader) -> Result<()> {
        for (i, data) in data.into_iter().enumerate() {
            let data = data?;
            self.newline();
            self.start_group("data ");
            self.print_name(&state.core.data_names, i as u32);
            self.result.push(' ');
            match &data.kind {
                DataKind::Passive => {}
                DataKind::Active {
                    memory_index,
                    offset_expr,
                } => {
                    if *memory_index != 0 {
                        self.start_group("memory ");
                        self.print_idx(&state.core.memory_names, *memory_index);
                        self.end_group();
                        self.result.push(' ');
                    }
                    self.print_const_expr_sugar(state, offset_expr, "offset")?;
                    self.result.push(' ');
                }
            }

            self.print_bytes(data.data);

            self.end_group();
        }
        Ok(())
    }

    /// Prints the operators of `expr` space-separated, taking into account that
    /// if there's only one operator in `expr` then instead of `(explicit ...)`
    /// the printing can be `(...)`.
    fn print_const_expr_sugar(
        &mut self,
        state: &mut State,
        expr: &ConstExpr,
        explicit: &str,
    ) -> Result<()> {
        self.start_group("");
        let mut reader = expr.get_operators_reader();

        if reader.read().is_ok() && !reader.is_end_then_eof() {
            write!(self.result, "{explicit} ").expect("formatting into String cannot fail");
            self.print_const_expr(state, expr)?;
        } else {
            self.print_const_expr(state, expr)?;
        }

        self.end_group();
        Ok(())
    }

    /// Prints the operators of `expr` space-separated.
    fn print_const_expr(&mut self, state: &mut State, expr: &ConstExpr) -> Result<()> {
        let mut reader = expr.get_binary_reader();
        let mut operator_state = OperatorState::new(self, OperatorSeparator::NoneThenSpace);

        let mut op_printer = PrintOperator::new(self, state, &mut operator_state);
        Self::print_operators(&mut reader, &[], 0, &mut op_printer)?;

        Ok(())
    }

    fn print_str(&mut self, name: &str) {
        self.result.push('"');
        self.print_str_contents(name);
        self.result.push('"');
    }

    fn print_str_contents(&mut self, name: &str) {
        for c in name.chars() {
            let v = c as u32;
            if (0x20..0x7f).contains(&v) && c != '"' && c != '\\' && v < 0xff {
                write!(self.result, "{c}").expect("formatting into String cannot fail");
            } else {
                write!(self.result, "\\u{{{v:x}}}",).expect("formatting into String cannot fail");
            }
        }
    }

    fn print_bytes(&mut self, bytes: &[u8]) {
        self.result.push('"');
        for byte in bytes {
            if *byte >= 0x20 && *byte < 0x7f && *byte != b'"' && *byte != b'\\' {
                write!(self.result, "{}", *byte as char)
                    .expect("formatting into String cannot fail");
            } else {
                self.hex_byte(*byte);
            }
        }
        self.result.push('"');
    }

    fn hex_byte(&mut self, byte: u8) {
        write!(self.result, "\\{byte:02x}").expect("formatting into String cannot fail");
    }

    fn print_known_custom_section(&mut self, section: CustomSectionReader<'_>) -> Result<bool> {
        match section.as_known() {
            // For now `wasmprinter` has invented syntax for `producers` and
            // `dylink.0` below to use in tests. Note that this syntax is not
            // official at this time.
            KnownCustom::Producers(s) => {
                self.newline();
                self.print_producers_section(s)?;
                Ok(true)
            }
            KnownCustom::Dylink0(s) => {
                self.newline();
                self.print_dylink0_section(s)?;
                Ok(true)
            }

            // These are parsed during `read_names` and are part of
            // printing elsewhere, so don't print them.
            KnownCustom::Name(_) | KnownCustom::BranchHints(_) => Ok(true),

            _ => Ok(false),
        }
    }

    fn print_raw_custom_section(&mut self, state: &State, section: CustomSectionReader<'_>) {
        self.newline();
        self.start_group("@custom ");
        self.print_str(section.name());
        if let Some((place, _)) = state.custom_section_place {
            write!(self.result, " ({place})").expect("formatting into String cannot fail");
        }
        self.result.push(' ');

        self.print_bytes(section.data());

        self.end_group();
    }

    fn print_producers_section(&mut self, section: ProducersSectionReader<'_>) -> Result<()> {
        self.start_group("@producers");
        for field in section {
            let field = field?;
            for value in field.values.into_iter() {
                let value = value?;
                self.newline();
                self.start_group(field.name);
                self.result.push(' ');
                self.print_str(value.name);
                self.result.push(' ');
                self.print_str(value.version);
                self.end_group();
            }
        }
        self.end_group();
        Ok(())
    }

    fn print_dylink0_section(&mut self, mut section: Dylink0SectionReader<'_>) -> Result<()> {
        self.start_group("@dylink.0");
        loop {
            let next = match section.next() {
                Some(Ok(next)) => next,
                Some(Err(e)) => return Err(e.into()),
                None => break,
            };
            match next {
                Dylink0Subsection::MemInfo(info) => {
                    self.newline();
                    self.start_group("mem-info");
                    if info.memory_size > 0 || info.memory_alignment > 0 {
                        write!(
                            self.result,
                            " (memory {} {})",
                            info.memory_size, info.memory_alignment
                        )
                        .expect("formatting into String cannot fail");
                    }
                    if info.table_size > 0 || info.table_alignment > 0 {
                        write!(
                            self.result,
                            " (table {} {})",
                            info.table_size, info.table_alignment
                        )
                        .expect("formatting into String cannot fail");
                    }
                    self.end_group();
                }
                Dylink0Subsection::Needed(needed) => {
                    self.newline();
                    self.start_group("needed");
                    for s in needed {
                        self.result.push(' ');
                        self.print_str(s);
                    }
                    self.end_group();
                }
                Dylink0Subsection::ExportInfo(info) => {
                    for info in info {
                        self.newline();
                        self.start_group("export-info ");
                        self.print_str(info.name);
                        self.print_dylink0_flags(info.flags);
                        self.end_group();
                    }
                }
                Dylink0Subsection::ImportInfo(info) => {
                    for info in info {
                        self.newline();
                        self.start_group("import-info ");
                        self.print_str(info.module);
                        self.result.push(' ');
                        self.print_str(info.field);
                        self.print_dylink0_flags(info.flags);
                        self.end_group();
                    }
                }
                Dylink0Subsection::RuntimePath(runtime_path) => {
                    self.newline();
                    self.start_group("runtime-path");
                    for s in runtime_path {
                        self.result.push(' ');
                        self.print_str(s);
                    }
                    self.end_group();
                }
                Dylink0Subsection::Unknown { ty, .. } => {
                    return Err(Error::UnknownDylinkSubsection(ty));
                }
            }
        }
        self.end_group();
        Ok(())
    }

    fn print_dylink0_flags(&mut self, mut flags: SymbolFlags) {
        macro_rules! print_flag {
            ($($name:ident = $text:tt)*) => ({$(
                if flags.contains(SymbolFlags::$name) {
                    flags.remove(SymbolFlags::$name);
                    write!(self.result, concat!(" ", $text)).expect("formatting into String cannot fail");
                }
            )*})
        }
        // N.B.: Keep in sync with `parse_sym_flags` in `crates/wast/src/core/custom.rs`.
        print_flag! {
            BINDING_WEAK = "binding-weak"
            BINDING_LOCAL = "binding-local"
            VISIBILITY_HIDDEN = "visibility-hidden"
            UNDEFINED = "undefined"
            EXPORTED = "exported"
            EXPLICIT_NAME = "explicit-name"
            NO_STRIP = "no-strip"
            TLS = "tls"
            ABSOLUTE = "absolute"
        }
        if !flags.is_empty() {
            write!(self.result, " {flags:#x}").expect("formatting into String cannot fail");
        }
    }

    fn register_branch_hint_section(&mut self, section: BranchHintSectionReader<'_>) -> Result<()> {
        self.code_section_hints.clear();
        for func in section {
            let func = func?;
            if self.code_section_hints.len() >= MAX_WASM_FUNCTIONS as usize {
                return Err(Error::TooManyBranchHintFunctions);
            }
            if func.hints.count() >= MAX_WASM_FUNCTION_SIZE {
                return Err(Error::TooManyBranchHints);
            }
            let hints = func
                .hints
                .into_iter()
                .collect::<wasmparser::Result<Vec<_>>>()?;
            self.code_section_hints.push((func.func, hints));
        }
        self.code_section_hints.reverse();
        Ok(())
    }
}

struct NamedLocalPrinter {
    group_name: &'static str,
    in_group: bool,
    end_group_after_local: bool,
    first: bool,
}

impl NamedLocalPrinter {
    fn new(group_name: &'static str) -> NamedLocalPrinter {
        NamedLocalPrinter {
            group_name,
            in_group: false,
            end_group_after_local: false,
            first: true,
        }
    }

    fn start_local(&mut self, func: Option<u32>, local: u32, dst: &mut Printer, state: &State) {
        let name = state
            .core
            .local_names
            .index_to_name
            .get(&(func.unwrap_or(u32::MAX), local));

        // Named locals must be in their own group, so if we have a name we need
        // to terminate the previous group.
        if name.is_some() && self.in_group {
            dst.end_group();
            self.in_group = false;
        }

        if self.first {
            self.first = false;
        } else {
            dst.result.push(' ');
        }

        // Next we either need a separator if we're already in a group or we
        // need to open a group for our new local.
        if !self.in_group {
            dst.start_group(self.group_name);
            dst.result.push(' ');
            self.in_group = true;
        }

        // Print the optional name if given...
        match name {
            Some(name) => {
                name.write(dst);
                dst.result.push(' ');
                self.end_group_after_local = true;
            }

            None => {
                self.end_group_after_local = false;
            }
        }
    }

    fn end_local(&mut self, dst: &mut Printer) {
        if self.end_group_after_local {
            dst.end_group();
            self.end_group_after_local = false;
            self.in_group = false;
        }
    }
    fn finish(self, dst: &mut Printer) {
        if self.in_group {
            dst.end_group();
        }
    }
}

macro_rules! print_float {
    ($name:ident $float:ident $uint:ident $sint:ident $exp_bits:tt) => {
        fn $name(&mut self, mut bits: $uint) {
            // Calculate a few constants
            let int_width = mem::size_of::<$uint>() * 8;
            let exp_width = $exp_bits;
            let mantissa_width = int_width - 1 - exp_width;
            let bias = (1 << (exp_width - 1)) - 1;
            let max_exp = (1 as $sint) << (exp_width - 1);
            let min_exp = -max_exp + 1;

            // Handle `NaN` and infinity specially
            let f = $float::from_bits(bits);
            if bits >> (int_width - 1) != 0 {
                bits ^= 1 << (int_width - 1);
                self.result.push('-');
            }
            if f.is_infinite() {
                self.result.push_str("inf ");

                write!(self.result, "(;={f};)").expect("formatting into String cannot fail");

                return;
            }
            if f.is_nan() {
                let payload = bits & ((1 << mantissa_width) - 1);

                if payload == 1 << (mantissa_width - 1) {
                    self.result.push_str("nan ");

                    write!(self.result, "(;={f};)").expect("formatting into String cannot fail");
                } else {
                    write!(self.result, "nan:{:#x} ", payload)
                        .expect("formatting into String cannot fail");

                    write!(self.result, "(;={f};)").expect("formatting into String cannot fail");
                }

                return;
            }

            // Figure out our exponent, but keep in mine that it's in an
            // integer width that may not be supported. As a result we do a few
            // tricks here:
            //
            // * Make the MSB the top bit of the exponent, then shift the
            //   exponent to the bottom. This means we now have a signed
            //   integer in `$sint` width representing the whole exponent.
            // * Do the arithmetic for the exponent (subtract)
            // * Next we only care about the lowest `$exp_bits` bits of the
            //   result, but we do care about the sign. Use sign-carrying of
            //   the signed integer shifts to shift it left then shift it back.
            //
            // Overall this should do basic arithmetic for `$exp_bits` bit
            // numbers and get the result back as a signed integer with `$sint`
            // bits in `exponent` representing the same decimal value.
            let mut exponent = (((bits << 1) as $sint) >> (mantissa_width + 1)).wrapping_sub(bias);
            exponent = (exponent << (int_width - exp_width)) >> (int_width - exp_width);
            let mut fraction = bits & ((1 << mantissa_width) - 1);

            self.result.push_str("0x");
            if bits == 0 {
                self.result.push_str("0p+0");
            } else {
                self.result.push('1');
                if fraction > 0 {
                    fraction <<= (int_width - mantissa_width);

                    // Apparently the subnormal is handled here. I don't know
                    // what a subnormal is. If someone else does, please let me
                    // know!
                    if exponent == min_exp {
                        let leading = fraction.leading_zeros();
                        if (leading as usize) < int_width - 1 {
                            fraction <<= leading + 1;
                        } else {
                            fraction = 0;
                        }
                        exponent -= leading as $sint;
                    }

                    self.result.push('.');
                    while fraction > 0 {
                        write!(self.result, "{:x}", fraction >> (int_width - 4))
                            .expect("formatting into String cannot fail");
                        fraction <<= 4;
                    }
                }
                write!(self.result, "p{:+}", exponent).expect("formatting into String cannot fail");
            }

            write!(self.result, " (;={};)", f).expect("formatting into String cannot fail");
        }
    };
}

impl Printer<'_> {
    print_float!(print_f32 f32 u32 i32 8);
    print_float!(print_f64 f64 u64 i64 11);
}

impl Naming {
    fn new<'a>(
        name: &'a str,
        index: u32,
        group: &str,
        used: Option<&mut HashSet<&'a str>>,
    ) -> Naming {
        let mut kind = NamingKind::DollarName;
        if name.chars().any(|c| !is_idchar(c)) {
            kind = NamingKind::DollarQuotedName;
        }

        // If the `name` provided can't be used as the raw identifier for the
        // item that it's describing then a synthetic name must be made. The
        // rules here which generate a name are:
        //
        // * Empty identifiers are not allowed
        // * Identifiers have a fixed set of valid characters
        // * For wasmprinter's purposes we "reserve" identifiers with the `#`
        //   prefix, which is in theory rare to encounter in practice.
        // * If the name has already been used for some other item and cannot
        //   be reused (e.g. because shadowing in this context is not possible).
        //
        // If any of these conditions match then we generate a unique identifier
        // based on `name` but not it exactly. By factoring in the `group`,
        // `index`, and `name` we get a guaranteed unique identifier (due to the
        // leading `#` prefix that we reserve and factoring in of the item
        // index) while preserving human readability at least somewhat (the
        // valid identifier characters of `name` still appear in the returned
        // name).
        if name.is_empty()
            || name.starts_with('#')
            || used.map(|set| !set.insert(name)).unwrap_or(false)
        {
            kind = NamingKind::SyntheticPrefix(format!("#{group}{index}"));
        }
        return Naming {
            kind,
            name: name.to_string(),
        };

        // See https://webassembly.github.io/spec/core/text/values.html#text-id
        fn is_idchar(c: char) -> bool {
            matches!(
                c,
                '0'..='9'
                | 'a'..='z'
                | 'A'..='Z'
                | '!'
                | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '.'
                | '/'
                | ':'
                | '<'
                | '='
                | '>'
                | '?'
                | '@'
                | '\\'
                | '^'
                | '_'
                | '`'
                | '|'
                | '~'
            )
        }
    }

    fn write_identifier(&self, printer: &mut Printer<'_>) {
        match &self.kind {
            NamingKind::DollarName => {
                printer.result.push('$');
                printer.result.push_str(&self.name);
            }
            NamingKind::DollarQuotedName => {
                printer.result.push_str("$\"");
                printer.print_str_contents(&self.name);
                printer.result.push('"');
            }
            NamingKind::SyntheticPrefix(prefix) => {
                printer.result.push_str("$\"");
                printer.result.push_str(prefix);
                printer.result.push(' ');
                printer.print_str_contents(&self.name);
                printer.result.push('"');
            }
        }
    }

    fn write(&self, dst: &mut Printer<'_>) {
        self.write_identifier(dst);
        match &self.kind {
            NamingKind::DollarName | NamingKind::DollarQuotedName => {}

            NamingKind::SyntheticPrefix(_) => {
                dst.result.push(' ');
                dst.start_group("@name \"");
                dst.print_str_contents(&self.name);
                dst.result.push('"');
                dst.end_group();
            }
        }
    }
}

struct NameFunc;
struct NameGlobal;
struct NameMemory;
struct NameLocal;
struct NameLabel;
struct NameTable;
struct NameType;
struct NameField;
struct NameData;
struct NameElem;
struct NameTag;

fn name_map<K>(into: &mut NamingMap<u32, K>, names: NameMap<'_>, name: &str) -> Result<()> {
    let mut used = HashSet::new();
    for naming in names {
        let naming = naming?;
        into.index_to_name.insert(
            naming.index,
            Naming::new(naming.name, naming.index, name, Some(&mut used)),
        );
    }
    Ok(())
}
