use crate::{CustomSection, Encode, Ieee32, Ieee64, Section};
use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// The "core" custom section for coredumps, as described in the
/// [tool-conventions
/// repository](https://github.com/WebAssembly/tool-conventions/blob/main/Coredump.md).
///
/// There are four sections that comprise a core dump:
///     - "core", which contains the name of the core dump
///     - "coremodules", a listing of modules
///     - "coreinstances", a listing of module instances
///     - "corestack", a listing of frames for a specific thread
///
/// # Example of how these could be constructed and encoded into a module:
///
/// ```
/// use wasm_encoder::{
///     CoreDumpInstancesSection, CoreDumpModulesSection, CoreDumpSection, CoreDumpStackSection,
///     CoreDumpValue, Module,
/// };
/// let core = CoreDumpSection::new("MyModule.wasm");
///
/// let mut modules = CoreDumpModulesSection::new();
/// modules.module("my_module");
///
/// let mut instances = CoreDumpInstancesSection::new();
/// let module_idx = 0;
/// let memories = vec![1];
/// let globals = vec![2];
/// instances.instance(module_idx, memories, globals);
///
/// let mut thread = CoreDumpStackSection::new("main");
/// let instance_index = 0;
/// let func_index = 42;
/// let code_offset = 0x1234;
/// let locals = vec![CoreDumpValue::I32(1)];
/// let stack = vec![CoreDumpValue::I32(2)];
/// thread.frame(instance_index, func_index, code_offset, locals, stack);
///
/// let mut module = Module::new();
/// module.section(&core);
/// module.section(&modules);
/// module.section(&instances);
/// module.section(&thread);
/// ```
#[derive(Clone, Debug, Default)]
pub struct CoreDumpSection {
    name: String,
}

impl CoreDumpSection {
    /// Create a new core dump section encoder
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        CoreDumpSection { name }
    }

    /// View the encoded section as a CustomSection.
    fn as_custom<'a>(&'a self) -> CustomSection<'a> {
        let mut data = vec![0];
        self.name.encode(&mut data);
        CustomSection {
            name: "core".into(),
            data: Cow::Owned(data),
        }
    }
}

impl Encode for CoreDumpSection {
    fn encode(&self, sink: &mut Vec<u8>) {
        self.as_custom().encode(sink);
    }
}

impl Section for CoreDumpSection {
    fn id(&self) -> u8 {
        crate::core::SectionId::Custom as u8
    }
}

/// The "coremodules" custom section for coredumps which lists the names of the
/// modules
///
/// # Example
///
/// ```
/// use wasm_encoder::{CoreDumpModulesSection, Module};
/// let mut modules_section = CoreDumpModulesSection::new();
/// modules_section.module("my_module");
/// let mut module = Module::new();
/// module.section(&modules_section);
/// ```
#[derive(Debug)]
pub struct CoreDumpModulesSection {
    num_added: u32,
    bytes: Vec<u8>,
}

impl CoreDumpModulesSection {
    /// Create a new core dump modules section encoder.
    pub fn new() -> Self {
        CoreDumpModulesSection {
            bytes: vec![],
            num_added: 0,
        }
    }

    /// View the encoded section as a CustomSection.
    pub fn as_custom(&self) -> CustomSection<'_> {
        let mut data = vec![];
        self.num_added.encode(&mut data);
        data.extend(self.bytes.iter().copied());
        CustomSection {
            name: "coremodules".into(),
            data: Cow::Owned(data),
        }
    }

    /// Encode a module name into the section's bytes.
    pub fn module(&mut self, module_name: impl AsRef<str>) -> &mut Self {
        self.bytes.push(0x0);
        module_name.as_ref().encode(&mut self.bytes);
        self.num_added += 1;
        self
    }

    /// The number of modules that are encoded in the section.
    pub fn len(&self) -> u32 {
        self.num_added
    }
}

impl Encode for CoreDumpModulesSection {
    fn encode(&self, sink: &mut Vec<u8>) {
        self.as_custom().encode(sink);
    }
}

impl Section for CoreDumpModulesSection {
    fn id(&self) -> u8 {
        crate::core::SectionId::Custom as u8
    }
}

/// The "coreinstances" section for the core dump
#[derive(Debug)]
pub struct CoreDumpInstancesSection {
    num_added: u32,
    bytes: Vec<u8>,
}

impl CoreDumpInstancesSection {
    /// Create a new core dump instances section encoder.
    pub fn new() -> Self {
        CoreDumpInstancesSection {
            bytes: vec![],
            num_added: 0,
        }
    }

    /// View the encoded section as a CustomSection.
    pub fn as_custom(&self) -> CustomSection<'_> {
        let mut data = vec![];
        self.num_added.encode(&mut data);
        data.extend(self.bytes.iter().copied());
        CustomSection {
            name: "coreinstances".into(),
            data: Cow::Owned(data),
        }
    }

    /// Encode an instance into the section's bytes.
    pub fn instance<M, G>(&mut self, module_index: u32, memories: M, globals: G) -> &mut Self
    where
        M: IntoIterator<Item = u32>,
        <M as IntoIterator>::IntoIter: ExactSizeIterator,
        G: IntoIterator<Item = u32>,
        <G as IntoIterator>::IntoIter: ExactSizeIterator,
    {
        self.bytes.push(0x0);
        module_index.encode(&mut self.bytes);
        crate::encode_vec(memories, &mut self.bytes);
        crate::encode_vec(globals, &mut self.bytes);
        self.num_added += 1;
        self
    }

    /// The number of instances that are encoded in the section.
    pub fn len(&self) -> u32 {
        self.num_added
    }
}

impl Encode for CoreDumpInstancesSection {
    fn encode(&self, sink: &mut Vec<u8>) {
        self.as_custom().encode(sink);
    }
}

impl Section for CoreDumpInstancesSection {
    fn id(&self) -> u8 {
        crate::core::SectionId::Custom as u8
    }
}

/// A "corestack" custom section as described in the [tool-conventions
/// repository](https://github.com/WebAssembly/tool-conventions/blob/main/Coredump.md)
///
/// # Example
///
/// ```
/// use wasm_encoder::{CoreDumpStackSection, Module, CoreDumpValue};
/// let mut thread = CoreDumpStackSection::new("main");
///
/// let instance_index = 0;
/// let func_index = 42;
/// let code_offset = 0x1234;
/// let locals = vec![CoreDumpValue::I32(1)];
/// let stack = vec![CoreDumpValue::I32(2)];
/// thread.frame(instance_index, func_index, code_offset, locals, stack);
///
/// let mut module = Module::new();
/// module.section(&thread);
/// ```
#[derive(Clone, Debug, Default)]
pub struct CoreDumpStackSection {
    frame_bytes: Vec<u8>,
    count: u32,
    name: String,
}

impl CoreDumpStackSection {
    /// Create a new core dump stack section encoder.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        CoreDumpStackSection {
            frame_bytes: Vec::new(),
            count: 0,
            name,
        }
    }

    /// Add a stack frame to this coredump stack section.
    pub fn frame<L, S>(
        &mut self,
        instanceidx: u32,
        funcidx: u32,
        codeoffset: u32,
        locals: L,
        stack: S,
    ) -> &mut Self
    where
        L: IntoIterator<Item = CoreDumpValue>,
        <L as IntoIterator>::IntoIter: ExactSizeIterator,
        S: IntoIterator<Item = CoreDumpValue>,
        <S as IntoIterator>::IntoIter: ExactSizeIterator,
    {
        self.count += 1;
        self.frame_bytes.push(0);
        instanceidx.encode(&mut self.frame_bytes);
        funcidx.encode(&mut self.frame_bytes);
        codeoffset.encode(&mut self.frame_bytes);
        crate::encode_vec(locals, &mut self.frame_bytes);
        crate::encode_vec(stack, &mut self.frame_bytes);
        self
    }

    /// View the encoded section as a CustomSection.
    pub fn as_custom<'a>(&'a self) -> CustomSection<'a> {
        let mut data = vec![0];
        self.name.encode(&mut data);
        self.count.encode(&mut data);
        data.extend(&self.frame_bytes);
        CustomSection {
            name: "corestack".into(),
            data: Cow::Owned(data),
        }
    }
}

impl Encode for CoreDumpStackSection {
    fn encode(&self, sink: &mut Vec<u8>) {
        self.as_custom().encode(sink);
    }
}

impl Section for CoreDumpStackSection {
    fn id(&self) -> u8 {
        crate::core::SectionId::Custom as u8
    }
}

/// Local and stack values are encoded using one byte for the type (similar to
/// Wasm's Number Types) followed by bytes representing the actual value
/// See the tool-conventions repo for more details.
#[derive(Clone, Debug)]
pub enum CoreDumpValue {
    /// a missing value (usually missing because it was optimized out)
    Missing,
    /// An i32 value
    I32(i32),
    /// An i64 value
    I64(i64),
    /// An f32 value
    F32(Ieee32),
    /// An f64 value
    F64(Ieee64),
}

impl Encode for CoreDumpValue {
    fn encode(&self, sink: &mut Vec<u8>) {
        match self {
            CoreDumpValue::Missing => sink.push(0x01),
            CoreDumpValue::I32(x) => {
                sink.push(0x7F);
                x.encode(sink);
            }
            CoreDumpValue::I64(x) => {
                sink.push(0x7E);
                x.encode(sink);
            }
            CoreDumpValue::F32(x) => {
                sink.push(0x7D);
                x.encode(sink);
            }
            CoreDumpValue::F64(x) => {
                sink.push(0x7C);
                x.encode(sink);
            }
        }
    }
}

#[cfg(test)]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../test/foundation/wasm-encoder/unit/src/core/dump/tests_337.rs"
));
