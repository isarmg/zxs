use crate::descriptor::VectorKind;
use crate::intrinsic::Intrinsic;
use crate::transforms::{
    has_local_exception_tags, threads as threads_xform, unstart_start_function,
};
use crate::wasm_conventions::get_memory;
use crate::wit::{
    Adapter, AdapterId, AdapterJsImportKind, AuxExportedMethodKind, AuxReceiverKind, AuxStringEnum,
    AuxValue,
};
use crate::wit::{AdapterKind, Instruction, InstructionData};
use crate::wit::{AuxEnum, AuxExport, AuxExportKind, AuxImport, AuxStruct};
use crate::wit::{JsImport, JsImportName, NonstandardWitSection, WasmBindgenAux};
use crate::{Bindgen, PLACEHOLDER_MODULE};
use anyhow::{anyhow, bail, Context as _, Error};
use std::borrow::Cow;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use walrus::{FunctionId, ImportId, MemoryId, Module, TableId, ValType};
use wasm_bindgen_shared::escape_string;
use wasm_bindgen_shared::identifier::{is_valid_ident, to_valid_ident};

mod binding;

macro_rules! region {
    ($ctx:expr, $name:literal, $code:block) => {
        if $ctx.config.debug {
            $ctx.globals.push_str(concat!("\n//#region ", $name, "\n"));
        }
        $code
        if $ctx.config.debug {
            $ctx.globals.push_str("\n//#endregion\n");
        }
    };
}

/// JS code that satisfies a wasm import.
///
/// Web imports refer to a function, an expression, or an existing global.
#[derive(Debug, Clone)]
enum ImportDefinition {
    /// A function literal. Holds the parameter list and body, with no
    /// leading `function` keyword (e.g. `(arg0, arg1) { ... }`).
    Function(String),
    /// An arbitrary JS expression evaluated for its value.
    Expression(String),
    /// The name of a top-level binding declared elsewhere in the same JS
    /// file (e.g. via `Context::global`).
    GlobalRef(String),
}

impl ImportDefinition {
    fn body(&self) -> String {
        match self {
            ImportDefinition::Function(body) => format!("function{}", body.trim()),
            ImportDefinition::Expression(expr) => expr.trim().to_string(),
            ImportDefinition::GlobalRef(ident) => ident.trim().to_string(),
        }
    }
}

pub struct Context<'a> {
    globals: String,
    /// ES module `import` statements collected during codegen, emitted at the
    /// top of the output in `finalize()`.
    es_module_imports: String,
    intrinsics: Option<BTreeMap<Cow<'static, str>, Cow<'static, str>>>,

    imports_post: String,
    export_name_list: Vec<String>,

    config: &'a Bindgen,
    pub module: &'a mut Module,
    aux: &'a WasmBindgenAux,
    wit: &'a NonstandardWitSection,

    /// A map representing the `import` statements we'll be generating in the JS
    /// glue. The key is the module we're importing from and the value is the
    /// list of identifier we're importing from the module, with optional
    /// renames for each identifier.
    js_imports: HashMap<String, Vec<(String, Option<String>)>>,

    /// A map of each Wasm import and what JS to hook up to it.
    wasm_import_definitions: HashMap<ImportId, ImportDefinition>,

    /// A map from an import to the name we've locally imported it as.
    imported_names: HashMap<JsImportName, String>,

    /// A set of all defined identifiers through either exports or imports to
    /// the number of times they've been used, used to generate new
    /// identifiers.
    defined_identifiers: HashMap<String, usize>,

    /// String enums that are used internally by the generated bindings.
    ///
    /// This tracks which string enums are used independently from whether their
    /// type is used, because users may only use them in a way that doesn't
    /// require the type or requires only the type.
    used_string_enums: HashSet<String>,

    exported_classes: BTreeMap<String, ExportedClass>,

    /// Public module exports
    exports: BTreeMap<String, ExportEntry>,

    /// A map of the name of npm dependencies we've loaded so far to the path
    /// they're defined in as well as their version specification.
    pub npm_dependencies: HashMap<String, (PathBuf, String)>,

    /// A mapping from the memory IDs as we see them to an index for that memory,
    /// used in function names, as well as all the kinds of views we've created
    /// of that memory.
    ///
    /// `BTreeMap` and `BTreeSet` are used to make the ordering deterministic.
    memories: BTreeMap<MemoryId, (usize, BTreeSet<&'static str>)>,
    table_indices: HashMap<TableId, usize>,

    /// A flag to track if the stack pointer setter shim has been injected.
    stack_pointer_shim_injected: bool,

    /// If threading is enabled.
    threads_enabled: bool,

    /// If exception handling / unwinding is enabled.
    unwind_enabled: bool,

    /// Whether reinit machinery should be emitted. True when `schedule_reinit()`
    /// is used (auto-detected via the `__wbindgen_reinit` intrinsic) or when
    /// `--experimental-reset-state-function` is passed. Controls instance-id
    /// tracking, the `__wbg_instance_id` global, and the private
    /// `__wbg_reset_state` function.
    generate_reinit: bool,

    /// Mapping from qualified name (used in WasmDescribe) to the unique declaration identifier.
    pub(crate) qualified_to_identifier: HashMap<String, String>,

    /// `true` when the module's memory is a memory64 (wasm64) memory.
    memory64: bool,

    /// Tracks whether the module-level `__wbgSuperSkip` sentinel has been
    /// emitted. The sentinel is used to short-circuit the body of parent
    /// class constructors when they are invoked as `super(__wbgSuperSkip)`
    /// from a subclass — avoiding a spurious extra Rust allocation that
    /// would otherwise be leaked.
    super_skip_sentinel_emitted: bool,

    /// Set to `true` while generating code for a JSPI-enabled export adapter.
    /// Consumed by `binding.rs` `Invocation::Core::generate()` to wrap the
    /// raw `wasm.fn(args)` call with a `WebAssembly.promising` lazy cache.
    pub(crate) current_adapter_jspi: bool,

    /// Set to `true` while generating the shim body for a
    /// `#[wasm_bindgen(suspending)]` import.  Consumed by `binding.rs` to
    /// ensure void-returning shims still emit `return <call>` — required
    /// because `WebAssembly.Suspending` must receive the Promise back from
    /// the wrapper function.
    pub(crate) current_adapter_suspending: bool,
}

/// Definition of a module export
enum ExportEntry {
    /// Any export definition
    Definition(ExportDefinition),
    /// Namespace export
    Namespace(ExportedNamespace),
}

struct ExportDefinition {
    /// The identifier for the declaration, if distinct from the export name
    /// This allows invalid identifier export names (like "default").
    identifier: String,
    comments: Option<String>,
    definition: String,

    /// Whether this is a private export, so not actually exposed on the module exports interface
    private: bool,

    /// If this definition is `class Child extends Parent { ... }`, the
    /// `Parent` identifier the body references. Used by `write_exports` to
    /// emit parents before children — otherwise `BTreeMap` (export-name)
    /// order may interleave `class Child extends Parent` before `Parent`
    /// is initialized, producing a TDZ `ReferenceError` at module load.
    parent_identifier: Option<String>,
}

/// Module namespace export
#[derive(Default)]
struct ExportedNamespace {
    /// Namespace id.
    id: Option<String>,
    /// Namespace entries in emission order. The order is set by
    /// `topo_sort_class_exports` and must reach JS emission unchanged so
    /// that `class Child extends Parent` is declared after `Parent` —
    /// otherwise the JS class-decl TDZ trips at module load.
    ns: Vec<(String, ExportEntry)>,
}

#[derive(Default)]
struct ExportedClass {
    comments: String,
    contents: String,
    identifier: String,

    /// Whether to skip exporting this class from the module exports
    private: bool,
    has_constructor: bool,
    wrap_needed: bool,
    unwrap_needed: bool,
    /// Whether to generate helper methods for inspecting the class
    is_inspectable: bool,
    /// All readable properties of the class
    readable_properties: Vec<String>,

    /// The namespace to export the class through, if any
    js_namespace: Option<Vec<String>>,
    /// The JS-facing name of this class (used for JS output)
    js_name: Option<String>,
    /// The JS name of the parent class this class extends, if any.
    /// Sourced from `AuxStruct.extends` via the `#[wasm_bindgen(extends)]`
    /// attribute on an exported Rust struct. The parent class is expected
    /// to be another exported Rust type defined in the same wasm module.
    extends: Option<String>,
    /// Resolved ancestor chain, nearest-first (i.e. direct parent at index
    /// 0, grandparent at index 1, etc.). Populated in
    /// `populate_inheritance_chains` before any JS emission so that
    /// constructor, `__wrap`, `free`, and method-body emitters all see a
    /// consistent per-class view of the chain. Each entry is
    /// `(identifier, qualified_name)` — `identifier` is the class's
    /// auto-sanitized JS ident (used as the `__wbg_ptr_<Ident>` field
    /// suffix), `qualified_name` is the `exported_classes` key (used for
    /// `upcast_function` / `free_function` symbol names).
    ancestors: Vec<(String, String)>,
    /// True if this class either declares `extends` or is extended by some
    /// other class. Non-participating classes keep the single-pointer
    /// `this.__wbg_ptr` representation for backward compatibility.
    participates_in_inheritance: bool,
}

const INITIAL_HEAP_VALUES: &[&str] = &["undefined", "null", "true", "false"];
// Must be kept in sync with `src/lib.rs` of the `wasm-bindgen` crate
const INITIAL_HEAP_OFFSET: usize = 1024;

impl<'a> Context<'a> {
    pub fn new(
        module: &'a mut Module,
        config: &'a Bindgen,
        wit: &'a NonstandardWitSection,
        aux: &'a WasmBindgenAux,
    ) -> Result<Context<'a>, Error> {
        let memory64 = module.memories.iter().next().is_some_and(|m| m.memory64);
        Ok(Context {
            globals: String::new(),
            es_module_imports: String::new(),
            intrinsics: Some(Default::default()),
            imports_post: String::new(),
            export_name_list: Vec::new(),

            imported_names: Default::default(),
            js_imports: Default::default(),
            defined_identifiers: Default::default(),
            wasm_import_definitions: Default::default(),

            used_string_enums: Default::default(),
            exported_classes: Default::default(),
            exports: Default::default(),
            config,
            threads_enabled: threads_xform::is_enabled(module),
            unwind_enabled: has_local_exception_tags(module),

            generate_reinit: (aux.uses_reinit || config.generate_reset_state),
            module,
            npm_dependencies: Default::default(),
            wit,
            aux,
            memories: Default::default(),
            table_indices: Default::default(),
            stack_pointer_shim_injected: false,
            qualified_to_identifier: Default::default(),

            memory64,
            super_skip_sentinel_emitted: false,
            current_adapter_jspi: false,
            current_adapter_suspending: false,
        })
    }

    /// Emits `const __wbgSuperSkip = Symbol();` at module level exactly
    /// once. Safe to call repeatedly. See the constructor codepath in
    /// `binding.rs` for the full rationale.
    pub(crate) fn expose_super_skip_sentinel(&mut self) {
        if self.super_skip_sentinel_emitted {
            return;
        }
        {
            self.globals
                .push_str("const __wbgSuperSkip = Symbol('wasm-bindgen.super-skip');\n");
        }
        self.super_skip_sentinel_emitted = true;
    }

    /// Whether the super-skip sentinel has been emitted. Constructor
    /// prelude only emits the sentinel check when a subclass in the same
    /// module has already forced its emission.
    pub(crate) fn has_super_skip_sentinel(&self) -> bool {
        self.super_skip_sentinel_emitted
    }

    fn has_intrinsic(&self, name: &str) -> bool {
        self.intrinsics.as_ref().unwrap().contains_key(name)
    }

    /// Suffix ` >>> 0` on wasm32, empty on wasm64. Append to a pointer-sized
    /// expression about to be used as a typed-array / DataView index.
    fn coerce_ptr_suffix(&self) -> &'static str {
        if self.memory64 {
            ""
        } else {
            " >>> 0"
        }
    }

    /// Statement form of [`Self::coerce_ptr_suffix`]: normalizes `name` in
    /// place on wasm32, expands to nothing on wasm64.
    fn coerce_ptr_assign(&self, name: &str) -> String {
        if self.memory64 {
            String::new()
        } else {
            format!("{name} = {name} >>> 0;")
        }
    }

    fn intrinsic(&mut self, name: Cow<'static, str>, val: Cow<'static, str>) {
        if self.intrinsics.as_ref().unwrap().contains_key(&name) {
            return;
        }

        {
            self.intrinsics.as_mut().unwrap().insert(name, val);
        }
    }
    /// Writes an export definition to the JavaScript output.
    /// Handles realising for invalid identifier export names.
    /// define_only used when only locally declared but not explicitly exported.
    fn export_def(&mut self, export_name: Option<&str>, def: &ExportDefinition) {
        let ExportDefinition {
            definition: decl,

            identifier: id,
            comments,

            private,
            parent_identifier: _,
        } = def;
        self.globals.push('\n');

        // Unless it is `export ...declaration...` form (common case), write the declaration first
        // and then export.
        if export_name.map(|name| name != id).unwrap_or(true) || *private {
            if let Some(c) = comments {
                self.globals.push_str(c);
            }
            self.globals.push_str(decl);
        } else if let Some(c) = comments {
            self.globals.push_str(c);
        }

        if *private {
            return;
        }
        if let Some(export_name) = export_name {
            {
                if export_name == id {
                    if !decl.is_empty() {
                        self.globals.push_str(&format!("export {decl}"));
                    } else {
                        // reexport case
                        self.globals.push_str(&format!("export {{ {id} }}\n"));
                    }
                } else if is_valid_ident(export_name) {
                    self.globals
                        .push_str(&format!("export {{ {id} as {export_name} }}\n"));
                } else {
                    self.globals
                        .push_str(&format!("export {{ {id} as '{export_name}' }}\n"));
                }
            }
        }
    }

    pub fn finalize(&mut self, module_name: &str) -> Result<String, Error> {
        // Finalize all bindings for JS classes. This is where we'll generate JS
        // glue for all classes as well as finish up a few final imports like
        // `__wrap` and such.
        self.write_classes()?;

        // Process reexports
        for (export_name, js_import) in self.aux.reexports.clone() {
            let import_name = self.import_name(&js_import)?;

            define_export(
                &mut self.exports,
                &export_name,
                &[],
                ExportEntry::Definition(ExportDefinition {
                    identifier: import_name,
                    comments: None,
                    definition: "".to_string(),

                    private: false,
                    parent_identifier: None,
                }),
            )?;
        }

        let body = std::mem::take(&mut self.globals);
        let body = body.trim();

        if !self.js_imports.is_empty() {
            self.js_import_header()?;
        }

        if !self.exports.is_empty() {
            // Write out all exports
            region!(self, "exports", {
                self.write_exports()?;
            });
        }

        {
            if let Some(mem) = self.module.memories.iter().next() {
                if let Some(id) = mem.import {
                    self.module.imports.get_mut(id).module = PLACEHOLDER_MODULE.to_owned();
                    let mut init_memory = "new WebAssembly.Memory({".to_string();
                    init_memory.push_str(&format!("initial:{}", mem.initial));
                    if let Some(max) = mem.maximum {
                        init_memory.push_str(&format!(",maximum:{max}"));
                    }
                    if mem.shared {
                        init_memory.push_str(",shared:true");
                    }
                    init_memory.push_str("})");
                    self.wasm_import_definitions
                        .insert(id, ImportDefinition::Expression(init_memory));
                }
            }
        }

        let mut has_memory = false;
        if let Some(mem) = self.module.memories.iter().next() {
            if let Some(id) = mem.import {
                if let Some(def) = self.wasm_import_definitions.get_mut(&id) {
                    {
                        match def {
                            ImportDefinition::Expression(expr) => expr.insert_str(0, "memory || "),
                            ImportDefinition::Function(_) | ImportDefinition::GlobalRef(_) => {
                                unreachable!("memory import is always inserted as an Expression")
                            }
                        }
                    }
                    has_memory = true;
                }
            }
        }

        region!(self, "wasm imports", {
            let imports = self.generate_web_imports(module_name, has_memory);
            self.globals.push_str(&imports);
        });

        let imports_post = std::mem::take(&mut self.imports_post);
        let imports_post = imports_post.trim();

        if !imports_post.is_empty() {
            self.globals.push_str(imports_post);
            self.globals.push('\n');
        }
        if !body.is_empty() {
            self.globals.push_str(body);
            self.globals.push_str("\n\n");
        }

        let intrinsics = self.intrinsics.take().unwrap();
        if !intrinsics.is_empty() {
            region!(self, "intrinsics", {
                for code in intrinsics.values() {
                    self.globals.push_str(code.trim());
                    self.globals.push_str("\n\n");
                }
            });
        }

        // Initialization is just flat out tricky and not something we
        // understand super well. To try to handle various issues that have come
        // up we always remove the `start` function if one is present. The JS
        // bindings glue then manually calls the start function (if it was
        // previously present).
        let needs_manual_start = unstart_start_function(self.module);
        region!(self, "wasm loading", {
            let wasm_loading =
                self.generate_wasm_loading(module_name, needs_manual_start, has_memory);
            self.globals.push_str(&wasm_loading);
        });

        if self.config.debug {
            {
                self.globals.push_str("export { wasm as __wasm }");
            }
        }

        // Insert collected ES module imports at the top of the output.
        if !self.es_module_imports.is_empty() {
            let mut es_imports = std::mem::take(&mut self.es_module_imports);
            es_imports.push('\n');
            self.globals.insert_str(0, &es_imports);
        }

        Ok(self.globals.to_owned())
    }

    fn generate_web_imports(&mut self, module_name: &str, has_memory: bool) -> String {
        let init_memory_arg = if has_memory { "memory" } else { "" };
        let mut fn_def = format!(
            "function __wbg_get_imports({init_memory_arg}) {{
            const import0 = {{
            __proto__: null,
        "
        );

        let self_module_name = format!("./{module_name}_bg.js");
        let mut return_stmt = format!(
            r#"
        return {{
            __proto__: null,
            "{self_module_name}": import0,
        "#
        );

        // e.g. snippets without parameters
        let import_modules = self
            .module
            .imports
            .iter()
            .map(|import| &import.module)
            .filter(|module| module.as_str() != PLACEHOLDER_MODULE);
        for (i, module) in import_modules.enumerate() {
            let i = i + 1;
            {
                self.es_module_imports
                    .push_str(&format!("import * as import{i} from \"{module}\"\n"));
            }

            return_stmt.push_str(&format!(r#""{module}": import{i},"#));
            return_stmt.push('\n');
        }
        return_stmt.push_str("};\n");

        for (id, def) in iter_by_import(&self.wasm_import_definitions, self.module) {
            let import = self.module.imports.get_mut(*id);
            fn_def.push_str(&format!("{}: {},\n", import.name, def.body()));
            import.module = self_module_name.clone();
        }

        fn_def.push_str("};");
        fn_def.push_str(&return_stmt);
        fn_def.push_str("}\n");

        format!("{fn_def}\n")
    }

    fn js_import_header(&mut self) -> Result<(), Error> {
        if self.config.omit_imports {
            return Ok(());
        }

        {
            for (module, items) in crate::sorted_iter(&self.js_imports) {
                write_es_import(&mut self.es_module_imports, module, items);
            }
        }
        Ok(())
    }

    fn generate_web_loading(
        &self,
        needs_manual_start: bool,
        default_module_path: &str,
        has_memory: bool,
    ) -> String {
        let mut init_memviews = String::new();
        for &(num, ref views) in self.memories.values() {
            for kind in views {
                writeln!(
                    init_memviews,
                    // Reset the memory views to null in case `init` gets called multiple times.
                    // Without this, the `length = 0` check would never detect that the view was
                    // outdated.
                    "cached{kind}Memory{num} = null;",
                )
                .unwrap()
            }
        }
        format!(
            "let wasmModule, wasmInstance, wasm;
            function __wbg_finalize_init(instance, module{init_stack_size_arg}) {{
                wasmInstance = instance;
                wasm = instance.exports;
                wasmModule = module;
                {init_memviews}{init_stack_size_check}{start}return wasm;
            }}

            async function __wbg_load(module, imports) {{
                if (typeof Response === 'function' && module instanceof Response) {{
                    if (!module.ok) {{
                        throw new Error(`failed to fetch Wasm: ${{module.status}} ${{module.statusText}} fetching '${{module.url}}'`);
                    }}

                    return await WebAssembly.instantiateStreaming(module, imports);
                }} else {{
                    const instance = await WebAssembly.instantiate(module, imports);

                    if (instance instanceof WebAssembly.Instance) {{
                        return {{ instance, module }};
                    }} else {{
                        return instance;
                    }}
                }}

            }}

            function initSync(module{init_memory_arg}) {{
                if (wasm !== undefined) return wasm;

                {init_stack_size}
                if (module !== undefined) {{
                    if (Object.getPrototypeOf(module) === Object.prototype) {{
                        ({{module{init_memory_arg}{init_stack_size_arg}}} = module)
                    }} else {{
                        console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
                    }}
                }}

                const imports = __wbg_get_imports({init_memory_arg_alone});
                if (!(module instanceof WebAssembly.Module)) {{
                    module = new WebAssembly.Module(module);
                }}
                const instance = new WebAssembly.Instance(module, imports);
                return __wbg_finalize_init(instance, module{init_stack_size_arg});
            }}

            async function __wbg_init(module_or_path{init_memory_arg}) {{
                if (wasm !== undefined) return wasm;

                {init_stack_size}
                if (module_or_path !== undefined) {{
                    if (Object.getPrototypeOf(module_or_path) === Object.prototype) {{
                        ({{module_or_path{init_memory_arg}{init_stack_size_arg}}} = module_or_path)
                    }} else {{
                        console.warn('using deprecated parameters for the initialization function; pass a single object instead')
                    }}
                }}

                {default_module_path}
                const imports = __wbg_get_imports({init_memory_arg_alone});

                if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {{
                    module_or_path = fetch(module_or_path);
                }}

                const {{ instance, module }} = await __wbg_load(await module_or_path, imports);

                return __wbg_finalize_init(instance, module{init_stack_size_arg});
            }}
            ",
            start = if needs_manual_start && self.threads_enabled {
                "wasm.__wbindgen_start(thread_stack_size);\n"
            } else if needs_manual_start {
                "wasm.__wbindgen_start();\n"
            } else {
                ""
            },
            init_stack_size = if self.threads_enabled {
                "let thread_stack_size"
            } else {
                ""
            },
            init_stack_size_arg = if self.threads_enabled {
                ", thread_stack_size"
            } else {
                ""
            },
            init_stack_size_check = if self.threads_enabled {
                format!(
                    "if (typeof thread_stack_size !== 'undefined' && (typeof thread_stack_size !== 'number' || thread_stack_size === 0 || thread_stack_size % {} !== 0)) {{
                        throw new Error('invalid stack size');
                    }}\n\n",
                    threads_xform::PAGE_SIZE,
                )
            } else {
                String::new()
            },
            init_memory_arg = if has_memory {
                ", memory"
            } else {
                ""
            },
            init_memory_arg_alone = if has_memory {
                "memory"
            } else {
                ""
            },
        )
    }

    fn generate_wasm_loading(
        &mut self,
        module_name: &str,
        needs_manual_start: bool,
        has_memory: bool,
    ) -> String {
        {
            let default_module_path = if self.config.omit_default_module_path {
                ""
            } else {
                &format!(
                    "if (module_or_path === undefined) {{
                            module_or_path = new URL('{module_name}_bg.wasm', import.meta.url);
                        }}"
                )
            };
            let mut loading =
                self.generate_web_loading(needs_manual_start, default_module_path, has_memory);

            loading.push_str("\nexport { initSync, __wbg_init as default };");

            loading
        }
    }

    /// Resolve a class name to the key used in `exported_classes`. Classes are
    /// keyed by their qualified JS identity, which is exactly what callers
    /// already hold (from WasmDescribe / `js_class`), so this is a direct
    /// passthrough. Kept as a single named choke point in case key
    /// normalization is needed again.
    pub(crate) fn resolve_class_name<'b>(&'b self, name: &'b str) -> &'b str {
        name
    }

    /// Populate `ancestors` and `participates_in_inheritance` on every
    /// `ExportedClass`. Walks the `extends` chain from each subclass up to
    /// its root ancestor; if a cycle or missing parent is encountered the
    /// chain is truncated there (the missing-parent error is reported later
    /// during `write_class` with better span info).
    fn populate_inheritance_chains(&mut self) {
        // Resolve a chain of `exported_classes` keys first (so cycle
        // detection works), then translate to `(identifier, qualified_name)`
        // pairs for emission in `resolved_chain` below.
        let mut updates: Vec<(String, Vec<String>)> = Vec::new();
        let keys: Vec<String> = self.exported_classes.keys().cloned().collect();
        for key in &keys {
            let mut chain: Vec<String> = Vec::new();
            let mut cursor = match self
                .exported_classes
                .get(key)
                .and_then(|c| c.extends.clone())
            {
                Some(p) => p,
                None => continue,
            };
            loop {
                // `extends` is stored as the parent's qualified JS identity
                // (the `exported_classes` key). Direct lookup hits the
                // parent's entry. The `js_name` fallback covers the edge
                // case where a parent was registered under a key that
                // differs from its current `js_name` (rare, defensive).
                let resolved_key = if self.exported_classes.contains_key(&cursor) {
                    cursor.clone()
                } else {
                    match self.exported_classes.iter().find_map(|(k, v)| {
                        if v.js_name.as_deref() == Some(cursor.as_str()) {
                            Some(k.clone())
                        } else {
                            None
                        }
                    }) {
                        Some(k) => k,
                        None => break,
                    }
                };
                if chain.contains(&resolved_key) || resolved_key == *key {
                    break;
                }
                chain.push(resolved_key.clone());
                cursor = match self
                    .exported_classes
                    .get(&resolved_key)
                    .and_then(|c| c.extends.clone())
                {
                    Some(p) => p,
                    None => break,
                };
            }
            if !chain.is_empty() {
                updates.push((key.clone(), chain));
            }
        }
        // Mark every subclass with its chain and flag participation on both
        // sides of every edge.
        for (child, chain) in &updates {
            // For each ancestor we keep two names:
            //   0. `identifier`: the class's auto-sanitized JS identifier,
            //      used as a `__wbg_ptr_<Ident>` property suffix and as a
            //      JS-side reference (always a valid JS ident).
            //   1. `qualified_name`: the `exported_classes` key, used for
            //      `__wbg_<...>_free` and `upcast_function` symbol names.
            //      Post-keying refactor (#5154) the map key *is* the
            //      qualified name, so `k` is used directly.
            let resolved_chain: Vec<(String, String)> = chain
                .iter()
                .map(|k| {
                    let c = self.exported_classes.get(k);
                    let id = c.map(|c| c.identifier.clone()).unwrap_or_else(|| k.clone());
                    (id, k.clone())
                })
                .collect();
            if let Some(c) = self.exported_classes.get_mut(child) {
                c.ancestors = resolved_chain;
                c.participates_in_inheritance = true;
            }
            for ancestor in chain {
                if let Some(c) = self.exported_classes.get_mut(ancestor) {
                    c.participates_in_inheritance = true;
                }
            }
        }
    }

    fn require_class<'b>(&'b mut self, name: &str) -> &'b mut ExportedClass {
        // `name` is the qualified JS identity (`<ns>__<js_name>`). Both the
        // struct registration and impl-block exports key `exported_classes`
        // by qualified_name, so the name is used directly as the key.
        let key = name.to_string();
        if self
            .exported_classes
            .get(&key)
            .is_none_or(|cls| cls.identifier.is_empty())
        {
            // The key is the qualified JS identity; use it directly to
            // seed the class's JS-side identifier.
            let identifier = self.generate_identifier(&key);
            let class = self.exported_classes.entry(key.clone()).or_default();
            class.identifier = identifier;
        }
        self.exported_classes.get_mut(&key).unwrap()
    }

    /// Verify that every impl-block class reference resolves to a
    /// registered struct. The encoded `class` string on each export is
    /// the qualified JS identity (`<ns>__<js_class>`), built from the
    /// impl block's own `js_class` + `js_namespace`. The struct
    /// registration produces the same qualified form from its `js_name`
    /// + `js_namespace`. Both must agree exactly.
    ///
    /// On mismatch we produce a targeted hint:
    ///
    ///   1. A struct exists with matching bare `js_name` but in a
    ///      different `js_namespace` — the user almost certainly forgot
    ///      to repeat `js_namespace` on the impl (or got it wrong).
    ///   2. No exact `js_name` match — fall back to fuzzy ranking across
    ///      all registered class identities. Catches typos in `js_class`
    ///      and surfaces the closest plausible candidate.
    ///
    /// Without this check the failure mode is silent: `require_class`
    /// would mint a fresh empty entry, the namespace export would
    /// resolve to a stub class, and the user only discovers the problem
    /// at runtime when `new wasm.ns.Foo(...)` throws.
    fn validate_impl_class_references(&self) -> Result<(), Error> {
        for aux in self.aux.export_map.values() {
            let class = match &aux.kind {
                AuxExportKind::Constructor(c) => c,
                AuxExportKind::Method { class, .. } => class,
                _ => continue,
            };
            if self.exported_classes.contains_key(class) {
                continue;
            }
            // Targeted hint: same `js_name`, different namespace. This is
            // the most common cause: impl missed `js_namespace` (or set
            // it to the wrong value), so its qualified identity differs
            // from the struct's. Strip the leading namespace from the
            // claimed `class` string before comparing against bare struct
            // `js_name`s.
            let bare = class.rsplit_once("__").map(|(_, n)| n).unwrap_or(class);
            let mismatched: Vec<(&AuxStruct, String)> = self
                .aux
                .structs
                .iter()
                .filter_map(|s| {
                    if s.name != bare {
                        return None;
                    }
                    let q = wasm_bindgen_shared::qualified_name(s.js_namespace.as_deref(), &s.name);
                    (q != *class).then_some((s, q))
                })
                .collect();
            if !mismatched.is_empty() {
                let hints = mismatched
                    .iter()
                    .map(|(s, q)| match &s.js_namespace {
                        Some(ns) => {
                            let ns = ns.join(", ");
                            format!("  - `js_namespace = {ns}` (matches struct `{q}`)")
                        }
                        None => format!("  - no `js_namespace` (matches struct `{}`)", s.name),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!(
                    "class `{class}` referenced by an impl block does not \
                     match any exported struct.\nhelp: a struct with the \
                     same `js_name` exists in a different namespace. The \
                     impl block must declare the matching `js_namespace` \
                     (the impl macro cannot see the struct's attributes \
                     cross-invocation):\n{hints}"
                );
            }
            // General fuzzy fallback: rank against all registered class
            // qualified names.
            let candidates: Vec<&str> = self.exported_classes.keys().map(|s| s.as_str()).collect();
            let hint = crate::suggest::suggest(class, candidates);
            bail!(
                "class `{class}` referenced by an impl block does not \
                 match any exported struct{hint}"
            );
        }
        Ok(())
    }

    fn write_classes(&mut self) -> Result<(), Error> {
        let exported_classes = std::mem::take(&mut self.exported_classes);
        // Resolve qualified names and unrenamed parent class names to JS identifiers.
        let class_identifiers: HashMap<String, String> = exported_classes
            .iter()
            .flat_map(|(key, cls)| {
                let entry = cls.identifier.clone();
                let mut entries = vec![(key.clone(), entry.clone())];
                if let Some(js) = &cls.js_name {
                    if js != key {
                        entries.push((js.clone(), entry));
                    }
                }
                entries
            })
            .collect();
        for (class, exports) in exported_classes {
            self.write_class(&class, exports, &class_identifiers)?;
        }
        Ok(())
    }

    fn write_class(
        &mut self,
        name: &str,
        class: ExportedClass,
        class_identifiers: &HashMap<String, String>,
    ) -> Result<(), Error> {
        let identifier = &class.identifier;
        // Use js_name for JS output, falling back to the key name.
        let js_name = class.js_name.as_deref().unwrap_or(name);
        // `exported_classes` is keyed by the qualified JS identity, so the
        // map key (`name`) is the qualified name used for wasm symbol
        // references.
        let qualified = name;
        // For every class in an `extends` chain we emit per-class
        // `__wbg_ptr_<Class>` fields alongside the unqualified `__wbg_ptr`
        // alias, so parent-method prototype dispatch can route through
        // the correct ancestor pointer. Resolve the ancestors' js_names
        // (field name) and qualified_names (wasm symbol base) once here.
        let participates = class.participates_in_inheritance;
        let ancestors: Vec<(String, String)> = class.ancestors.clone();
        // Resolve the parent class identifier for JavaScript inheritance.
        let parent_meta = match &class.extends {
            Some(parent_name) => {
                let meta = class_identifiers.get(parent_name).cloned().ok_or_else(|| {
                    // Fuzzy-rank against the set of registered class
                    // identifiers so the user gets a "did you mean ...?"
                    // hint when the parent name is misspelled or absent.
                    let candidates: Vec<&str> =
                        class_identifiers.keys().map(|s| s.as_str()).collect();
                    let hint = crate::suggest::suggest(parent_name, candidates);
                    anyhow::anyhow!(
                        "class `{identifier}` extends `{parent_name}`, but no \
                         exported Rust struct named `{parent_name}` was found \
                         in this module{hint}",
                    )
                })?;
                Some(meta)
            }
            None => None,
        };
        let parent_identifier = parent_meta;
        let mut dst = match &parent_identifier {
            Some(parent_id) => format!("class {identifier} extends {parent_id} {{\n"),
            None => format!("class {identifier} {{\n"),
        };

        if !class.has_constructor {
            // declare the constructor as private to prevent direct instantiation

            if self.config.debug {
                dst.push_str(
                    "\
                        constructor() {
                            throw new Error('cannot invoke `new` directly');
                        }
                    ",
                );
            }
        }

        if class.wrap_needed {
            let ptr_assignment = if participates {
                let mut body = String::new();
                body.push_str("obj.__wbg_ptr = ptr;\n");
                body.push_str(&format!("obj.__wbg_ptr_{identifier} = ptr;\n"));
                if self.generate_reinit {
                    body.push_str("Object.defineProperty(obj, '__wbg_inst', { value: __wbg_instance_id, writable: true });\n");
                }
                let mut prev_expr = String::from("ptr");
                let mut from_qualified = qualified.to_string();
                for (idx, (anc_id, anc_qualified)) in ancestors.iter().enumerate() {
                    // The macro emits the upcast symbol as
                    // `upcast_function(child_qualified_name, parent_qualified_name)`,
                    // matching the qualified_name keying of
                    // `exported_classes`. Both sides derive the parent
                    // half from `extends_js_class` + `extends_js_namespace`
                    // (or the defaulted-from-Rust-path equivalents).
                    let sym = wasm_bindgen_shared::upcast_function(&from_qualified, anc_qualified);
                    let sym = self.wasm_export_ref(&sym);
                    let tmp = format!("__wbg_anc_{idx}");
                    body.push_str(&format!("const {tmp} = {sym}({prev_expr}) >>> 0;\n"));
                    body.push_str(&format!("obj.__wbg_ptr_{anc_id} = {tmp};\n"));
                    prev_expr = tmp;
                    from_qualified = anc_qualified.clone();
                }
                body
            } else if self.generate_reinit {
                "obj.__wbg_ptr = ptr;\nObject.defineProperty(obj, '__wbg_inst', { value: __wbg_instance_id, writable: true });\n".to_string()
            } else {
                "obj.__wbg_ptr = ptr;".to_string()
            };

            let register_data = if participates {
                let mut parts: Vec<String> = Vec::new();
                parts.push(format!(
                    "__wbg_ptr_{identifier}: obj.__wbg_ptr_{identifier}"
                ));
                for (anc_id, _) in ancestors.iter() {
                    parts.push(format!("__wbg_ptr_{anc_id}: obj.__wbg_ptr_{anc_id}"));
                }
                if self.generate_reinit {
                    parts.push("instance: __wbg_instance_id".to_string());
                }
                format!("{{ {} }}", parts.join(", "))
            } else if self.generate_reinit {
                "{ ptr, instance: __wbg_instance_id }".to_string()
            } else {
                "obj.__wbg_ptr".to_string()
            };

            dst.push_str(&format!(
                "\
                static __wrap(ptr) {{
                    const obj = Object.create({identifier}.prototype);
                    {ptr_assignment}
                    {identifier}Finalization.register(obj, {register_data}, obj);
                    return obj;
                }}
                "
            ));
        }

        if class.unwrap_needed {
            // For inheritance-participating classes, a Rust descendant has
            // `__wbg_ptr` set to its own (descendant) pointer rather than
            // one compatible with this class's wasm shim, so consuming via
            // `__destroy_into_raw()` would hand the wrong pointer to Rust.
            // Reject by comparing the leaf `__wbg_ptr` against the per-class
            // slot `__wbg_ptr_<Class>`: for the actual class instance (or a
            // user-written JS-only subclass) those slots agree, so the
            // unwrap proceeds; for a Rust descendant they differ, so we
            // return 0 — Rust receives null, trips `assert_not_null`, and
            // throws cleanly.
            let check = if participates {
                format!("jsValue.__wbg_ptr !== jsValue.__wbg_ptr_{identifier}")
            } else {
                format!("!(jsValue instanceof {identifier})")
            };
            dst.push_str(&format!(
                "\
                static __unwrap(jsValue) {{
                    if ({check}) {{
                        return 0;
                    }}
                    return jsValue.__destroy_into_raw();
                }}
                ",
            ));
        }

        let finalization_callback = if participates {
            let mut body = String::new();
            body.push_str(&format!(
                "{}(tok.__wbg_ptr_{identifier} >>> 0, 1);\n",
                self.wasm_export_ref(&wasm_bindgen_shared::free_function(qualified))
            ));
            for (anc_id, anc_q) in ancestors.iter() {
                body.push_str(&format!(
                    "{}(tok.__wbg_ptr_{anc_id} >>> 0, 1);\n",
                    self.wasm_export_ref(&wasm_bindgen_shared::free_function(anc_q))
                ));
            }
            if self.generate_reinit {
                format!("(tok) => {{ if (tok.instance === __wbg_instance_id) {{ {body} }} }}")
            } else {
                format!("(tok) => {{ {body} }}")
            }
        } else if self.generate_reinit {
            format!(
                "({{ ptr, instance }}) => {{
                if (instance === __wbg_instance_id) {}(ptr, 1);
            }}",
                self.wasm_export_ref(&wasm_bindgen_shared::free_function(qualified))
            )
        } else {
            format!(
                "ptr => {}(ptr, 1)",
                self.wasm_export_ref(&wasm_bindgen_shared::free_function(qualified))
            )
        };

        // Hoisted classes need their finalization registry as a sibling library
        // symbol rather than a `$initBindgen` local.

        let finalization_value = format!("new FinalizationRegistry({finalization_callback})");
        {
            self.globals.push_str(&format!(
                "const {identifier}Finalization = {finalization_value};\n"
            ));
        }

        // If the class is inspectable, generate `toJSON` and `toString`
        // to expose all readable properties of the class. Otherwise,
        // the class shows only the "ptr" property when logged or serialized
        if class.is_inspectable {
            // Creates a `toJSON` method which returns an object of all readable properties
            // This object looks like { a: this.a, b: this.b }. Names that are
            // not valid identifiers, such as the `0` and `1` fields of a
            // tuple struct, are quoted: { "0": this["0"] }
            dst.push_str(&format!(
                "\
                toJSON() {{
                    return {{{}}};
                }}
                toString() {{
                    return JSON.stringify(this);
                }}
                ",
                class
                    .readable_properties
                    .iter()
                    .fold(String::from("\n"), |fields, field_name| {
                        if is_valid_ident(field_name) {
                            format!("{fields}{field_name}: this.{field_name},\n")
                        } else {
                            let key = format!("\"{}\"", escape_string(field_name));
                            format!("{fields}{key}: this[{key}],\n")
                        }
                    })
            ));
            // Also add definitions to the .d.ts file.
        }

        let mut free = format!(
            "{}(ptr, 0)",
            self.wasm_export_ref(&wasm_bindgen_shared::free_function(qualified))
        );
        free = binding::maybe_wrap_export_call(
            &free,
            if self.aux.wrapped_js_tag.is_some() {
                binding::ExportGuard::Abort
            } else if self.generate_reinit {
                binding::ExportGuard::GuardOnly
            } else {
                binding::ExportGuard::None
            },
        );
        let destroy_body = if participates {
            let mut body = String::new();
            body.push_str("const ptr = this.__wbg_ptr;\n");
            body.push_str("this.__wbg_ptr = 0;\n");
            body.push_str(&format!("this.__wbg_ptr_{identifier} = 0;\n"));
            // Release JS-held ancestor Rc clones (allow_delayed=1 — the
            // child's Rc<Parent> field may still be alive). Order doesn't
            // matter because refcounts commute.
            for (anc_id, anc_q) in ancestors.iter() {
                let anc_free = self.wasm_export_ref(&wasm_bindgen_shared::free_function(anc_q));
                body.push_str(&format!(
                    "const __anc_{anc_id} = this.__wbg_ptr_{anc_id};\n"
                ));
                body.push_str(&format!("this.__wbg_ptr_{anc_id} = 0;\n"));
                body.push_str(&format!(
                    "if (__anc_{anc_id} !== 0) {anc_free}(__anc_{anc_id} >>> 0, 1);\n"
                ));
            }
            body.push_str(&format!("{identifier}Finalization.unregister(this);\n"));
            body.push_str("return ptr;");
            body
        } else {
            format!(
                "const ptr = this.__wbg_ptr;\nthis.__wbg_ptr = 0;\n{identifier}Finalization.unregister(this);\nreturn ptr;"
            )
        };
        // For inheritance-participating classes, guard `free()` against
        // `Parent.prototype.free.call(child)` style cross-class dispatch.
        // The body uses `this.__destroy_into_raw()` (which dispatches via
        // the prototype chain to the descendant's `__destroy_into_raw`) and
        // hands the resulting pointer to *this* class's wasm free shim. If
        // `this` is a Rust descendant, that pointer is the descendant's
        // own-class pointer, which mismatches the wasm shim's expected
        // layout — type-confused free / UB. Reject by checking that
        // `this.__wbg_ptr` matches the per-class pointer slot for this
        // class; for the actual leaf instance (or a JS-only subclass that
        // doesn't add its own wasm-bindgen layer) those are identical, so
        // the check passes; for a Rust descendant, the per-class pointer
        // is the upcasted ancestor pointer and differs from `__wbg_ptr`,
        // so the call throws cleanly.
        let free_guard = if participates {
            format!(
                "if (this.__wbg_ptr !== this.__wbg_ptr_{identifier}) {{ \
                    throw new TypeError('{identifier}: free cannot be invoked through subclass prototype dispatch'); \
                }}\n                "
            )
        } else {
            String::new()
        };
        dst.push_str(&format!(
            "\
            __destroy_into_raw() {{
                {destroy_body}
            }}
            free() {{
                {free_guard}const ptr = this.__destroy_into_raw();
                {free}
            }}
            "
        ));

        dst.push_str(&class.contents);

        dst.push_str("}\n");

        let dispose_wiring = format!(
            "if (Symbol.dispose) {identifier}.prototype[Symbol.dispose] = {identifier}.prototype.free;"
        );

        dst.push_str(&dispose_wiring);
        dst.push('\n');

        define_export(
            &mut self.exports,
            js_name,
            class.js_namespace.as_deref().unwrap_or_default(),
            ExportEntry::Definition(ExportDefinition {
                identifier: class.identifier.clone(),
                comments: Some(class.comments),
                definition: dst,

                private: class.private,
                parent_identifier: parent_identifier.clone(),
            }),
        )?;

        Ok(())
    }

    fn write_exports(&mut self) -> Result<(), Error> {
        let exports = std::mem::take(&mut self.exports);
        // Reorder so a `class Child extends Parent { ... }` definition is
        // emitted after the `Parent` definition. BTreeMap iteration is by
        // export name, which can place the child first and trip the JS
        // class-declaration TDZ at module load (`Cannot access 'Parent'
        // before initialization`). Definitions without a parent dependency
        // (or whose parent isn't itself an export — e.g. an imported class)
        // keep their existing position relative to other independents.
        let exports = topo_sort_class_exports(exports);
        for (ref export_name, export) in exports {
            match export {
                ExportEntry::Definition(def) => {
                    if !def.private {
                        self.export_name_list.push(export_name.clone());
                    }
                    self.export_def(Some(export_name), &def);
                }
                ExportEntry::Namespace(ns) => {
                    let (identifier, existing) = match ns.id {
                        Some(id) => (id, true),
                        None => (self.generate_identifier(export_name), false),
                    };

                    let ns_dst = self.write_namespace(&identifier, &ns.ns, existing)?;

                    self.export_name_list.push(export_name.clone());
                    let definition = if !existing {
                        format!("const {identifier} = {{}};\n{ns_dst}")
                    } else {
                        self.global(&ns_dst);
                        "".to_string()
                    };

                    self.export_def(
                        Some(export_name),
                        &ExportDefinition {
                            comments: None,
                            definition,

                            identifier,
                            private: false,
                            parent_identifier: None,
                        },
                    );
                }
            }
        }
        Ok(())
    }

    fn write_namespace(
        &mut self,
        name: &str,
        namespace: &[(String, ExportEntry)],
        existing: bool,
    ) -> Result<String, Error> {
        let mut output = String::new();
        for (key, entry) in namespace {
            let full_name = if is_valid_ident(key) {
                format!("{name}.{key}")
            } else {
                format!("{name}['{key}']")
            };
            match entry {
                ExportEntry::Namespace(ns) => {
                    output.push_str(&format!(
                        "{full_name} {}= {{}};\n",
                        if existing { "||" } else { "" }
                    ));
                    output.push_str(&self.write_namespace(&full_name, &ns.ns, existing)?);
                }
                ExportEntry::Definition(def) => {
                    {
                        self.export_def(None, def);
                    }
                    output.push_str(&format!("{full_name} = {};\n", def.identifier));
                }
            }
        }
        Ok(output)
    }

    fn expose_drop_ref(&mut self) {
        // Note that here we check if `idx` shouldn't actually be dropped. This
        // is due to the fact that `JsValue::null()` and friends can be passed
        // by value to JS where we'll automatically call this method. Those
        // constants, however, cannot be dropped. See #1054 for removing this
        // branch.
        //
        // Otherwise the free operation here is pretty simple, just appending to
        // the linked list of heap slots that are free.
        self.expose_global_heap();
        self.expose_global_heap_next();
        self.intrinsic(
            "drop_ref".into(),
            format!(
                "
                function dropObject(idx) {{
                    if (idx < {}) return;
                    heap[idx] = heap_next;
                    heap_next = idx;
                }}
                ",
                INITIAL_HEAP_OFFSET + INITIAL_HEAP_VALUES.len(),
            )
            .into(),
        );
    }

    fn expose_global_heap(&mut self) {
        assert!(!self.config.externref);

        self.intrinsic(
            "heap".into(),
            format!(
                "
                let heap = new Array({INITIAL_HEAP_OFFSET}).fill(undefined);
                heap.push({});
                ",
                INITIAL_HEAP_VALUES.join(", ")
            )
            .into(),
        );
    }

    fn expose_global_heap_next(&mut self) {
        self.expose_global_heap();

        self.intrinsic(
            "heap_next".into(),
            "\nlet heap_next = heap.length;\n".into(),
        );
    }

    fn expose_get_object(&mut self) {
        // Accessing a heap object is just a simple index operation due to how
        // the stack/heap are laid out.
        self.expose_global_heap();
        self.intrinsic(
            "get_object".into(),
            "\nfunction getObject(idx) { return heap[idx]; }\n".into(),
        );
    }

    fn expose_not_defined(&mut self) {
        self.intrinsic("not_defined".into(),
            "\nfunction notDefined(what) { return () => { throw new Error(`${what} is not defined`); }; }\n".into(),

        );
    }

    fn expose_assert_num(&mut self) {
        self.intrinsic("assert_num".into(),
            "
            function _assertNum(n) {
                if (typeof(n) !== 'number') throw new Error(`expected a number argument, found ${typeof(n)}`);
            }
            ".into(),

        );
    }

    fn expose_assert_bigint(&mut self) {
        self.intrinsic("assert_bigint".into(),
            "
            function _assertBigInt(n) {
                if (typeof(n) !== 'bigint') throw new Error(`expected a bigint argument, found ${typeof(n)}`);
            }
            ".into(),

        );
    }

    fn expose_assert_bool(&mut self) {
        self.intrinsic(
            "assert_bool".into(),
            "
            function _assertBoolean(n) {
                if (typeof(n) !== 'boolean') {
                    throw new Error(`expected a boolean argument, found ${typeof(n)}`);
                }
            }
            "
            .into(),
        );
    }

    fn expose_wasm_vector_len(&mut self) {
        {
            self.intrinsic(
                "wasm_vector_len".into(),
                "\nlet WASM_VECTOR_LEN = 0;\n".into(),
            );
        }
    }

    fn expose_pass_string_to_wasm(&mut self, memory: MemoryId) -> MemView {
        self.expose_wasm_vector_len();

        let mem = self.expose_uint8_memory(memory);
        let ret = MemView {
            name: "passStringToWasm".into(),
            num: mem.num,
        };

        // Ensure the encoder and its polyfills are registered
        self.expose_text_encoder(memory);

        let mem_formatted = mem.access();

        let debug = if self.config.debug {
            "if (typeof(arg) !== 'string') throw new Error(`expected a string argument, found ${typeof(arg)}`);\n"
        } else {
            ""
        };
        let debug_end = if self.config.debug {
            "if (ret.read !== arg.length) throw new Error('failed to pass whole string');"
        } else {
            ""
        };

        // A fast path that directly writes char codes into Wasm memory as long
        // as it finds only ASCII characters.
        //
        // This is much faster for common ASCII strings because it can avoid
        // calling out into C++ TextEncoder code.
        //
        // This might be not very intuitive, but such calls are usually more
        // expensive in mainstream engines than staying in the JS, and
        // charCodeAt on ASCII strings is usually optimised to raw bytes.
        let ptr_coerce = self.coerce_ptr_suffix();
        let encode_as_ascii = format!(
            "\
                if (realloc === undefined) {{
                    const buf = cachedTextEncoder.encode(arg);
                    const ptr = malloc(buf.length, 1){ptr_coerce};
                    {mem_formatted}.subarray(ptr, ptr + buf.length).set(buf);
                    WASM_VECTOR_LEN = buf.length;
                    return ptr;
                }}

                let len = arg.length;
                let ptr = malloc(len, 1){ptr_coerce};

                const mem = {mem_formatted};

                let offset = 0;

                for (; offset < len; offset++) {{
                    const code = arg.charCodeAt(offset);
                    if (code > 0x7F) break;
                    mem[ptr + offset] = code;
                }}
            ",
        );

        let func_decl = format!(
            "
                function {ret}(arg, malloc, realloc) {{
                    {debug}{encode_as_ascii}if (offset !== len) {{
                        if (offset !== 0) {{
                            arg = arg.slice(offset);
                        }}
                        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1){ptr_coerce};
                        const view = {mem_formatted}.subarray(ptr + offset, ptr + len);
                        const ret = cachedTextEncoder.encodeInto(arg, view);
                        {debug_end}
                        offset += ret.written;
                        ptr = realloc(ptr, len, offset, 1){ptr_coerce};
                    }}

                    WASM_VECTOR_LEN = offset;
                    return ptr;
                }}
                ",
        );

        self.intrinsic(ret.to_string().into(), func_decl.into());

        ret
    }

    fn expose_pass_array8_to_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_uint8_memory(memory);
        self.pass_array_to_wasm("passArray8ToWasm", view, 1)
    }

    fn expose_pass_array16_to_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_uint16_memory(memory);
        self.pass_array_to_wasm("passArray16ToWasm", view, 2)
    }

    fn expose_pass_array32_to_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_uint32_memory(memory);
        self.pass_array_to_wasm("passArray32ToWasm", view, 4)
    }

    fn expose_pass_array64_to_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_uint64_memory(memory);
        self.pass_array_to_wasm("passArray64ToWasm", view, 8)
    }

    fn expose_pass_array_f32_to_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_f32_memory(memory);
        self.pass_array_to_wasm("passArrayF32ToWasm", view, 4)
    }

    fn expose_pass_array_f64_to_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_f64_memory(memory);
        self.pass_array_to_wasm("passArrayF64ToWasm", view, 8)
    }

    fn expose_pass_array_jsvalue_to_wasm(&mut self, memory: MemoryId) -> MemView {
        let mem = self.expose_dataview_memory(memory);
        let ret = MemView {
            name: "passArrayJsValueToWasm".into(),
            num: mem.num,
        };
        self.expose_wasm_vector_len();
        let mem_formatted: String = mem.access();
        let ptr_coerce = self.coerce_ptr_suffix();
        match (self.aux.externref_table, self.aux.externref_alloc) {
            (Some(table), Some(alloc)) => {
                // TODO: using `addToExternrefTable` goes back and forth between wasm
                // and JS a lot, we should have a bulk operation for this.
                let add = self.expose_add_to_externref_table(table, alloc);

                self.intrinsic(
                    ret.to_string().into(),
                    format!(
                        "
                        function {ret}(array, malloc) {{
                            const ptr = malloc(array.length * 4, 4){ptr_coerce};
                            for (let i = 0; i < array.length; i++) {{
                                const add = {add}(array[i]);
                                {mem_formatted}.setUint32(ptr + 4 * i, add, true);
                            }}
                            WASM_VECTOR_LEN = array.length;
                            return ptr;
                        }}
                    "
                    )
                    .into(),
                );
            }
            _ => {
                self.expose_add_heap_object();
                self.intrinsic(ret.to_string().into(), {
                    format!(
                        "
                        function {ret}(array, malloc) {{
                            const ptr = malloc(array.length * 4, 4){ptr_coerce};
                            const mem = {mem_formatted};
                            for (let i = 0; i < array.length; i++) {{
                                mem.setUint32(ptr + 4 * i, addHeapObject(array[i]), true);
                            }}
                            WASM_VECTOR_LEN = array.length;
                            return ptr;
                        }}
                    "
                    )
                    .into()
                });
            }
        }
        ret
    }

    fn pass_array_to_wasm(&mut self, name: &'static str, view: MemView, size: usize) -> MemView {
        let ret = MemView {
            name: name.into(),
            num: view.num,
        };
        self.expose_wasm_vector_len();
        let ptr_coerce = self.coerce_ptr_suffix();

        let view_access = view.access();
        let func_decl = format!(
            "
                function {ret}(arg, malloc) {{
                    const ptr = malloc(arg.length * {size}, {size}){ptr_coerce};
                    {view_access}.set(arg, ptr / {size});
                    WASM_VECTOR_LEN = arg.length;
                    return ptr;
                }}
                "
        );
        self.intrinsic(ret.to_string().into(), func_decl.into());
        ret
    }

    fn expose_text_encoder(&mut self, memory: MemoryId) {
        self.intrinsic("text_encoder".into(), {
            {
                let mut dst = Self::write_text_processor(
                    self.module,
                    memory,
                    "const",
                    "TextEncoder",
                    "()",
                    None,
                );

                // Encoding directly into shared Wasm memory is a separate
                // host data limitation, not a missing-method compatibility path.
                if self.module.memories.get(memory).shared {
                    dst.push_str(
                        "cachedTextEncoder.encodeInto = function (arg, view) {
                            const buf = cachedTextEncoder.encode(arg);
                            view.set(buf);
                            return { read: arg.length, written: buf.length };
                        };",
                    );
                }

                dst.into()
            }
        });
    }

    fn expose_text_decoder(&mut self, mem: &MemView, memory: MemoryId) {
        self.intrinsic("text_decoder".into(),  {
            // This is needed to workaround a bug in Safari
            // See: https://github.com/wasm-bindgen/wasm-bindgen/issues/1825
            let init = Some("cachedTextDecoder.decode();");

            // `ignoreBOM` is needed so that the BOM will be preserved when sending a string from Rust to JS
            // `fatal` is needed to catch any weird encoding bugs when sending a string from Rust to JS
            let mut dst = {
                Self::write_text_processor(
                    self.module,
                    memory,
                    "let",
                    "TextDecoder",
                    "('utf-8', { ignoreBOM: true, fatal: true })",
                    init,
                )
            };

            // Typically we try to give a raw view of memory out to `TextDecoder` to
            // avoid copying too much data. If, however, a `SharedArrayBuffer` is
            // being used it looks like that is rejected by `TextDecoder` or
            // otherwise doesn't work with it. When we detect a shared situation we
            // use `slice` which creates a new array instead of `subarray` which
            // creates just a view. That way in shared mode we copy more data but in
            // non-shared mode there's no need to copy the data except for the
            // string itself.
            let text_decoder_decode = {
                let is_shared = self.module.memories.get(memory).shared;
                let method = if is_shared { "slice" } else { "subarray" };
                format!("cachedTextDecoder.decode({}.{method}(ptr, ptr + len))", mem.access())
            };

            {
                    // For targets that can run in a browser, we need a workaround for the fact that
                    // (at least) Safari 16 to 18 has a TextDecoder that can't decode anymore after
                    // processing 2GiB of data. The workaround is that we keep track of how much the
                    // decoder has decoded and just create a new decoder when we're getting close to
                    // the limit.
                    // See MAX_SAFARI_DECODE_BYTES below for link to bug report.

                    // Maximum number of bytes Safari can handle for one TextDecoder is 2GiB (0x80000000 bytes)
                    // but empirically it seems to crash a bit before the end, so we remove 1MiB (0x100000 bytes)
                    // of margin.
                    // Workaround for a bug in Safari.
                    // See https://github.com/rustwasm/wasm-bindgen/issues/4471
                    const MAX_SAFARI_DECODE_BYTES: u32 = 0x80000000 - 0x100000;
                    dst.push_str(&format!(
                        "
                        const MAX_SAFARI_DECODE_BYTES = {MAX_SAFARI_DECODE_BYTES};
                        let numBytesDecoded = 0;
                        function decodeText(ptr, len) {{
                            numBytesDecoded += len;
                            if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {{
                                cachedTextDecoder = new TextDecoder('utf-8', {{ ignoreBOM: true, fatal: true }});
                                cachedTextDecoder.decode();
                                numBytesDecoded = len;
                            }}
                            return {text_decoder_decode};
                        }}
                        ",
                    ));
                }

            dst.into()
        }, )
    }

    fn write_text_processor(
        module: &walrus::Module,
        memory: MemoryId,
        decl_kind: &str,
        s: &str,
        args: &str,
        init: Option<&str>,
    ) -> String {
        let mut dst: String = String::new();

        // Audio worklets don't support `TextDe/Encoder`. When using audio worklets directly,
        // users will have to make sure themselves not to use any corresponding APIs. But
        // when spawning audio worklets, its fine to have `TextDe/Encoder` in a "normal worker"
        // while not using corresponding APIs in the audio worklet itself.
        if module.memories.get(memory).shared {
            dst.push_str(&format!(
                "{decl_kind} cached{s} = (typeof {s} !== 'undefined' ? new {s}{args} : undefined);\n"
            ));

            if let Some(init) = init {
                dst.push_str(&format!("if (cached{s}) {init}\n"));
            }
        } else {
            dst.push_str(&format!("{decl_kind} cached{s} = new {s}{args};\n"));

            if let Some(init) = init {
                dst.push_str(init);
            }
        }
        dst
    }

    fn expose_get_string_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let mem = self.expose_uint8_memory(memory);
        self.expose_text_decoder(&mem, memory);
        let ret = MemView {
            name: "getStringFromWasm".into(),
            num: mem.num,
        };
        let ptr_coerce = self.coerce_ptr_suffix();
        self.intrinsic(ret.to_string().into(), {
            format!(
                "
                function {ret}(ptr, len) {{
                    return decodeText(ptr{ptr_coerce}, len);
                }}
                ",
            )
            .into()
        });
        ret
    }

    fn expose_get_cached_string_from_wasm(
        &mut self,
        memory: MemoryId,
        table: Option<TableId>,
    ) -> MemView {
        let get_object = if let Some(table) = table {
            self.expose_get_from_externref_table(table).to_string()
        } else {
            self.expose_get_object();
            "getObject".to_string()
        };
        let get_string = self.expose_get_string_from_wasm(memory);
        let ret = MemView {
            name: "getCachedStringFromWasm".into(),
            num: get_string.num,
        };
        // This has support for both `&str` and `Option<&str>`.
        //
        // If `ptr` is not `0` then we know that it's a `&str` or `Some(&str)`, so we just decode it.
        //
        // If `ptr` is `0` then the `len` is a pointer to the cached `JsValue`, so we return that.
        //
        // If `ptr` and `len` are both `0` then that means it's `None`, in that case we rely upon
        // the fact that `getObject(0)` is guaranteed to be `undefined`.
        self.intrinsic(ret.to_string().into(), {
            format!(
                "
                function {ret}(ptr, len) {{
                    if (ptr === 0) {{
                        return {get_object}(len);
                    }} else {{
                        return {get_string}(ptr, len);
                    }}
                }}
                "
            )
            .into()
        });
        ret
    }

    fn expose_get_array_js_value_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let mem: MemView = self.expose_dataview_memory(memory);
        let ret = MemView {
            name: "getArrayJsValueFromWasm".into(),
            num: mem.num,
        };

        let mem_access = { format!("{mem}()") };
        match (self.aux.externref_table, self.aux.externref_drop_slice) {
            (Some(table), Some(drop)) => {
                let table = self.export_name_of(table);
                let table = self.wasm_export_ref(&table);
                let drop = self.export_name_of(drop);
                let drop = self.wasm_export_ref(&drop);
                let ptr_fixup = self.coerce_ptr_assign("ptr");
                self.intrinsic(ret.to_string().into(), {
                    format!(
                        "
                        function {ret}(ptr, len) {{
                            {ptr_fixup}
                            const mem = {mem_access};
                            const result = [];
                            for (let i = ptr; i < ptr + 4 * len; i += 4) {{
                                result.push({table}.get(mem.getUint32(i, true)));
                            }}
                            {drop}(ptr, len);
                            return result;
                        }}
                        ",
                    )
                    .into()
                });
            }
            _ => {
                self.expose_take_object();
                let ptr_fixup = self.coerce_ptr_assign("ptr");
                self.intrinsic(ret.to_string().into(), {
                    format!(
                        "
                        function {ret}(ptr, len) {{
                            {ptr_fixup}
                            const mem = {mem_access};
                            const result = [];
                            for (let i = ptr; i < ptr + 4 * len; i += 4) {{
                                result.push(takeObject(mem.getUint32(i, true)));
                            }}
                            return result;
                        }}
                        ",
                    )
                    .into()
                });
            }
        }
        ret
    }

    /// Like `expose_get_array_js_value_from_wasm` but for borrowed slices.
    /// Uses `getObject` instead of `takeObject` and doesn't drop the heap entries.
    fn expose_get_array_js_value_view_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let mem = self.expose_dataview_memory(memory);
        let ret = MemView {
            name: "getArrayJsValueViewFromWasm".into(),
            num: mem.num,
        };

        let mem_access = { format!("{mem}()") };
        match self.aux.externref_table {
            Some(table) => {
                let table = self.export_name_of(table);
                let table = self.wasm_export_ref(&table);
                let ptr_fixup = self.coerce_ptr_assign("ptr");
                self.intrinsic(ret.to_string().into(), {
                    format!(
                        "
                        function {ret}(ptr, len) {{
                            {ptr_fixup}
                            const mem = {mem_access};
                            const result = [];
                            for (let i = ptr; i < ptr + 4 * len; i += 4) {{
                                result.push({table}.get(mem.getUint32(i, true)));
                            }}
                            return result;
                        }}
                        ",
                    )
                    .into()
                });
            }
            _ => {
                self.expose_get_object();
                let ptr_fixup = self.coerce_ptr_assign("ptr");
                self.intrinsic(ret.to_string().into(), {
                    format!(
                        "
                        function {ret}(ptr, len) {{
                            {ptr_fixup}
                            const mem = {mem_access};
                            const result = [];
                            for (let i = ptr; i < ptr + 4 * len; i += 4) {{
                                result.push(getObject(mem.getUint32(i, true)));
                            }}
                            return result;
                        }}
                        ",
                    )
                    .into()
                });
            }
        }
        ret
    }

    fn expose_get_array_i8_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_int8_memory(memory);
        self.arrayget("getArrayI8FromWasm", view, 1)
    }

    fn expose_get_array_u8_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_uint8_memory(memory);
        self.arrayget("getArrayU8FromWasm", view, 1)
    }

    fn expose_get_clamped_array_u8_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_clamped_uint8_memory(memory);
        self.arrayget("getClampedArrayU8FromWasm", view, 1)
    }

    fn expose_get_array_i16_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_int16_memory(memory);
        self.arrayget("getArrayI16FromWasm", view, 2)
    }

    fn expose_get_array_u16_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_uint16_memory(memory);
        self.arrayget("getArrayU16FromWasm", view, 2)
    }

    fn expose_get_array_i32_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_int32_memory(memory);
        self.arrayget("getArrayI32FromWasm", view, 4)
    }

    fn expose_get_array_u32_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_uint32_memory(memory);
        self.arrayget("getArrayU32FromWasm", view, 4)
    }

    fn expose_get_array_i64_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_int64_memory(memory);
        self.arrayget("getArrayI64FromWasm", view, 8)
    }

    fn expose_get_array_u64_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_uint64_memory(memory);
        self.arrayget("getArrayU64FromWasm", view, 8)
    }

    fn expose_get_array_f32_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_f32_memory(memory);
        self.arrayget("getArrayF32FromWasm", view, 4)
    }

    fn expose_get_array_f64_from_wasm(&mut self, memory: MemoryId) -> MemView {
        let view = self.expose_f64_memory(memory);
        self.arrayget("getArrayF64FromWasm", view, 8)
    }

    fn arrayget(&mut self, name: &'static str, view: MemView, size: usize) -> MemView {
        let ret = MemView {
            name: name.into(),
            num: view.num,
        };
        let heap_view = view.access();
        let ptr_fixup = self.coerce_ptr_assign("ptr");
        self.intrinsic(
            name.into(),
            format!(
                "
                function {ret}(ptr, len) {{
                    {ptr_fixup}
                    return {heap_view}.subarray(ptr / {size}, ptr / {size} + len);
                }}
                ",
            )
            .into(),
        );
        ret
    }

    fn expose_int8_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("Int8Array", memory)
    }

    fn expose_uint8_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("Uint8Array", memory)
    }

    fn expose_clamped_uint8_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("Uint8ClampedArray", memory)
    }

    fn expose_int16_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("Int16Array", memory)
    }

    fn expose_uint16_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("Uint16Array", memory)
    }

    fn expose_int32_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("Int32Array", memory)
    }

    fn expose_uint32_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("Uint32Array", memory)
    }

    fn expose_int64_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("BigInt64Array", memory)
    }

    fn expose_uint64_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("BigUint64Array", memory)
    }

    fn expose_f32_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("Float32Array", memory)
    }

    fn expose_f64_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("Float64Array", memory)
    }

    fn expose_dataview_memory(&mut self, memory: MemoryId) -> MemView {
        self.memview("DataView", memory)
    }

    fn memview(&mut self, kind: &'static str, memory: walrus::MemoryId) -> MemView {
        let view = self.memview_memory(kind, memory);
        let mem = self.export_name_of(memory);
        let mem = self.wasm_export_ref(&mem);
        self.intrinsic(view.name.to_string().into(),  {
            let cache = format!("cached{kind}Memory{}", view.num);
            let resized_check = if self.module.memories.get(memory).shared {
                // When it's backed by a `SharedArrayBuffer`, growing the Wasm module's memory
                // doesn't detach old references; instead, it just leaves them pointing to a
                // slice of the up-to-date memory. So in order to check if it's been grown, we
                // have to compare it to the up-to-date buffer.
                format!("{cache}.buffer !== {mem}.buffer")
            } else if kind == "DataView" {
                // `DataView`s throw when accessing detached memory, including `byteLength`.
                // However this requires JS engine support, so we fallback to comparing the buffer.
                format!("{cache}.buffer.detached === true || ({cache}.buffer.detached === undefined && {cache}.buffer !== {mem}.buffer)")
            } else {
                // Otherwise, we can do a quicker check of whether the buffer's been detached,
                // which is indicated by a length of 0.
                format!("{cache}.byteLength === 0")
            };
            format!(
                "
                let {cache} = null;
                function {view}() {{
                    if ({cache} === null || {resized_check}) {{
                        {cache} = new {kind}({mem}.buffer);
                    }}
                    return {cache};
                }}
                ",
            )
            .into()
        }, );
        view
    }

    fn memview_memory(&mut self, kind: &'static str, memory: walrus::MemoryId) -> MemView {
        let next = self.memories.len();
        let &mut (num, ref mut kinds) = self
            .memories
            .entry(memory)
            .or_insert((next, Default::default()));
        kinds.insert(kind);
        {
            MemView {
                name: format!("get{kind}Memory").into(),
                num,
            }
        }
    }

    fn memview_table(&mut self, name: &'static str, table: walrus::TableId) -> MemView {
        let next = self.table_indices.len();
        let num = *self.table_indices.entry(table).or_insert(next);
        MemView {
            name: name.into(),
            num,
        }
    }

    fn expose_assert_class(&mut self) {
        self.intrinsic("assert_class".into(), {
            "
            function _assertClass(instance, klass) {
                if (!(instance instanceof klass)) {
                    throw new Error(`expected instance of ${klass.name}`);
                }
            }
            "
            .into()
        });
    }

    fn expose_global_stack_pointer(&mut self) {
        self.intrinsic(
            "stack_pointer".into(),
            format!("\nlet stack_pointer = {INITIAL_HEAP_OFFSET};\n").into(),
        );
    }

    fn expose_borrowed_objects(&mut self) {
        self.expose_global_heap();
        self.expose_global_stack_pointer();
        // Our `stack_pointer` points to where we should start writing stack
        // objects, and the `stack_pointer` is incremented in a `finally` block
        // after executing this. Once we've reserved stack space we write the
        // value. Eventually underflow will throw an exception, but JS sort of
        // just handles it today...
        self.intrinsic("borrowed_objects".into(), {
            "
            function addBorrowedObject(obj) {
                if (stack_pointer == 1) throw new Error('out of js stack');
                heap[--stack_pointer] = obj;
                return stack_pointer;
            }
            "
            .into()
        });
    }

    fn expose_take_object(&mut self) {
        self.expose_get_object();
        self.expose_drop_ref();
        self.intrinsic("take_object".into(), {
            "
            function takeObject(idx) {
                const ret = getObject(idx);
                dropObject(idx);
                return ret;
            }
            "
            .into()
        });
    }

    fn expose_add_heap_object(&mut self) {
        self.expose_global_heap();
        self.expose_global_heap_next();

        // Allocating a slot on the heap first goes through the linked list
        // (starting at `heap_next`). Once that linked list is exhausted we'll
        // be pointing beyond the end of the array, at which point we'll reserve
        // one more slot and use that.
        self.intrinsic("add_heap_object".into(), {
            format!(
                "
                function addHeapObject(obj) {{
                    if (heap_next === heap.length) heap.push(heap.length + 1);
                    const idx = heap_next;
                    heap_next = heap[idx];
                    {}
                    heap[idx] = obj;
                    return idx;
                }}
                ",
                if self.config.debug {
                    "if (typeof(heap_next) !== 'number') throw new Error('corrupt heap');"
                } else {
                    ""
                }
            )
            .into()
        });
    }

    fn expose_handle_error(&mut self) -> Result<(), Error> {
        if self
            .intrinsics
            .as_ref()
            .unwrap()
            .contains_key("handle_error")
        {
            return Ok(());
        }
        let store = self
            .aux
            .exn_store
            .ok_or_else(|| anyhow!("failed to find `__wbindgen_exn_store` intrinsic"))?;
        let store = self.export_name_of(store);
        let store = self.wasm_export_ref(&store);
        match (self.aux.externref_table, self.aux.externref_alloc) {
            (Some(table), Some(alloc)) => {
                let add = self.expose_add_to_externref_table(table, alloc);

                self.intrinsic("handle_error".into(), {
                    format!(
                        "
                        function handleError(f, args) {{
                            try {{
                                return f.apply(this, args);
                            }} catch (e) {{
                                const idx = {add}(e);
                                {store}(idx);
                            }}
                        }}
                        "
                    )
                    .into()
                });
            }
            _ => {
                self.expose_add_heap_object();

                self.intrinsic("handle_error".into(), {
                    format!(
                        "
                        function handleError(f, args) {{
                            try {{
                                return f.apply(this, args);
                            }} catch (e) {{
                                {store}(addHeapObject(e));
                            }}
                        }}
                        "
                    )
                    .into()
                });
            }
        }
        Ok(())
    }

    /// JSTag polyfill helper. Wraps a JS import call so that any caught
    /// exception is re-thrown as a `WebAssembly.Exception` against the
    /// polyfilled `__wbindgen_jstag`, restoring the engine behaviour that
    /// `WebAssembly.JSTag` provides natively on Node 22+.
    ///
    /// Already-wrapped exceptions (e.g. those thrown via `__wbindgen_throw` /
    /// `__wbindgen_rethrow` against `__wbindgen_wrapped_jstag`) are passed
    /// through untouched so the recoverable-vs-abort discrimination in
    /// `__wbg_handle_catch` keeps working.
    fn expose_wrap_error(&mut self) {
        self.intrinsic(
            "wrapError".into(),

            "
            function wrapError(f, args) {
                try {
                    return f.apply(this, args);
                } catch (e) {
                    if (e instanceof WebAssembly.Exception) throw e;
                    throw new WebAssembly.Exception(__wbindgen_jstag_polyfill, [e], { traceStack: true });
                }
            }
            "
            .into(),

        );
    }

    fn expose_log_error(&mut self) {
        self.intrinsic("log_error".into(), {
            "
            function logError(f, args) {
                try {
                    return f.apply(this, args);
                } catch (e) {
                    let error = (function () {
                        try {
                            return e instanceof Error \
                                ? `${e.message}\\n\\nStack:\\n${e.stack}` \
                                : e.toString();
                        } catch(_) {
                            return \"<failed to stringify thrown value>\";
                        }
                    }());
                    console.error(\"wasm-bindgen: imported JS function that \
                                    was not marked as `catch` threw an error:\", \
                                    error);
                    throw e;
                }
            }
            "
            .into()
        });
    }

    fn pass_to_wasm_function(&mut self, t: VectorKind, memory: MemoryId) -> MemView {
        match t {
            VectorKind::String => self.expose_pass_string_to_wasm(memory),
            VectorKind::I8 | VectorKind::U8 | VectorKind::ClampedU8 => {
                self.expose_pass_array8_to_wasm(memory)
            }
            VectorKind::U16 | VectorKind::I16 => self.expose_pass_array16_to_wasm(memory),
            VectorKind::I32 | VectorKind::U32 => self.expose_pass_array32_to_wasm(memory),
            VectorKind::I64 | VectorKind::U64 => self.expose_pass_array64_to_wasm(memory),
            VectorKind::F32 => self.expose_pass_array_f32_to_wasm(memory),
            VectorKind::F64 => self.expose_pass_array_f64_to_wasm(memory),
            VectorKind::Externref => self.expose_pass_array_jsvalue_to_wasm(memory),
            VectorKind::NamedExternref(_) => self.expose_pass_array_jsvalue_to_wasm(memory),
        }
    }

    fn expose_get_vector_from_wasm(&mut self, ty: VectorKind, memory: MemoryId) -> MemView {
        match ty {
            VectorKind::String => self.expose_get_string_from_wasm(memory),
            VectorKind::I8 => self.expose_get_array_i8_from_wasm(memory),
            VectorKind::U8 => self.expose_get_array_u8_from_wasm(memory),
            VectorKind::ClampedU8 => self.expose_get_clamped_array_u8_from_wasm(memory),
            VectorKind::I16 => self.expose_get_array_i16_from_wasm(memory),
            VectorKind::U16 => self.expose_get_array_u16_from_wasm(memory),
            VectorKind::I32 => self.expose_get_array_i32_from_wasm(memory),
            VectorKind::U32 => self.expose_get_array_u32_from_wasm(memory),
            VectorKind::I64 => self.expose_get_array_i64_from_wasm(memory),
            VectorKind::U64 => self.expose_get_array_u64_from_wasm(memory),
            VectorKind::F32 => self.expose_get_array_f32_from_wasm(memory),
            VectorKind::F64 => self.expose_get_array_f64_from_wasm(memory),
            VectorKind::Externref => self.expose_get_array_js_value_from_wasm(memory),
            VectorKind::NamedExternref(_) => self.expose_get_array_js_value_from_wasm(memory),
        }
    }

    fn expose_get_inherited_descriptor(&mut self) {
        // It looks like while rare some browsers will move descriptors up the
        // property chain which runs the risk of breaking wasm-bindgen-generated
        // code because we're looking for precise descriptor functions rather
        // than relying on the prototype chain like most "normal JS" projects
        // do.
        //
        // As a result we have a small helper here which will walk the prototype
        // chain looking for a descriptor. For some more information on this see
        // #109
        self.intrinsic("get_inherited_descriptor".into(), {
            "
                function GetOwnOrInheritedPropertyDescriptor(obj, id) {
                    while (obj) {
                        let desc = Object.getOwnPropertyDescriptor(obj, id);
                        if (desc) return desc;
                        obj = Object.getPrototypeOf(obj);
                    }
                    return {};
                }
                "
            .into()
        });
    }

    fn expose_is_like_none(&mut self) {
        self.intrinsic("is_like_none".into(), {
            "
            function isLikeNone(x) {
                return x === undefined || x === null;
            }
            "
            .into()
        });
    }

    fn expose_assert_non_null(&mut self) {
        self.intrinsic("assert_non_null".into(),  {
            "
            function _assertNonNull(n) {
                if (typeof(n) !== 'number' || n === 0) throw new Error(`expected a number argument that is not 0, found ${n}`);
            }
            ".into()
        }, );
    }

    fn expose_assert_char(&mut self) {
        self.intrinsic("assert_char".into(),  {
            "
            function _assertChar(c) {
                if (typeof(c) === 'number' && (c >= 0x110000 || (c >= 0xD800 && c < 0xE000))) throw new Error(`expected a valid Unicode scalar value, found ${c}`);
            }
            ".into()
        }, );
    }

    fn expose_make_mut_closure(&mut self) {
        let destroy_state = self.expose_closure_finalization();

        // For mutable closures they can't be invoked recursively.
        // To handle that we swap out the `this.a` pointer with zero
        // while we invoke it. If we finish and the closure wasn't
        // destroyed, then we put back the pointer so a future
        // invocation can succeed.
        self.intrinsic("make_mut_closure".into(), {
            let (state_init, instance_check) = if self.generate_reinit {
                (
                    "const state = { a: arg0, b: arg1, cnt: 1, instance: __wbg_instance_id };",
                    "
                    if (state.instance !== __wbg_instance_id) {
                        throw new Error('Cannot invoke closure from previous WASM instance');
                    }
                    ",
                )
            } else {
                ("const state = { a: arg0, b: arg1, cnt: 1 };", "")
            };
            format!(
                "
                function makeMutClosure(arg0, arg1, f) {{
                    {state_init}
                    const real = (...args) => {{
                        {instance_check}
                        // First up with a closure we increment the internal reference
                        // count. This ensures that the Rust closure environment won't
                        // be deallocated while we're invoking it.
                        state.cnt++;
                        const a = state.a;
                        state.a = 0;
                        try {{
                            return f(a, state.b, ...args);
                        }} finally {{
                            state.a = a;
                            real._wbg_cb_unref();
                        }}
                    }};
                    real._wbg_cb_unref = () => {{
                        if (--state.cnt === 0) {{
                            {destroy_state};
                            state.a = 0;
                            CLOSURE_DTORS.unregister(state);
                        }}
                    }};
                    CLOSURE_DTORS.register(real, state, state);
                    return real;
                }}
                "
            )
            .into()
        });
    }

    fn expose_make_closure(&mut self) {
        let destroy_state = self.expose_closure_finalization();

        // For shared closures they can be invoked recursively so we
        // just immediately pass through `this.a`. If we end up
        // executing the destructor, however, we clear out the
        // `this.a` pointer to prevent it being used again the
        // future.
        self.intrinsic("make_closure".into(), {
            let (state_init, instance_check) = if self.generate_reinit {
                (
                    "const state = { a: arg0, b: arg1, cnt: 1, instance: __wbg_instance_id };",
                    "
                    if (state.instance !== __wbg_instance_id) {
                        throw new Error('Cannot invoke closure from previous WASM instance');
                    }
                    ",
                )
            } else {
                ("const state = { a: arg0, b: arg1, cnt: 1 };", "")
            };
            format!(
                "
                function makeClosure(arg0, arg1, f) {{
                    {state_init}
                    const real = (...args) => {{
                        {instance_check}
                        // First up with a closure we increment the internal reference
                        // count. This ensures that the Rust closure environment won't
                        // be deallocated while we're invoking it.
                        state.cnt++;
                        try {{
                            return f(state.a, state.b, ...args);
                        }} finally {{
                            real._wbg_cb_unref();
                        }}
                    }};
                    real._wbg_cb_unref = () => {{
                        if (--state.cnt === 0) {{
                            {destroy_state};
                            state.a = 0;
                            CLOSURE_DTORS.unregister(state);
                        }}
                    }};
                    CLOSURE_DTORS.register(real, state, state);
                    return real;
                }}
                "
            )
            .into()
        });
    }

    /// Exposes the `CLOSURE_DTORS` finalization registry and returns a JS
    /// expression like `wasm.__wbindgen_destroy_closure(state.a, state.b)` for
    /// use in closure destructor code.
    fn expose_closure_finalization(&mut self) -> String {
        let func_id = self
            .aux
            .destroy_closure
            .expect("failed to find `__wbindgen_destroy_closure` intrinsic");
        let dtor = self.export_name_of(func_id);
        let dtor = self.wasm_export_ref(&dtor);
        let destroy_state = format!("{dtor}(state.a, state.b)");
        self.intrinsic("closure_finalization".into(), {
            let prevent_stale = if self.generate_reinit {
                format!(
                    "state => {{
                        if (state.instance === __wbg_instance_id) {{
                            {destroy_state};
                        }}
                    }}"
                )
            } else {
                format!("state => {destroy_state}")
            };

            {
                format!(
                    "
                    const CLOSURE_DTORS = new FinalizationRegistry({prevent_stale});
                    "
                )
            }
            .into()
        });

        destroy_state
    }

    fn expose_panic_error(&mut self) {
        self.intrinsic("panic_error".into(), {
            {
                "class PanicError extends Error {}
                Object.defineProperty(PanicError.prototype, 'name', {
                    value: 'PanicError',
                });
                "
                .into()
            }
        });
    }

    fn expose_reinit_scheduled(&mut self) {
        self.intrinsic(
            "reinit_scheduled".into(),
            "
            let __wbg_reinit_scheduled = false;
            "
            .into(),
        );
    }

    /// Emit the `__wbg_reset_state` function and instance-id tracking for the
    /// reinit lifecycle. Called when `schedule_reinit()` is used or
    /// `--experimental-reset-state-function` is passed. The function is private
    /// unless the CLI flag is set.
    fn generate_reinit_wrappers(&mut self) -> Result<(), Error> {
        self.global("let __wbg_instance_id = 0;");

        let mut reset_statements = Vec::new();
        reset_statements.push("__wbg_instance_id++;".to_string());

        for (num, kinds) in self.memories.values() {
            for kind in kinds {
                let memview_name = format!("get{kind}Memory");
                if self.has_intrinsic(memview_name.as_str()) {
                    reset_statements.push(format!("cached{kind}Memory{num} = null;"));
                }
            }
        }

        if self.has_intrinsic("text_decoder") {
            reset_statements.push(
                "if (typeof numBytesDecoded !== 'undefined') numBytesDecoded = 0;".to_string(),
            );
        }

        if self.has_intrinsic("wasm_vector_len") {
            reset_statements.push(
                "if (typeof WASM_VECTOR_LEN !== 'undefined') WASM_VECTOR_LEN = 0;".to_string(),
            );
        }

        if self.has_intrinsic("heap") {
            let mut heap_reset = format!(
                "\
                    if (typeof heap !== 'undefined') {{
                        heap = new Array({INITIAL_HEAP_OFFSET}).fill(undefined);
                        heap = heap.concat([{}]);
                ",
                INITIAL_HEAP_VALUES.join(", ")
            );

            if self.has_intrinsic("heap_next") {
                heap_reset.push_str(
                    "\
                        if (typeof heap_next !== 'undefined')
                            heap_next = heap.length;
                    ",
                );
            }

            if self.has_intrinsic("stack_pointer") {
                heap_reset.push_str(&format!(
                    "\
                        if (typeof stack_pointer !== 'undefined')
                            stack_pointer = {INITIAL_HEAP_OFFSET};
                    "
                ));
            }

            heap_reset.push('}');
            reset_statements.push(heap_reset);
        }

        let has_catch_handler = self.aux.wrapped_js_tag.is_some();
        let abort_reset = if has_catch_handler {
            "\
            __wbg_called_abort = false;
            __wbg_reinit_scheduled = false;"
        } else {
            "__wbg_reinit_scheduled = false;"
        };
        reset_statements.push(format!(
            "{abort_reset}
            wasmInstance = new WebAssembly.Instance(wasmModule, __wbg_get_imports());
            wasm = wasmInstance.exports;
            wasm.__wbindgen_start();
            "
        ));

        let function_body = format!("() {{\n{}}}", reset_statements.join("\n"));

        let identifier = self.generate_identifier("__wbg_reset_state");
        let definition = format!("function {identifier} {function_body}\n");
        define_export(
            &mut self.exports,
            "__wbg_reset_state",
            &[],
            ExportEntry::Definition(ExportDefinition {
                comments: None,
                identifier,
                definition,

                // Only publicly exported when --experimental-reset-state-function is passed
                private: !self.config.generate_reset_state,
                parent_identifier: None,
            }),
        )?;

        Ok(())
    }

    fn global(&mut self, s: &str) {
        let s = s.trim();

        // Ensure a blank line between adjacent items, and ensure everything is
        // terminated with a newline.
        while !self.globals.ends_with("\n\n\n") && !self.globals.ends_with("*/\n") {
            self.globals.push('\n');
        }
        self.globals.push_str(s);
        self.globals.push('\n');
    }

    /// Gets the JS identifier for a class, which may be aliased if the original
    /// name conflicts with a JS builtin (e.g., `Array` -> `Array2`).
    pub fn require_class_identifier(&mut self, name: &str) -> String {
        self.require_class(name).identifier.clone()
    }

    fn require_class_wrap(&mut self, name: &str) -> String {
        let cls = self.require_class(name);
        cls.wrap_needed = true;
        cls.identifier.clone()
    }

    fn require_class_unwrap(&mut self, name: &str) -> String {
        let cls = self.require_class(name);
        cls.unwrap_needed = true;
        cls.identifier.clone()
    }

    fn add_module_import(&mut self, module: String, name: &str, actual: &str) {
        let rename = if name == actual {
            None
        } else {
            Some(actual.to_string())
        };
        self.js_imports
            .entry(module)
            .or_default()
            .push((name.to_string(), rename));
    }

    fn import_name(&mut self, import: &JsImport) -> Result<String, Error> {
        if let Some(name) = self.imported_names.get(&import.name) {
            let mut name = name.clone();
            for field in import.fields.iter() {
                name.push_str(&property_accessor(field));
            }
            return Ok(name.clone());
        }

        let mut name = match &import.name {
            JsImportName::Module { module, name } => {
                let unique_name = self.generate_identifier(name);
                self.add_module_import(module.clone(), name, &unique_name);
                unique_name
            }

            JsImportName::LocalModule { module, name } => {
                let unique_name = self.generate_identifier(name);
                let module = self.config.local_module_name(module);
                self.add_module_import(module, name, &unique_name);
                unique_name
            }

            JsImportName::InlineJs {
                unique_crate_identifier,
                snippet_idx_in_crate,
                name,
            } => {
                let module = self
                    .config
                    .inline_js_module_name(unique_crate_identifier, *snippet_idx_in_crate);
                let unique_name = self.generate_identifier(name);
                self.add_module_import(module, name, &unique_name);
                unique_name
            }

            JsImportName::VendorPrefixed { name, prefixes } => {
                self.imports_post.push_str("const l");
                self.imports_post.push_str(name);
                self.imports_post.push_str(" = ");
                switch(&mut self.imports_post, name, "", prefixes);
                self.imports_post.push_str(";\n");

                fn switch(dst: &mut String, name: &str, prefix: &str, left: &[String]) {
                    dst.push_str("(typeof ");
                    dst.push_str(prefix);
                    dst.push_str(name);
                    dst.push_str(" !== 'undefined' ? ");
                    dst.push_str(prefix);
                    dst.push_str(name);
                    dst.push_str(" : ");
                    if left.is_empty() {
                        dst.push_str("undefined");
                    } else {
                        switch(dst, name, &left[0], &left[1..]);
                    }
                    dst.push(')');
                }
                format!("l{name}")
            }

            JsImportName::Global { name } => {
                // Just register the name for collision detection without modifying it.
                // We should implement separate local / external name handling here in due course
                // and then just use generate_identifier, but for now this retains backwards compat.
                let cnt = self
                    .defined_identifiers
                    .entry(name.to_string())
                    .or_insert(0);
                *cnt += 1;
                if *cnt > 1 {
                    bail!("cannot import `{name}` from two locations");
                }
                name.to_string()
            }
        };
        self.imported_names
            .insert(import.name.clone(), name.clone());

        // After we've got an actual name handle field projections
        for field in import.fields.iter() {
            name.push_str(&property_accessor(field));
        }
        Ok(name)
    }

    fn import_static(&mut self, import: &JsImport, optional: bool) -> Result<String, Error> {
        let mut name = self.import_name(&JsImport {
            name: import.name.clone(),
            fields: Vec::new(),
        })?;

        // After we've got an actual name handle field projections
        if optional {
            name = format!("typeof {name} === 'undefined' ? null : {name}");

            for field in import.fields.iter() {
                let access = property_accessor(field);
                // `.foo` -> `?.foo`; `[..]` -> `?.[..]`.
                if access.starts_with('.') {
                    name.push('?');
                    name.push_str(&access);
                } else {
                    name.push_str("?.");
                    name.push_str(&access);
                }
            }
        } else {
            for field in import.fields.iter() {
                name.push_str(&property_accessor(field));
            }
        }

        Ok(name)
    }

    fn expose_get_from_externref_table(&mut self, table: TableId) -> MemView {
        let view = self.memview_table("getFromExternrefTable", table);
        assert!(self.config.externref);
        let table = self.export_name_of(table);
        let table = self.wasm_export_ref(&table);
        self.intrinsic(
            view.to_string().into(),
            format!("\nfunction {view}(idx) {{ return {table}.get(idx); }}\n").into(),
        );
        view
    }

    fn expose_take_from_externref_table(&mut self, table: TableId, drop: FunctionId) -> MemView {
        let view = self.memview_table("takeFromExternrefTable", table);
        assert!(self.config.externref);
        let drop = self.export_name_of(drop);
        let drop = self.wasm_export_ref(&drop);
        let table = self.export_name_of(table);
        let table = self.wasm_export_ref(&table);
        self.intrinsic(view.to_string().into(), {
            format!(
                "
                function {view}(idx) {{
                    const value = {table}.get(idx);
                    {drop}(idx);
                    return value;
                }}
            ",
            )
            .into()
        });

        view
    }

    fn expose_add_to_externref_table(&mut self, table: TableId, alloc: FunctionId) -> MemView {
        let view = self.memview_table("addToExternrefTable", table);
        assert!(self.config.externref);
        let alloc = self.export_name_of(alloc);
        let alloc = self.wasm_export_ref(&alloc);
        let table = self.export_name_of(table);
        let table = self.wasm_export_ref(&table);

        self.intrinsic(view.to_string().into(), {
            format!(
                "
                function {view}(obj) {{
                    const idx = {alloc}();
                    {table}.set(idx, obj);
                    return idx;
                }}
                ",
            )
            .into()
        });
        view
    }

    pub fn generate(&mut self) -> Result<(), Error> {
        self.prestore_global_import_identifiers()?;

        // Set up qualified-name mappings before processing adapters, so that
        // WasmDescribe lookups can resolve to the right exported class and TS
        // declaration identifier.
        // `exported_classes` is keyed by `qualified_name` — the struct's
        // JS-side identity (namespace-qualified `js_name`). Both struct
        // registration and impl-block exports must agree on this key, and
        // both derive it locally from `(js_namespace, js_name)` /
        // `(js_namespace, js_class)`, defaulting to the Rust ident when
        // unset so the no-rename case lines up without cross-invocation
        // knowledge. Keying by `rust_name` would let two structs sharing
        // a Rust ident in different modules (with distinct `js_name`s)
        // clobber each other.
        let mut any_class_has_parent = false;
        for s in self.aux.structs.iter() {
            let qualified_name =
                wasm_bindgen_shared::qualified_name(s.js_namespace.as_deref(), &s.name);
            let needs_identifier = self
                .exported_classes
                .get(&qualified_name)
                .is_none_or(|class| class.identifier.is_empty());
            let identifier = if needs_identifier {
                Some(self.generate_identifier(&qualified_name))
            } else {
                None
            };
            let class = self
                .exported_classes
                .entry(qualified_name.clone())
                .or_default();
            class.js_name = Some(s.name.clone());
            // Pre-populate extends so the constructor binding (emitted while
            // processing adapters below) can tell whether a class is a
            // subclass and needs `super(__wbgSuperSkip)`. The same field is
            // repopulated later by `generate_struct` for the JS `extends`
            // clause, but that runs after adapter processing. Store the
            // parent's qualified JS identity so downstream lookups against
            // `exported_classes` (keyed by qualified_name) resolve.
            class.extends = parent_qualified_name(s);
            if s.extends.is_some() {
                any_class_has_parent = true;
            }
            if let Some(identifier) = identifier {
                class.identifier = identifier;
            }
            self.qualified_to_identifier
                .insert(qualified_name, class.identifier.clone());
        }
        // Validate that every impl-block export references a class that
        // was registered by a struct macro. The qualified form of `class`
        // (built from the impl's `js_namespace` + `js_class`) must match a
        // struct's `qualified_name`. The common ways this fails:
        //
        //   * impl block forgets `js_namespace` that the struct declares
        //     — the symptoms used to be silent (stub class, no methods)
        //     because `require_class` would mint a fresh empty entry
        //   * impl block declares a different `js_namespace` than the
        //     struct — same outcome
        //   * typo in `js_class` value
        //
        // We surface a targeted hint for the namespace-mismatch case
        // (matching `name` in a different namespace) and fall back to a
        // fuzzy "did you mean" search across all registered struct
        // qualified names.
        self.validate_impl_class_references()?;

        // Emit the super-skip sentinel once up front if any class in this
        // module extends another. Every user-defined constructor then checks
        // for the sentinel as the first thing in its body so that a
        // subclass's `super(__wbgSuperSkip)` short-circuits the parent
        // ctor body and doesn't allocate an orphan parent instance.
        if any_class_has_parent {
            self.expose_super_skip_sentinel();
        }
        // Resolve the full ancestor chain for every class that participates
        // in inheritance (i.e. declares `extends` or is extended by another
        // class). Each class's `__wbg_ptr_<Class>` per-ancestor pointer
        // scheme is keyed on this list; ordering is nearest-first
        // (direct parent at index 0).
        self.populate_inheritance_chains();
        for e in self.aux.enums.values() {
            let qualified_name =
                wasm_bindgen_shared::qualified_name(e.js_namespace.as_deref(), &e.name);
            self.get_or_create_identifier(&qualified_name);
        }

        self.generate_jstag_import();
        self.generate_wrapped_jstag_import()?;
        self.generate_rethrow_critical_import();
        self.maybe_generate_call_guard()?;

        for (id, adapter, kind) in iter_adapter(self.aux, self.wit, self.module) {
            let instrs = match &adapter.kind {
                AdapterKind::Import { .. } => continue,
                AdapterKind::Local { instructions } => instructions,
            };
            self.generate_adapter(id, adapter, instrs, kind)?;
        }

        // Ensure all imports for reexports are defined
        for js_import in self.aux.reexports.values() {
            self.import_name(js_import)?;
        }

        let mut pairs = self.aux.export_map.iter().collect::<Vec<_>>();
        pairs.sort_by_key(|(k, _)| *k);
        check_duplicated_getter_and_setter_names(&pairs)?;

        for (_, e) in crate::sorted_iter(&self.aux.enums) {
            self.generate_enum(e)?;
        }
        for (_, e) in crate::sorted_iter(&self.aux.string_enums) {
            self.generate_string_enum(e)?;
        }
        for s in self.aux.structs.iter() {
            self.generate_struct(s)?;
        }

        for path in self.aux.package_jsons.iter() {
            self.process_package_json(path)?;
        }

        self.export_destructor();

        // Generate reinit wrappers (__wbg_reset_state + instance-id tracking)
        // when schedule_reinit() is used or --experimental-reset-state-function
        // is set. Must come last to ensure it knows about all other state.
        if self.generate_reinit {
            self.generate_reinit_wrappers()?;
        }

        Ok(())
    }

    fn export_destructor(&mut self) {
        let thread_destroy = match self.aux.thread_destroy {
            Some(id) => id,
            None => return,
        };

        self.export_name_of(thread_destroy);
    }

    /// Generate the import for `WebAssembly.JSTag` if it was used.
    ///
    /// The tag import is created by the catch-wrapper transform and/or the
    /// JSPI transform's rejection protocol, so it is located by name rather
    /// than through `aux.js_tag` — that field means "catch imports use wasm
    /// catch wrappers" and is only set by the catch-wrapper transform.
    fn generate_jstag_import(&mut self) {
        let import_id = self.module.imports.iter().find_map(|import| {
            let walrus::ImportKind::Tag(_) = import.kind else {
                return None;
            };
            if import.module == PLACEHOLDER_MODULE && import.name == "__wbindgen_jstag" {
                Some(import.id())
            } else {
                None
            }
        });

        let Some(id) = import_id else {
            return;
        };

        if self.aux.legacy_exception_handling {
            self.global(
                "const __wbindgen_jstag_polyfill = new WebAssembly.Tag({ parameters: ['externref'] });",
            );
            self.wasm_import_definitions.insert(
                id,
                ImportDefinition::GlobalRef("__wbindgen_jstag_polyfill".to_string()),
            );
        } else {
            self.wasm_import_definitions.insert(
                id,
                ImportDefinition::Expression("WebAssembly.JSTag".to_string()),
            );
        }
    }

    /// Generate the import for the wrapped JSTag if it was used (abort-reinit mode).
    fn generate_wrapped_jstag_import(&mut self) -> Result<(), Error> {
        let Some(wrapped_js_tag) = self.aux.wrapped_js_tag else {
            return Ok(());
        };

        // Find the import ID for the wrapped JSTag
        let import_id = self.module.imports.iter().find_map(|import| {
            let walrus::ImportKind::Tag(tag_id) = import.kind else {
                return None;
            };
            if tag_id == wrapped_js_tag {
                Some(import.id())
            } else {
                None
            }
        });

        // Create a top-level constant for the wrapped tag
        self.global(
            "const __wbindgen_wrapped_jstag = new WebAssembly.Tag({ parameters: ['externref'] });",
        );

        if let Some(id) = import_id {
            // Use the constant for the import
            self.wasm_import_definitions.insert(
                id,
                ImportDefinition::GlobalRef("__wbindgen_wrapped_jstag".to_string()),
            );
        };

        Ok(())
    }

    /// Generate the import for `__wbindgen_rethrow_critical` if it was used.
    fn generate_rethrow_critical_import(&mut self) {
        let Some(rethrow_critical) = self.aux.rethrow_critical else {
            return;
        };

        // Find the import ID for the host function.
        let import_id = self.module.imports.iter().find_map(|import| {
            let walrus::ImportKind::Function(func_id) = import.kind else {
                return None;
            };
            if func_id == rethrow_critical {
                Some(import.id())
            } else {
                None
            }
        });

        let Some(id) = import_id else {
            return;
        };

        self.wasm_import_definitions.insert(
            id,
            ImportDefinition::Function(
                "(cause) { throw new Error(\"Critical error\", { cause }); }".to_string(),
            ),
        );
    }

    /// Emit a `__wbg_call_guard`, handling hard aborts and reinitialization
    /// - Hard aborts are emitted when we are using js exception tagging.
    /// - For reinitialization, checks __wbg_reinit_scheduled.
    /// - If neither features are required, no call guard is emitted.
    fn maybe_generate_call_guard(&mut self) -> Result<(), Error> {
        // No call guard needed when we dont have hard aborts or reinit
        if self.aux.wrapped_js_tag.is_none() && !self.generate_reinit {
            return Ok(());
        }

        let mut termination_guard = String::from("function __wbg_call_guard() {");
        // Exception handling tags -> hard aborts
        if self.aux.wrapped_js_tag.is_some() || self.config.force_enable_abort_handler {
            let memory = get_memory(self.module).unwrap();
            let mem_view = self.expose_int32_memory(memory);
            let table = self.export_function_table()?;
            let table = self.wasm_export_ref(&table);
            let abort_handler = self.wasm_export_ref("__abort_handler");

            self.global(&format!(
                "
                let __wbg_terminated_addr;
                let __wbg_called_abort = false;
                function __wbg_call_abort_hook() {{
                    __wbg_called_abort = true;
                    try {{
                        const idx = {mem_view}()[{abort_handler}.value / 4];
                        if (idx) {table}.get(idx)();
                    }} catch(_) {{}}
                }}

                function __wbg_handle_catch(e) {{
                    if (e instanceof WebAssembly.Exception && e.is(__wbindgen_wrapped_jstag)) {{
                        throw e.getArg(__wbindgen_wrapped_jstag, 0);
                    }}
                    {mem_view}()[__wbg_terminated_addr] = 1;
                    __wbg_call_abort_hook();
                    throw e;
                }}
                "
            ));

            let instance_terminated = self.wasm_export_ref("__instance_terminated");
            termination_guard.push_str(&format!(
                "
                __wbg_terminated_addr ??= {instance_terminated}.value / 4;
                const flag = {mem_view}()[__wbg_terminated_addr];
                if (flag) {{
                    if (!__wbg_called_abort) {{
                        __wbg_call_abort_hook();
                    }}"
            ));
        }
        // reinit guard
        if self.generate_reinit {
            self.expose_reinit_scheduled();
            termination_guard.push_str(
                "
                if (__wbg_reinit_scheduled) {
                    __wbg_reset_state();
                    return;
                }",
            );
        }
        if self.aux.wrapped_js_tag.is_some() {
            termination_guard.push_str("throw new Error('Module terminated');\n");
            if self.generate_reinit {
                termination_guard.push_str(
                    "
                    } else if (__wbg_reinit_scheduled) {
                        __wbg_reset_state();
                    }",
                );
            } else {
                termination_guard.push('}');
            }
        }
        termination_guard.push_str("\n}");
        self.global(&termination_guard);
        Ok(())
    }

    /// Registers import names for all `Global` imports first before we actually
    /// process any adapters.
    ///
    /// `Global` names must be imported as their exact name, so if the same name
    /// from a global is also imported from a module we have to be sure to
    /// import the global first to ensure we don't shadow the actual global
    /// value. Otherwise we have no way of accessing the global value!
    ///
    /// This function will iterate through the import map up-front and generate
    /// a cache entry for each import name which is a `Global`.
    fn prestore_global_import_identifiers(&mut self) -> Result<(), Error> {
        for import in self.aux.import_map.values() {
            let js = match import {
                AuxImport::Value(AuxValue::Bare(js))
                | AuxImport::Value(AuxValue::ClassGetter(js, ..))
                | AuxImport::Value(AuxValue::Getter(js, ..))
                | AuxImport::Value(AuxValue::ClassSetter(js, ..))
                | AuxImport::Value(AuxValue::Setter(js, ..))
                | AuxImport::ValueWithThis(js, ..)
                | AuxImport::Instanceof(js)
                | AuxImport::Static { js, .. }
                | AuxImport::StructuralClassGetter(js, ..)
                | AuxImport::StructuralClassSetter(js, ..)
                | AuxImport::IndexingGetterOfClass(js)
                | AuxImport::IndexingSetterOfClass(js)
                | AuxImport::IndexingDeleterOfClass(js) => js,
                _ => continue,
            };
            if let JsImportName::Global { .. } = js.name {
                self.import_name(js)?;
            }
        }
        Ok(())
    }

    fn generate_adapter(
        &mut self,
        id: AdapterId,
        adapter: &Adapter,
        instrs: &[InstructionData],
        kind: ContextAdapterKind,
    ) -> Result<(), Error> {
        let catch = self.aux.imports_with_catch.contains(&id);
        let needs_jstag_wrap =
            self.aux.legacy_exception_handling && matches!(kind, ContextAdapterKind::Import(_));
        // Compute before builder borrows self mutably.
        let is_suspending = matches!(kind, ContextAdapterKind::Import(_))
            && self.aux.imports_with_suspending.contains(&id);

        if let ContextAdapterKind::Import(core) = kind {
            // Suspending imports must never take the direct-import path: they
            // need the `WebAssembly.Suspending` wrapper emitted below.
            if !catch
                && !needs_jstag_wrap
                && !is_suspending
                && self.attempt_direct_import(core, instrs)?
            {
                return Ok(());
            }
        }
        // Construct a JS shim builder, and configure it based on the kind of
        // export that we're generating.
        let mut builder = binding::Builder::new(self);
        builder.log_error(match kind {
            ContextAdapterKind::Export(_) | ContextAdapterKind::Adapter => false,
            ContextAdapterKind::Import(_) => builder.cx.config.debug,
        });
        builder.catch(catch);
        let mut args = &None;
        let mut asyncness = false;
        let mut jspi = false;
        let mut variadic = false;
        let mut generate_jsdoc = false;
        let mut ret_ty_override = &None;
        let mut ret_desc = &None;
        match kind {
            ContextAdapterKind::Export(export) => {
                args = &export.args;
                asyncness = export.asyncness;
                jspi = export.jspi;
                variadic = export.variadic;
                generate_jsdoc = export.generate_jsdoc;
                ret_ty_override = &export.fn_ret_ty_override;
                ret_desc = &export.fn_ret_desc;
                match &export.kind {
                    AuxExportKind::Function(_) => {}
                    AuxExportKind::FunctionThis(_) => {
                        builder.classless_this();
                    }
                    AuxExportKind::Constructor(class) => builder.constructor(class),
                    AuxExportKind::Method {
                        receiver, class, ..
                    } => match receiver {
                        AuxReceiverKind::None => {}
                        AuxReceiverKind::Borrowed => {
                            builder.method_class(class);
                            builder.method(false);
                        }
                        AuxReceiverKind::Owned => {
                            builder.method_class(class);
                            builder.method(true);
                        }
                    },
                }
            }
            ContextAdapterKind::Import(_) => {}
            ContextAdapterKind::Adapter => {}
        }

        // an internal debug name to help with error messages
        let debug_name = match kind {
            ContextAdapterKind::Import(i) => {
                let i = builder.cx.module.imports.get(i);
                format!("import of `{}::{}`", i.module, i.name)
            }
            ContextAdapterKind::Export(e) => format!("`{}`", e.debug_name),
            ContextAdapterKind::Adapter => format!("adapter {}", id.0),
        };
        // Set the JSPI flag on the context so `Invocation::Core::generate()`
        // wraps the raw wasm call with `WebAssembly.promising`. Only *sync*
        // jspi exports are promising-wrapped: an async jspi export keeps the
        // plain async-export JS contract (its activation only roots the JSPI
        // context in-wasm, via the fiber wrapper the jspi transform applies
        // to every jspi export).
        builder.cx.current_adapter_jspi = jspi && !asyncness;

        builder.cx.current_adapter_suspending = is_suspending;

        // Process the binding and generate JavaScript and its documentation.
        let binding::JsFunction {
            js_doc,

            code,

            catch,
            log_error,
            jspi_async,
        } = builder
            .process(
                adapter,
                instrs,
                args,
                asyncness,
                variadic,
                generate_jsdoc,
                &debug_name,
                ret_ty_override,
                ret_desc,
            )
            .with_context(|| "failed to generates bindings for ".to_string() + &debug_name)?;

        // Reset per-adapter flags after processing.
        self.current_adapter_jspi = false;
        self.current_adapter_suspending = false;

        // Once we've got all the JS then put it in the right location depending
        // on what's being exported.
        match kind {
            ContextAdapterKind::Export(export) => {
                assert!(!catch);
                assert!(!log_error);

                let js_docs = format_doc_comments(&export.comments, Some(js_doc));

                match &export.kind {
                    AuxExportKind::Function(name) | AuxExportKind::FunctionThis(name) => {
                        let qualified_name = wasm_bindgen_shared::qualified_name(
                            export.js_namespace.as_deref(),
                            name,
                        );
                        let identifier = self.get_or_create_identifier(&qualified_name);

                        let async_kw = if jspi_async { "async " } else { "" };
                        {
                            let definition = format!("{async_kw}function {identifier}{code}\n");
                            define_export(
                                &mut self.exports,
                                name,
                                export.js_namespace.as_deref().unwrap_or_default(),
                                ExportEntry::Definition(ExportDefinition {
                                    identifier: identifier.clone(),
                                    comments: Some(js_docs),
                                    definition,

                                    private: false,
                                    parent_identifier: None,
                                }),
                            )?;
                        }
                    }
                    AuxExportKind::Constructor(class) => {
                        let exported = self.require_class(class);

                        if exported.has_constructor {
                            bail!("found duplicate constructor for class `{class}`");
                        }

                        exported.has_constructor = true;
                        exported.push("constructor", "", &js_docs, &code);
                    }
                    AuxExportKind::Method {
                        class,
                        name,
                        receiver,
                        kind,
                    } => {
                        let exported = self.require_class(class);

                        let mut prefix = String::new();
                        if receiver.is_static() {
                            prefix += "static ";
                        }
                        if jspi_async {
                            prefix += "async ";
                        }
                        match kind {
                            AuxExportedMethodKind::Method => {}
                            AuxExportedMethodKind::Getter => {
                                prefix += "get ";
                                if !is_computed_key(name) {
                                    exported.readable_properties.push(name.clone());
                                }
                            }
                            AuxExportedMethodKind::Setter => {
                                prefix += "set ";
                            }
                        };

                        exported.push(name, &prefix, &js_docs, &code);
                    }
                }
            }
            ContextAdapterKind::Import(core) => {
                // When js_tag is set, all catch imports use wasm catch wrappers
                // instead of the JS handleError wrapper. Suspending imports
                // never use handleError: their `catch` handling is the
                // in-wasm rejection protocol (see `transforms::jspi`), and a
                // synchronous throw from the import surfaces at the suspend
                // point the same way a rejection does.
                let has_wasm_catch = self.aux.js_tag.is_some();

                // `code` here is `(args) { body }` (no leading `function`
                // keyword). The wrapping cases prefix `function` because
                // they need a function expression in the middle of the
                // outer body; the final value we store is itself a function
                // literal, so we strip the keyword once at the end.
                let code = if catch && !has_wasm_catch && !is_suspending {
                    self.expose_handle_error()?;
                    // The catch path of `handleError` returns undefined, which
                    // would trap converting to `i64` at the JS -> Wasm
                    // boundary, so substitute a `0n` placeholder (the value is
                    // discarded once the stored exception is checked).
                    let fallback = match self.module.imports.get(core).kind {
                        walrus::ImportKind::Function(f) => {
                            let ty = self.module.funcs.get(f).ty();
                            match self.module.types.get(ty).results() {
                                [walrus::ValType::I64] => " ?? 0n",
                                _ => "",
                            }
                        }
                        _ => "",
                    };
                    format!("() {{ return handleError(function {code}, arguments){fallback}; }}")
                } else if log_error {
                    format!("() {{ return logError(function {code}, arguments); }}")
                } else {
                    code
                };

                // JSTag polyfill: when running on Node 20, wrap each import in a JS try/catch that re-throws via a
                // `new WebAssembly.Exception(__wbindgen_jstag, [e])` against
                // the polyfilled tag
                let code = if self.aux.legacy_exception_handling {
                    self.expose_wrap_error();
                    format!("() {{ return wrapError(function {code}, arguments); }}")
                } else {
                    code
                };

                let import_def = if self.aux.imports_with_suspending.contains(&id) {
                    {
                        ImportDefinition::Expression(format!(
                            "((__inner) => new WebAssembly.Suspending(function(...args) {{\n\
                                 try {{ return __inner.apply(this, args); }}\n\
                                 catch (e) {{ return Promise.reject(e); }}\n\
                             }}))(function{code})"
                        ))
                    }
                } else {
                    ImportDefinition::Function(code)
                };

                self.wasm_import_definitions.insert(core, import_def);
            }
            ContextAdapterKind::Adapter => {
                assert!(!catch);
                assert!(!log_error);

                {
                    self.globals.push_str("function ");
                    self.globals.push_str(&self.export_adapter_name(id));
                    self.globals.push_str(&code);
                    self.globals.push_str("\n\n");
                }
            }
        }

        Ok(())
    }

    /// Returns whether we should disable the logic, in debug mode, to catch an
    /// error, log it, and rethrow it. This is only intended for user-defined
    /// imports, not all imports of everything.
    fn import_never_log_error(&self, import: &AuxImport) -> bool {
        match import {
            // Some intrinsics are intended to exactly throw errors, and in
            // general we shouldn't have exceptions in our intrinsics to debug,
            // so skip these.
            AuxImport::Intrinsic(_) => true,

            // Otherwise assume everything else gets a debug log of errors
            // thrown in debug mode.
            _ => false,
        }
    }

    fn import_never_handle_error(&self, import: &AuxImport) -> bool {
        match import {
            // Some intrinsics are intended to exactly throw errors, and in
            // general we shouldn't have exceptions in our intrinsics to debug,
            // so skip these.
            AuxImport::Intrinsic(Intrinsic::Throw | Intrinsic::Rethrow) => true,

            // Otherwise assume everything else gets a debug log of errors
            // thrown in debug mode.
            _ => false,
        }
    }

    /// Attempts to directly hook up the `id` import in the Wasm module with
    /// the `instrs` specified.
    ///
    /// If this succeeds it returns `Ok(true)`, otherwise if it cannot be
    /// directly imported then `Ok(false)` is returned.
    fn attempt_direct_import(
        &mut self,
        id: ImportId,
        instrs: &[InstructionData],
    ) -> Result<bool, Error> {
        let mut call = None;
        for instr in instrs {
            match instr.instr {
                Instruction::CallAdapter(id) => {
                    if call.is_some() {
                        return Ok(false);
                    } else {
                        call = Some(id);
                    }
                }
                Instruction::CallExport(_) => return Ok(false),
                _ => {}
            }
        }
        let adapter = match call {
            Some(id) => id,
            None => return Ok(false),
        };
        match &self.wit.adapters[&adapter].kind {
            AdapterKind::Import { kind, .. } => match kind {
                AdapterJsImportKind::Normal => {}
                // method/constructors need glue because we either need to
                // invoke them as `new` or we need to invoke them with
                // method-call syntax to get the `this` parameter right.
                AdapterJsImportKind::Method | AdapterJsImportKind::Constructor => return Ok(false),
            },
            // This is an adapter-to-adapter call, so it needs a shim.
            AdapterKind::Local { .. } => return Ok(false),
        }

        // Next up check to make sure that this import is to a bare JS value
        // itself, no extra fluff intended.
        let js = match &self.aux.import_map[&adapter] {
            AuxImport::Value(AuxValue::Bare(js)) => js,
            _ => return Ok(false),
        };

        // Make sure this isn't variadic in any way which means we need some
        // sort of adapter glue.
        if self.aux.imports_with_variadic.contains(&adapter) {
            return Ok(false);
        }

        // Ensure that every single instruction can be represented without JS
        // glue being generated, aka it's covered by the JS ECMAScript bindings
        // for wasm.
        if !self.representable_without_js_glue(instrs) {
            return Ok(false);
        }

        if js.fields.is_empty() {
            match &js.name {
                JsImportName::Module { module, name } => {
                    let import = self.module.imports.get_mut(id);
                    import.module.clone_from(module);
                    import.name.clone_from(name);
                    return Ok(true);
                }
                JsImportName::LocalModule { module, name } => {
                    let module = self.config.local_module_name(module);
                    let import = self.module.imports.get_mut(id);
                    import.module = module;
                    import.name.clone_from(name);
                    return Ok(true);
                }
                JsImportName::InlineJs {
                    unique_crate_identifier,
                    snippet_idx_in_crate,
                    name,
                } => {
                    let module = self
                        .config
                        .inline_js_module_name(unique_crate_identifier, *snippet_idx_in_crate);
                    let import = self.module.imports.get_mut(id);
                    import.module = module;
                    import.name.clone_from(name);
                    return Ok(true);
                }

                // Fall through below to requiring a JS shim to create an item
                // that we can import. These are plucked from the global
                // environment so there's no way right now to describe these
                // imports in an ES module-like fashion.
                JsImportName::Global { .. } | JsImportName::VendorPrefixed { .. } => {}
            }
        }

        if let JsImportName::Global { .. } | JsImportName::VendorPrefixed { .. } = js.name {
            // We generally cannot import globals directly, because users can
            // change most globals at runtime.
            //
            // An obvious example of this when the object literally changes
            // (e.g. binding `foo.bar`), but polyfills can also change the
            // object or fundtion.
            //
            // Late binding is another issue. The function might not even be
            // defined when the Wasm module is instantiated. In such cases,
            // there is an observable difference between a direct import and a
            // JS shim calling the function.
            return Ok(false);
        }

        self.expose_not_defined();
        let name = self.import_name(js)?;
        let js = format!("typeof {name} == 'function' ? {name} : notDefined('{name}')",);
        self.wasm_import_definitions
            .insert(id, ImportDefinition::Expression(js));
        Ok(true)
    }

    fn representable_without_js_glue(&self, instrs: &[InstructionData]) -> bool {
        use Instruction::*;

        let mut last_arg = None;
        let mut saw_call = false;
        for instr in instrs {
            match instr.instr {
                // Fetching arguments is just that, a fetch, so no need for
                // glue. Note though that the arguments must be fetched in order
                // for this to actually work,
                ArgGet(i) => {
                    if saw_call {
                        return false;
                    }
                    match (i, last_arg) {
                        (0, None) => last_arg = Some(0),
                        (n, Some(i)) if n == i + 1 => last_arg = Some(n),
                        _ => return false,
                    }
                }

                // Similarly calling a function is the same as in JS, no glue
                // needed.
                CallAdapter(_) => saw_call = true,

                // Conversions to Wasm integers are always supported since
                // they're coerced into i32/f32/f64 appropriately.
                Int32ToWasm => {}
                Int64ToWasm => {}

                // Converting into a u32 isn't supported because we
                // need to generate glue to change the sign.
                WasmToInt32 { unsigned_32: false } => {}
                // A Wasm `i64` is already a signed JS BigInt, so no glue needed.
                WasmToInt64 { unsigned: false } => {}

                // JS spec automatically coerces boolean values to i32 of 0 or 1
                // depending on true/false
                I32FromBool => {}

                _ => return false,
            }
        }

        true
    }

    /// Generates a JS snippet appropriate for invoking `import`.
    ///
    /// This is generating code for `binding` where `bindings` has more type
    /// information. The `args` array is the list of JS expressions representing
    /// the arguments to pass to JS. Finally `variadic` indicates whether the
    /// last argument is a list to be splatted in a variadic way, and `prelude`
    /// is a location to push some more initialization JS if necessary.
    ///
    /// The returned value here is a JS expression which evaluates to the
    /// purpose of `AuxImport`, which depends on the kind of import.
    fn invoke_import(
        &mut self,
        import: &AuxImport,
        kind: AdapterJsImportKind,
        args: &[String],
        variadic: bool,
        prelude: &mut String,
    ) -> Result<String, Error> {
        let variadic_args = |js_arguments: &[String]| {
            Ok(if !variadic {
                js_arguments.join(", ")
            } else {
                let (last_arg, args) = match js_arguments.split_last() {
                    Some(pair) => pair,
                    None => bail!("a function with no arguments cannot be variadic"),
                };
                // Parenthesize the splatted argument. This is not required:
                // spread takes an `AssignmentExpression`, so a compound
                // expression already binds the way we want and `...arg >>> 0`
                // *is* `...(arg >>> 0)`. The parentheses are kept only because
                // they make the grouping explicit to a reader of the generated
                // JS, which is otherwise easy to misread.
                if !args.is_empty() {
                    format!("{}, ...({last_arg})", args.join(", "))
                } else {
                    format!("...({last_arg})")
                }
            })
        };

        match import {
            AuxImport::Value(val) => match kind {
                AdapterJsImportKind::Constructor => {
                    let js = match val {
                        AuxValue::Bare(js) => self.import_name(js)?,
                        _ => bail!("invalid import set for constructor"),
                    };
                    Ok(format!("new {js}({})", variadic_args(args)?))
                }
                AdapterJsImportKind::Method => {
                    let descriptor = |anchor: &str, extra: &str, field: &str, which: &str| {
                        format!(
                            "GetOwnOrInheritedPropertyDescriptor({anchor}{extra}, '{field}').{which}"
                        )
                    };
                    let js = match val {
                        AuxValue::Bare(js) => self.import_name(js)?,
                        AuxValue::Getter(class, field) => {
                            self.expose_get_inherited_descriptor();
                            let class = self.import_name(class)?;
                            descriptor(&class, ".prototype", field, "get")
                        }
                        AuxValue::ClassGetter(class, field) => {
                            self.expose_get_inherited_descriptor();
                            let class = self.import_name(class)?;
                            descriptor(&class, "", field, "get")
                        }
                        AuxValue::Setter(class, field) => {
                            self.expose_get_inherited_descriptor();
                            let class = self.import_name(class)?;
                            descriptor(&class, ".prototype", field, "set")
                        }
                        AuxValue::ClassSetter(class, field) => {
                            self.expose_get_inherited_descriptor();
                            let class = self.import_name(class)?;
                            descriptor(&class, "", field, "set")
                        }
                    };
                    Ok(format!("{js}.call({})", variadic_args(args)?))
                }
                AdapterJsImportKind::Normal => {
                    let js = match val {
                        AuxValue::Bare(js) => self.import_name(js)?,
                        _ => bail!("invalid import set for free function"),
                    };
                    Ok(format!("{js}({})", variadic_args(args)?))
                }
            },

            AuxImport::ValueWithThis(class, name) => {
                let class = self.import_name(class)?;
                Ok(format!(
                    "{class}{}({})",
                    property_accessor(name),
                    variadic_args(args)?
                ))
            }

            AuxImport::Instanceof(js) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 1);
                let js = self.import_name(js)?;
                write!(
                    prelude,
                    "\
                    let result;
                    try {{
                        result = {} instanceof {js};
                    }} catch (_) {{
                        result = false;
                    }}
                    ",
                    args[0],
                )
                .unwrap();
                Ok("result".to_owned())
            }

            AuxImport::Static { js, optional } => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 0);
                self.import_static(js, *optional)
            }

            AuxImport::String(string) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 0);

                let mut escaped = String::with_capacity(string.len());
                string.chars().for_each(|c| match c {
                    '`' | '\\' | '$' => escaped.extend(['\\', c]),
                    _ => escaped.extend([c]),
                });
                Ok(format!("`{escaped}`"))
            }

            AuxImport::Cast { sig_comment } => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 1);

                // Emitted through the shared generic-import pipeline as a
                // `__wbindgen_generic_*` shim, but this particular one is an
                // identity cast (returns its argument unchanged).
                writeln!(prelude, "// Cast intrinsic for `{sig_comment}`.")?;
                Ok(args[0].clone())
            }

            AuxImport::StructuralMethod(name) => {
                assert!(kind == AdapterJsImportKind::Normal);
                let (receiver, args) = match args.split_first() {
                    Some(pair) => pair,
                    None => bail!("structural method calls must have at least one argument"),
                };
                Ok(format!(
                    "{receiver}{}({})",
                    property_accessor(name),
                    variadic_args(args)?
                ))
            }

            AuxImport::StructuralGetter(field) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(
                    args.len(),
                    1,
                    "The getter '{field}' as more than one args ({n})",
                    n = args.len()
                );
                Ok(format!("{}{}", args[0], property_accessor(field)))
            }

            AuxImport::StructuralClassGetter(class, field) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 0);
                let class = self.import_name(class)?;
                Ok(format!("{class}{}", property_accessor(field)))
            }

            AuxImport::StructuralSetter(field) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 2);
                Ok(format!(
                    "{}{} = {}",
                    args[0],
                    property_accessor(field),
                    args[1]
                ))
            }

            AuxImport::StructuralClassSetter(class, field) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 1);
                let class = self.import_name(class)?;
                Ok(format!("{class}{} = {}", property_accessor(field), args[0]))
            }

            AuxImport::IndexingGetterOfClass(class) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 1);
                let class = self.import_name(class)?;
                Ok(format!("{class}[{}]", args[0]))
            }

            AuxImport::IndexingGetterOfObject => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 2);
                Ok(format!("{}[{}]", args[0], args[1]))
            }

            AuxImport::IndexingSetterOfClass(class) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 2);
                let class = self.import_name(class)?;
                Ok(format!("{class}[{}] = {}", args[0], args[1]))
            }

            AuxImport::IndexingSetterOfObject => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 3);
                Ok(format!("{}[{}] = {}", args[0], args[1], args[2]))
            }

            AuxImport::IndexingDeleterOfClass(class) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 1);
                let class = self.import_name(class)?;
                Ok(format!("delete {class}[{}]", args[0]))
            }

            AuxImport::IndexingDeleterOfObject => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 2);
                Ok(format!("delete {}[{}]", args[0], args[1]))
            }

            AuxImport::WrapInExportedClass(class) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 1);
                let identifier = self.require_class_wrap(class);
                Ok(format!("{identifier}.__wrap({})", args[0]))
            }

            AuxImport::Intrinsic(intrinsic) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                self.invoke_intrinsic(intrinsic, args, prelude)
            }

            AuxImport::LinkTo(path, content) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 0);
                if self.config.split_linked_modules {
                    let base = { "import.meta.url" };
                    Ok(format!("new URL('{path}', {base}).toString()"))
                } else if let Some(content) = content {
                    let mut escaped = String::with_capacity(content.len());
                    content.chars().for_each(|c| match c {
                        '`' | '\\' | '$' => escaped.extend(['\\', c]),
                        _ => escaped.extend([c]),
                    });
                    prelude.push_str(&format!("const val = `{escaped}`;\n"));
                    Ok("typeof URL.createObjectURL === 'undefined' ? \
                        \"data:application/javascript,\" + encodeURIComponent(val) : \
                        URL.createObjectURL(new Blob([val], { type: \"text/javascript\" }))"
                        .to_owned())
                } else {
                    Err(anyhow!("wasm-bindgen needs to be invoked with `--split-linked-modules`, because \"{path}\" cannot be embedded.\n\
                        See https://wasm-bindgen.github.io/wasm-bindgen/reference/cli.html#--split-linked-modules for details."))
                }
            }

            AuxImport::UnwrapExportedClass(class) => {
                assert!(kind == AdapterJsImportKind::Normal);
                assert!(!variadic);
                assert_eq!(args.len(), 1);
                let identifier = self.require_class_unwrap(class);
                Ok(format!("{identifier}.__unwrap({})", args[0]))
            }
        }
    }

    /// Same as `invoke_import` above, except more specialized and only used for
    /// generating the JS expression needed to implement a particular intrinsic.
    fn invoke_intrinsic(
        &mut self,
        intrinsic: &Intrinsic,
        args: &[String],
        prelude: &mut String,
    ) -> Result<String, Error> {
        let expr = match intrinsic {
            Intrinsic::JsvalEq => {
                assert_eq!(args.len(), 2);
                format!("{} === {}", args[0], args[1])
            }

            Intrinsic::JsvalLooseEq => {
                assert_eq!(args.len(), 2);
                format!("{} == {}", args[0], args[1])
            }

            Intrinsic::IsFunction => {
                assert_eq!(args.len(), 1);
                format!("typeof({}) === 'function'", args[0])
            }

            Intrinsic::IsUndefined => {
                assert_eq!(args.len(), 1);
                format!("{} === undefined", args[0])
            }

            Intrinsic::IsNull => {
                assert_eq!(args.len(), 1);
                format!("{} === null", args[0])
            }

            Intrinsic::IsNullOrUndefined => {
                assert_eq!(args.len(), 1);
                format!("{} == null", args[0])
            }

            Intrinsic::IsObject => {
                assert_eq!(args.len(), 1);
                prelude.push_str(&format!("const val = {};\n", args[0]));
                "typeof(val) === 'object' && val !== null".to_string()
            }

            Intrinsic::IsSymbol => {
                assert_eq!(args.len(), 1);
                format!("typeof({}) === 'symbol'", args[0])
            }

            Intrinsic::IsString => {
                assert_eq!(args.len(), 1);
                format!("typeof({}) === 'string'", args[0])
            }

            Intrinsic::IsBigInt => {
                assert_eq!(args.len(), 1);
                format!("typeof({}) === 'bigint'", args[0])
            }

            Intrinsic::Typeof => {
                assert_eq!(args.len(), 1);
                format!("typeof {}", args[0])
            }

            Intrinsic::In => {
                assert_eq!(args.len(), 2);
                format!("{} in {}", args[0], args[1])
            }

            Intrinsic::IsFalsy => {
                assert_eq!(args.len(), 1);
                format!("!{}", args[0])
            }

            Intrinsic::TryIntoNumber => {
                assert_eq!(args.len(), 1);
                prelude.push_str("let result;\n");
                writeln!(
                    prelude,
                    "try {{ result = +{} }} catch (e) {{ result = e }}",
                    args[0]
                )
                .unwrap();
                "result".to_owned()
            }

            Intrinsic::Neg => {
                assert_eq!(args.len(), 1);
                format!("-{}", args[0])
            }

            Intrinsic::BitAnd => {
                assert_eq!(args.len(), 2);
                format!("{} & {}", args[0], args[1])
            }

            Intrinsic::BitOr => {
                assert_eq!(args.len(), 2);
                format!("{} | {}", args[0], args[1])
            }

            Intrinsic::BitXor => {
                assert_eq!(args.len(), 2);
                format!("{} ^ {}", args[0], args[1])
            }

            Intrinsic::BitNot => {
                assert_eq!(args.len(), 1);
                format!("~{}", args[0])
            }

            Intrinsic::Shl => {
                assert_eq!(args.len(), 2);
                format!("{} << {}", args[0], args[1])
            }

            Intrinsic::Shr => {
                assert_eq!(args.len(), 2);
                format!("{} >> {}", args[0], args[1])
            }

            Intrinsic::UnsignedShr => {
                assert_eq!(args.len(), 2);
                format!("{} >>> {}", args[0], args[1])
            }

            Intrinsic::Add => {
                assert_eq!(args.len(), 2);
                format!("{} + {}", args[0], args[1])
            }

            Intrinsic::Sub => {
                assert_eq!(args.len(), 2);
                format!("{} - {}", args[0], args[1])
            }

            Intrinsic::Div => {
                assert_eq!(args.len(), 2);
                format!("{} / {}", args[0], args[1])
            }

            Intrinsic::CheckedDiv => {
                assert_eq!(args.len(), 2);
                prelude.push_str("let result;\n");
                writeln!(
                    prelude,
                    "try {{
                        result = {} / {};
                    }} catch (e) {{
                        if (e instanceof RangeError) {{
                            result = e;
                        }} else {{
                            throw e;
                        }}
                    }}",
                    args[0], args[1]
                )
                .unwrap();
                "result".to_owned()
            }

            Intrinsic::Mul => {
                assert_eq!(args.len(), 2);
                format!("{} * {}", args[0], args[1])
            }

            Intrinsic::Rem => {
                assert_eq!(args.len(), 2);
                format!("{} % {}", args[0], args[1])
            }

            Intrinsic::Pow => {
                assert_eq!(args.len(), 2);
                format!("{} ** {}", args[0], args[1])
            }

            Intrinsic::LT => {
                assert_eq!(args.len(), 2);
                format!("{} < {}", args[0], args[1])
            }

            Intrinsic::LE => {
                assert_eq!(args.len(), 2);
                format!("{} <= {}", args[0], args[1])
            }

            Intrinsic::GE => {
                assert_eq!(args.len(), 2);
                format!("{} >= {}", args[0], args[1])
            }

            Intrinsic::GT => {
                assert_eq!(args.len(), 2);
                format!("{} > {}", args[0], args[1])
            }

            Intrinsic::ObjectCloneRef => {
                assert_eq!(args.len(), 1);
                args[0].clone()
            }

            Intrinsic::ObjectDropRef => {
                assert_eq!(args.len(), 1);
                args[0].clone()
            }

            Intrinsic::NumberGet => {
                assert_eq!(args.len(), 1);
                prelude.push_str(&format!("const obj = {};\n", args[0]));
                "typeof(obj) === 'number' ? obj : undefined".to_string()
            }

            Intrinsic::StringGet => {
                assert_eq!(args.len(), 1);
                prelude.push_str(&format!("const obj = {};\n", args[0]));
                "typeof(obj) === 'string' ? obj : undefined".to_string()
            }

            Intrinsic::BooleanGet => {
                assert_eq!(args.len(), 1);
                prelude.push_str(&format!("const v = {};\n", args[0]));
                "typeof(v) === 'boolean' ? v : undefined".to_string()
            }

            Intrinsic::BigIntGetAsI64 => {
                assert_eq!(args.len(), 1);
                prelude.push_str(&format!("const v = {};\n", args[0]));
                "typeof(v) === 'bigint' ? v : undefined".to_string()
            }

            Intrinsic::Throw => {
                assert_eq!(args.len(), 1);
                if self.aux.wrapped_js_tag.is_some() {
                    format!(
                        "throw new WebAssembly.Exception(__wbindgen_wrapped_jstag, [new Error({})])",
                        args[0]
                    )
                } else {
                    format!("throw new Error({})", args[0])
                }
            }

            Intrinsic::Rethrow => {
                assert_eq!(args.len(), 1);
                if self.aux.wrapped_js_tag.is_some() {
                    format!(
                        "throw new WebAssembly.Exception(__wbindgen_wrapped_jstag, [{}])",
                        args[0]
                    )
                } else {
                    format!("throw {}", args[0])
                }
            }

            Intrinsic::Module => {
                assert_eq!(args.len(), 0);

                { "wasmModule" }.to_string()
            }

            Intrinsic::Instance => {
                assert_eq!(args.len(), 0);
                { "wasmInstance" }.to_string()
            }

            Intrinsic::Exports => {
                assert_eq!(args.len(), 0);
                {
                    "wasm".to_string()
                }
            }

            Intrinsic::Memory => {
                assert_eq!(args.len(), 0);
                let mut memories = self.module.memories.iter();
                let memory = memories
                    .next()
                    .ok_or_else(|| anyhow!("no memory found to return in memory intrinsic"))?
                    .id();
                if memories.next().is_some() {
                    bail!(
                        "multiple memories found, unsure which to return \
                         from memory intrinsic"
                    );
                }
                drop(memories);
                format!("wasm.{}", self.export_name_of(memory))
            }

            Intrinsic::FunctionTable => {
                assert_eq!(args.len(), 0);
                let name = self.export_function_table()?;
                self.wasm_export_ref(&name)
            }

            Intrinsic::DebugString => {
                assert_eq!(args.len(), 1);
                self.expose_debug_string();

                format!("debugString({})", args[0])
            }

            Intrinsic::CopyToTypedArray => {
                assert_eq!(args.len(), 2);
                format!(
                    "new Uint8Array({dst}.buffer, {dst}.byteOffset, {dst}.byteLength).set({src})",
                    src = args[0],
                    dst = args[1]
                )
            }

            Intrinsic::ExternrefHeapLiveCount => {
                assert_eq!(args.len(), 0);
                self.expose_global_heap();
                prelude.push_str(
                    "
                        let free_count = 0;
                        let next = heap_next;
                        while (next < heap.length) {
                            free_count += 1;
                            next = heap[next];
                        }
                    ",
                );
                format!(
                    "heap.length - free_count - {INITIAL_HEAP_OFFSET} - {}",
                    INITIAL_HEAP_VALUES.len(),
                )
            }

            Intrinsic::InitExternrefTable => {
                let table = self
                    .aux
                    .externref_table
                    .ok_or_else(|| anyhow!("must enable externref to use externref intrinsic"))?;
                let name = self.export_name_of(table);
                let table_ref = self.wasm_export_ref(&name);

                // Grow the table to insert our initial values, and then also
                // set the 0th slot to `undefined` since that's what we've
                // historically used for our ABI which is that the index of 0
                // returns `undefined` for types like `None` going out.
                let mut base = format!(
                    "
                      const table = {table_ref};
                      const offset = table.grow({});
                      table.set(0, undefined);
                    ",
                    INITIAL_HEAP_VALUES.len(),
                );
                for (i, value) in INITIAL_HEAP_VALUES.iter().enumerate() {
                    base.push_str(&format!(
                        "{}table.set(offset + {i}, {value})",
                        if i > 0 { ";\n" } else { "" }
                    ));
                }
                base
            }
            Intrinsic::PanicError => {
                assert_eq!(args.len(), 1);
                self.expose_panic_error();
                format!("new PanicError({})", args[0])
            }
            Intrinsic::Reinit => {
                assert_eq!(args.len(), 0);
                if self.generate_reinit {
                    "__wbg_reinit_scheduled = true".to_string()
                } else {
                    "undefined".to_string()
                }
            }

            // Identity: the shim hands the pending Promise straight back to
            // `WebAssembly.Suspending`, which awaits it and resumes the fiber
            // with the settled value as the import's externref return value.
            // A rejection is thrown into wasm as a `WebAssembly.JSTag`
            // exception, which the in-wasm wrapper generated by
            // `transforms::jspi` catches and records in the
            // `__wbindgen_jspi_rejected` linear-memory flag.
            Intrinsic::JspiSuspend => {
                assert_eq!(args.len(), 1);
                args[0].clone()
            }

            // The ambient-context probe is always rewritten in-wasm by the
            // jspi transform (a `__jspi_stack_base` read, or constant 0), so
            // this shim is never emitted in practice; keep a semantically
            // equivalent fallback regardless.
            Intrinsic::JspiInContext => {
                assert_eq!(args.len(), 0);
                "0".to_string()
            }

            // Schedules a promising-entered task poll on the microtask
            // queue, through `WebAssembly.promising` of the raw task-poll
            // trampoline export (which the jspi transform has given the
            // in-wasm fiber wrapper), so the poll runs on a fresh
            // suspendable stack. The promise is deliberately dropped —
            // completion and re-poll bookkeeping live wasm-side, and a
            // panic surfaces as an unhandled rejection like any spawned
            // task failure. `JspiSpawnFirst` is the same operation reached
            // only from the initial schedule. In modules without jspi
            // exports both intrinsics are stubbed out in-wasm and these
            // shims are never emitted.
            Intrinsic::JspiSpawnPoll | Intrinsic::JspiSpawnFirst => {
                assert_eq!(args.len(), 1);

                let cache = "__wbg_jspi_task_poll_promising";
                let decl = { Cow::Owned(format!("let {cache};")) };
                self.intrinsic(Cow::Borrowed(cache), decl);
                let trampoline = self.wasm_export_ref(crate::transforms::jspi::TASK_POLL_EXPORT);
                // The promise is dropped, so a poll that unwinds (a panic, or
                // a rethrown rejection of a non-`catch` suspending import)
                // surfaces as an unhandled rejection. When the catch-wrapper
                // transform is active it re-tags JS exceptions against
                // `__wbindgen_wrapped_jstag`; unwrap those so the rejection
                // carries the original reason, exactly as `__wbg_handle_catch`
                // does for glue-wrapped exports.
                let unwrap = if self.aux.wrapped_js_tag.is_some() {
                    ".catch(e => {\n\
                         if (e instanceof WebAssembly.Exception && e.is(__wbindgen_wrapped_jstag)) \
                             throw e.getArg(__wbindgen_wrapped_jstag, 0);\n\
                         throw e;\n\
                     })"
                } else {
                    ""
                };
                format!(
                    "Promise.resolve().then(() => \
                     ({cache} ??= WebAssembly.promising({trampoline}))({})){unwrap}",
                    args[0]
                )
            }
        };
        Ok(expr)
    }

    fn generate_enum(&mut self, enum_: &AuxEnum) -> Result<(), Error> {
        let qualified_name =
            wasm_bindgen_shared::qualified_name(enum_.js_namespace.as_deref(), &enum_.name);
        let identifier = self.get_or_create_identifier(&qualified_name);

        let mut variants = String::new();
        for (name, value, comments) in enum_.variants.iter() {
            let variant_docs = if comments.is_empty() {
                String::new()
            } else {
                format_doc_comments(comments, None)
            };
            variants.push_str(&variant_docs);
            variants.push_str(&format!("{name}: {value}, "));
            variants.push_str(&format!("\"{value}\": \"{name}\",\n"));
        }

        // add an `@enum {1 | 2 | 3}` to ensure that enums type-check even without .d.ts
        let mut at_enum = "@enum {".to_string();
        for (i, (_, value, _)) in enum_.variants.iter().enumerate() {
            if i != 0 {
                at_enum.push_str(" | ");
            }
            at_enum.push_str(&value.to_string());
        }
        at_enum.push('}');
        let docs = format_doc_comments(&enum_.comments, Some(at_enum));

        let definition = format!("const {identifier} = Object.freeze({{\n{variants}}});\n");

        define_export(
            &mut self.exports,
            &enum_.name,
            enum_.js_namespace.as_deref().unwrap_or_default(),
            ExportEntry::Definition(ExportDefinition {
                identifier: identifier.clone(),
                comments: Some(docs),
                definition,

                private: enum_.private,
                parent_identifier: None,
            }),
        )?;

        Ok(())
    }

    fn generate_string_enum(&mut self, string_enum: &AuxStringEnum) -> Result<(), Error> {
        let variants: Vec<_> = string_enum
            .variant_values
            .iter()
            .map(|v| format!("\"{v}\""))
            .collect();

        if self.used_string_enums.contains(&string_enum.name) {
            // only generate the internal string enum array if it's actually used
            let name = &string_enum.name;
            let values = variants.join(", ");
            {
                self.global(&format!("\nconst __wbindgen_enum_{name} = [{values}];\n"));
            }
        }

        Ok(())
    }

    fn expose_string_enum(&mut self, string_enum_name: &str) {
        self.used_string_enums.insert(string_enum_name.to_string());
    }

    fn generate_struct(&mut self, struct_: &AuxStruct) -> Result<(), Error> {
        let qualified_name =
            wasm_bindgen_shared::qualified_name(struct_.js_namespace.as_deref(), &struct_.name);
        let parent_qualified = parent_qualified_name(struct_);
        let class = self.require_class(&qualified_name);
        class.comments = format_doc_comments(&struct_.comments, None);
        class.is_inspectable = struct_.is_inspectable;
        class.private = struct_.private;
        class.js_namespace = struct_.js_namespace.as_ref().map(|ns| ns.to_vec());
        class.js_name = Some(struct_.name.clone());
        class.extends = parent_qualified;
        Ok(())
    }

    fn process_package_json(&mut self, path: &Path) -> Result<(), Error> {
        let contents =
            fs::read_to_string(path).context(format!("failed to read `{}`", path.display()))?;
        let json: json_codec::Value = json_codec::from_str(&contents)?;
        let object = match json.as_object() {
            Some(s) => s,
            None => bail!(
                "expected `package.json` to have an JSON object in `{}`",
                path.display()
            ),
        };
        let iter = object.iter();
        let mut value = None;
        for (key, v) in iter {
            if key == "dependencies" {
                value = Some(v);
                break;
            }
        }
        let value = if let Some(value) = value {
            value
        } else {
            return Ok(());
        };
        let value = match value.as_object() {
            Some(s) => s,
            None => bail!(
                "expected `dependencies` to be a JSON object in `{}`",
                path.display()
            ),
        };

        for (name, value) in value.iter() {
            let value = match value.as_str() {
                Some(s) => s,
                None => bail!(
                    "keys in `dependencies` are expected to be strings in `{}`",
                    path.display()
                ),
            };
            if let Some((prev, _prev_version)) = self.npm_dependencies.get(name) {
                bail!(
                    "dependency on NPM package `{name}` specified in two `package.json` files, \
                     which at the time is not allowed:\n  * {}\n  * {}",
                    path.display(),
                    prev.display(),
                )
            }

            self.npm_dependencies
                .insert(name.to_string(), (path.to_path_buf(), value.to_string()));
        }

        Ok(())
    }

    fn expose_debug_string(&mut self) {
        self.intrinsic("debug_string".into(), {
            "
            function debugString(val) {
                // primitive types
                const type = typeof val;
                if (type == 'number' || type == 'boolean' || val == null) {
                    return  `${val}`;
                }
                if (type == 'string') {
                    return `\"${val}\"`;
                }
                if (type == 'symbol') {
                    const description = val.description;
                    if (description == null) {
                        return 'Symbol';
                    } else {
                        return `Symbol(${description})`;
                    }
                }
                if (type == 'function') {
                    const name = val.name;
                    if (typeof name == 'string' && name.length > 0) {
                        return `Function(${name})`;
                    } else {
                        return 'Function';
                    }
                }
                // objects
                if (Array.isArray(val)) {
                    const length = val.length;
                    let debug = '[';
                    if (length > 0) {
                        debug += debugString(val[0]);
                    }
                    for(let i = 1; i < length; i++) {
                        debug += ', ' + debugString(val[i]);
                    }
                    debug += ']';
                    return debug;
                }
                // Test for built-in
                const builtInMatches = /\\[object ([^\\]]+)\\]/.exec(toString.call(val));
                let className;
                if (builtInMatches && builtInMatches.length > 1) {
                    className = builtInMatches[1];
                } else {
                    // Failed to match the standard '[object ClassName]'
                    return toString.call(val);
                }
                if (className == 'Object') {
                    // we're a user defined class or Object
                    // JSON.stringify avoids problems with cycles, and is generally much
                    // easier than looping through ownProperties of `val`.
                    try {
                        return 'Object(' + JSON.stringify(val) + ')';
                    } catch (_) {
                        return 'Object';
                    }
                }
                // errors
                if (val instanceof Error) {
                    return `${val.name}: ${val.message}\\n${val.stack}`;
                }
                // TODO we could test for more things here, like `Set`s and `Map`s.
                return className;
            }
            "
            .into()
        });
    }

    fn export_function_table(&mut self) -> Result<String, Error> {
        match self.aux.function_table {
            Some(id) => Ok(self.export_name_of(id)),
            None => bail!("no function table found in module"),
        }
    }

    fn wasm_export_ref(&self, name: &str) -> String {
        {
            format!("wasm.{name}")
        }
    }

    fn wasm_export_of(&mut self, id: impl Into<walrus::ExportItem>) -> String {
        let name = self.export_name_of(id);
        self.wasm_export_ref(&name)
    }

    /// JS snippet for an optional trailing function argument, either
    /// `", <export ref>"` or empty when absent.
    fn optional_func_arg(&mut self, func: Option<walrus::FunctionId>) -> String {
        match func {
            Some(f) => format!(", {}", self.wasm_export_of(f)),
            None => String::new(),
        }
    }

    fn export_name_of(&mut self, id: impl Into<walrus::ExportItem>) -> String {
        use walrus::ExportItem::*;

        let id = id.into();
        let export = self.module.exports.iter().find(|e| match (e.item, id) {
            (Function(a), Function(b)) => a == b,
            (Table(a), Table(b)) => a == b,
            (Memory(a), Memory(b)) => a == b,
            (Global(a), Global(b)) => a == b,
            (Tag(a), Tag(b)) => a == b,
            _ => false,
        });
        if let Some(export) = export {
            return export.name.clone();
        }
        let name = match id {
            Function(f) => self.module.funcs.get(f).name.as_deref(),
            Table(table) => self.module.tables.get(table).name.as_deref(),
            Memory(_) => Some("memory"),
            Global(g) => self.module.globals.get(g).name.as_deref(),
            Tag(t) => self.module.tags.get(t).name.as_deref(),
        }
        .unwrap_or("__wbindgen_export");
        let name = self.generate_identifier(&to_valid_ident(name));
        self.module.exports.add(&name, id);
        name
    }

    fn export_adapter_name(&self, adapter_id: AdapterId) -> String {
        let (export_id, _) = *self
            .wit
            .exports
            .iter()
            .find(|(_, id)| *id == adapter_id)
            .expect("could not find an export adapter");

        self.module.exports.get(export_id).name.clone()
    }

    fn generate_identifier(&mut self, name: &str) -> String {
        Self::generate_identifier_with(&mut self.defined_identifiers, name)
    }

    /// Returns the identifier for a qualified name, reusing a previously
    /// registered one or generating (and storing) a new one.
    fn get_or_create_identifier(&mut self, qualified_name: &str) -> String {
        if let Some(id) = self.qualified_to_identifier.get(qualified_name) {
            return id.clone();
        }
        let id = self.generate_identifier(qualified_name);
        self.qualified_to_identifier
            .insert(qualified_name.to_string(), id.clone());
        id
    }

    fn generate_identifier_with(identifiers: &mut HashMap<String, usize>, name: &str) -> String {
        let name = to_valid_ident(name);
        let cnt = identifiers.entry(name.to_string()).or_insert(0);
        *cnt += 1;
        let mut suffix = *cnt;
        if suffix == 1 {
            name.to_string()
        } else {
            // Keep incrementing until we find an identifier that isn't already taken
            let mut candidate = format!("{name}{suffix}");
            while identifiers.contains_key(&candidate) {
                suffix += 1;
                candidate = format!("{name}{suffix}");
            }
            // Update the counter and reserve the candidate
            *identifiers.get_mut(&*name).unwrap() = suffix;
            identifiers.insert(candidate.clone(), 1);
            candidate
        }
    }

    fn inject_stack_pointer_shim(&mut self) -> Result<(), Error> {
        if self.stack_pointer_shim_injected {
            return Ok(());
        }
        let stack_pointer = match self.aux.stack_pointer {
            Some(s) => s,
            // In theory this shouldn't happen since malloc is included in
            // most Wasm binaries (and may be gc'd out) and that almost
            // always pulls in a stack pointer. We can try to synthesize
            // something here later if necessary.
            None => bail!("failed to find stack pointer"),
        };

        use walrus::ir::*;

        // Shim ABI matches `WasmWord`: `f64` on memory64, `i32` on wasm32.
        // The `__stack_pointer` global is still `i64` on memory64, so we
        // convert at the boundary.
        let val_type = if self.memory64 {
            ValType::F64
        } else {
            ValType::I32
        };

        let mut builder =
            walrus::FunctionBuilder::new(&mut self.module.types, &[val_type], &[val_type]);
        builder.name("__wbindgen_add_to_stack_pointer".to_string());

        let mut body = builder.func_body();
        let arg = self.module.locals.add(val_type);

        // Create a shim function that mutate the stack pointer
        // to avoid exporting a mutable global.
        if self.memory64 {
            body.local_get(arg)
                .unop(UnaryOp::I64TruncSSatF64)
                .global_get(stack_pointer)
                .binop(BinaryOp::I64Add)
                .global_set(stack_pointer)
                .global_get(stack_pointer)
                .unop(UnaryOp::F64ConvertSI64);
        } else {
            body.local_get(arg)
                .global_get(stack_pointer)
                .binop(BinaryOp::I32Add)
                .global_set(stack_pointer)
                .global_get(stack_pointer);
        }

        let add_to_stack_pointer_func = builder.finish(vec![arg], &mut self.module.funcs);

        self.module
            .exports
            .add("__wbindgen_add_to_stack_pointer", add_to_stack_pointer_func);

        self.stack_pointer_shim_injected = true;

        Ok(())
    }
}

/// A categorization of adapters for the purpose of code generation.
///
/// This is different from [`AdapterKind`] and is only used internally in the
/// code generation process.
enum ContextAdapterKind<'a> {
    /// An exported function, method, constructor, or getter/setter.
    Export(&'a AuxExport),
    /// An imported function or intrinsic.
    Import(walrus::ImportId),
    Adapter,
}
impl<'a> ContextAdapterKind<'a> {
    fn get(id: AdapterId, aux: &'a WasmBindgenAux, wit: &'a NonstandardWitSection) -> Self {
        match aux.export_map.get(&id) {
            Some(export) => ContextAdapterKind::Export(export),
            None => {
                let core = wit.implements.iter().find(|pair| pair.2 == id);
                match core {
                    Some((core, _, _)) => ContextAdapterKind::Import(*core),
                    None => ContextAdapterKind::Adapter,
                }
            }
        }
    }
}

/// Compute the qualified JS identity (`<ns>__<name>`) of a child
/// struct's parent class, used as the key into `exported_classes` when
/// resolving the parent for `class Child extends Parent` codegen.
///
/// Sources, in order:
///   * `extends_js_class` + `extends_js_namespace` if the user declared
///     them on the child (required when the parent uses `js_name` /
///     `js_namespace`).
///   * Otherwise the last segment of the `extends` Rust path as a
///     fall-back, which matches the no-rename / no-namespace case where
///     the parent's `js_name` defaults to its Rust ident and
///     `js_namespace` is `None`.
///
/// Returns `None` when the child has no `extends` at all.
fn parent_qualified_name(struct_: &AuxStruct) -> Option<String> {
    if struct_.extends.is_none() && struct_.extends_js_class.is_none() {
        return None;
    }
    let bare = struct_
        .extends_js_class
        .clone()
        .or_else(|| struct_.extends.clone())?;
    Some(wasm_bindgen_shared::qualified_name(
        struct_.extends_js_namespace.as_deref(),
        &bare,
    ))
}

/// Iterate over the adapters in a deterministic order.
fn iter_adapter<'a>(
    aux: &'a WasmBindgenAux,
    wit: &'a NonstandardWitSection,
    module: &Module,
) -> Vec<(AdapterId, &'a Adapter, ContextAdapterKind<'a>)> {
    let mut adapters: Vec<_> = wit
        .adapters
        .iter()
        .map(|(id, adapter)| {
            // we need the kind of the adapter to properly sort them
            let kind = ContextAdapterKind::get(*id, aux, wit);
            (*id, adapter, kind)
        })
        .collect();

    // Sort adapters by kind first (imports, exports, adapters), then by name within each kind
    // to ensure deterministic ordering of generated code.
    adapters.sort_by(|(a_id, _, a), (b_id, _, b)| {
        fn get_kind_order(kind: &ContextAdapterKind) -> u8 {
            match kind {
                ContextAdapterKind::Import(_) => 0,
                ContextAdapterKind::Export(_) => 1,
                ContextAdapterKind::Adapter => 2,
            }
        }

        match (a, b) {
            (ContextAdapterKind::Import(a), ContextAdapterKind::Import(b)) => {
                let a = module.imports.get(*a);
                let b = module.imports.get(*b);
                a.name.cmp(&b.name)
            }
            (ContextAdapterKind::Export(a), ContextAdapterKind::Export(b)) => {
                // Sort exports by debug_name to ensure deterministic identifier generation
                // when multiple exports have the same JS name (e.g., due to js_name attribute).
                a.debug_name.cmp(&b.debug_name)
            }
            (ContextAdapterKind::Adapter, ContextAdapterKind::Adapter) => {
                let export_a = wit.exports.iter().find(|(_, id)| id == a_id);
                let export_b = wit.exports.iter().find(|(_, id)| id == b_id);

                match (export_a, export_b) {
                    (Some((export_id_a, _)), Some((export_id_b, _))) => {
                        let export_a = module.exports.get(*export_id_a);
                        let export_b = module.exports.get(*export_id_b);
                        // We cannot sort mangled names as they are machine-dependent, therefore we instead
                        // sort by function signature.
                        let get_type_key = |export: &walrus::Export| -> String {
                            let func_id = match export.item {
                                walrus::ExportItem::Function(id) => id,
                                _ => return String::new(),
                            };
                            let ty_id = module.funcs.get(func_id).ty();
                            let ty = module.types.get(ty_id);
                            // Create a string representation of the type signature
                            format!("{:?}-{:?}", ty.params(), ty.results())
                        };

                        get_type_key(export_b).cmp(&get_type_key(export_a))
                    }
                    (Some(_), None) => std::cmp::Ordering::Less, // Exported adapters come first
                    (None, Some(_)) => std::cmp::Ordering::Greater, // Exported adapters come first
                    (None, None) => a_id.cmp(b_id), // Both without exports, compare by ID
                }
            }
            _ => get_kind_order(a).cmp(&get_kind_order(b)),
        }
    });

    adapters
}

/// Iterate over the imports in a deterministic order.
fn iter_by_import<'a, T>(
    map: &'a HashMap<ImportId, T>,
    module: &Module,
) -> Vec<(&'a ImportId, &'a T)> {
    let mut items: Vec<_> = map.iter().collect();

    // Sort by import name.
    //
    // Imports have a name and a module, and it's important that we *ignore*
    // the module. The module of an import is set to its final value *during*
    // code generation, so using it here would cause the imports to be sorted
    // differently depending on which part of the code generation process we're
    // in.
    items.sort_by(|&(a, _), &(b, _)| {
        let a = module.imports.get(*a);
        let b = module.imports.get(*b);

        a.name.cmp(&b.name)
    });

    items
}

fn check_duplicated_getter_and_setter_names(
    exports: &[(&AdapterId, &AuxExport)],
) -> Result<(), Error> {
    fn verify_exports(
        first_class: &str,
        first_field: &str,
        first_receiver: &AuxReceiverKind,
        second_class: &str,
        second_field: &str,
        second_receiver: &AuxReceiverKind,
    ) -> Result<(), Error> {
        let both_are_in_the_same_class = first_class == second_class;
        let both_are_referencing_the_same_field = first_field == second_field
            && first_receiver.is_static() == second_receiver.is_static();
        if both_are_in_the_same_class && both_are_referencing_the_same_field {
            bail!(format!(
                "There can be only one getter/setter definition for `{first_field}` in `{first_class}`"
            ));
        }
        Ok(())
    }
    for (idx, (_, first_export)) in exports.iter().enumerate() {
        for (_, second_export) in exports.iter().skip(idx + 1) {
            match (&first_export.kind, &second_export.kind) {
                (
                    AuxExportKind::Method {
                        class: first_class,
                        name: first_name,
                        kind: AuxExportedMethodKind::Getter,
                        receiver: first_receiver,
                    },
                    AuxExportKind::Method {
                        class: second_class,
                        name: second_name,
                        kind: AuxExportedMethodKind::Getter,
                        receiver: second_receiver,
                    },
                ) => verify_exports(
                    first_class,
                    first_name,
                    first_receiver,
                    second_class,
                    second_name,
                    second_receiver,
                )?,
                (
                    AuxExportKind::Method {
                        class: first_class,
                        name: first_name,
                        kind: AuxExportedMethodKind::Setter,
                        receiver: first_receiver,
                    },
                    AuxExportKind::Method {
                        class: second_class,
                        name: second_name,
                        kind: AuxExportedMethodKind::Setter,
                        receiver: second_receiver,
                    },
                ) => verify_exports(
                    first_class,
                    first_name,
                    first_receiver,
                    second_class,
                    second_name,
                    second_receiver,
                )?,
                _ => {}
            }
        }
    }
    Ok(())
}

fn format_doc_comments(comments: &str, js_doc_comments: Option<String>) -> String {
    let body: String = comments.lines().fold(String::new(), |mut output, c| {
        output.push_str(" *");
        if !c.is_empty() && !c.starts_with(' ') {
            output.push(' ');
        }
        output.push_str(c);
        output.push('\n');
        output
    });
    let doc = if let Some(docs) = js_doc_comments {
        docs.lines().fold(String::new(), |mut output: String, l| {
            let _ = writeln!(output, " * {l}");
            output
        })
    } else {
        String::new()
    };
    if body.is_empty() && doc.is_empty() {
        // don't emit empty doc comments
        String::new()
    } else {
        format!("/**\n{body}{doc} */\n")
    }
}

/// Reorder exports so that a `class Child extends Parent` Definition appears
/// after the Definition that introduces `Parent` (matched by identifier).
/// Independents preserve their relative BTreeMap (export-name) order.
///
/// Children of imported / external bases (where no Definition introduces the
/// referenced parent_identifier in this module) are treated as independents;
/// the JS runtime resolves the `extends` reference normally.
///
/// Recurses into namespaces so nested class hierarchies are also ordered.
fn topo_sort_class_exports(exports: BTreeMap<String, ExportEntry>) -> Vec<(String, ExportEntry)> {
    topo_sort_class_entries(exports.into_iter().collect())
}

/// Inner helper that operates on an ordered `Vec` so a recursion into a
/// namespace whose contents are already topo-sorted does not re-shuffle
/// them.
///
/// `entries` are sorted by name first (stable), so non-dependent siblings
/// emit in alphabetic order. Topo-sort then permutes only as needed to
/// place `class Child extends Parent` after `Parent`.
fn topo_sort_class_entries(mut entries: Vec<(String, ExportEntry)>) -> Vec<(String, ExportEntry)> {
    use std::collections::HashSet;

    entries.sort_by(|(a, _), (b, _)| a.cmp(b));

    // Collect the set of identifiers introduced by Definitions in *this* level
    // — only those participate as resolvable parents. Anything pointing
    // outside the set (imported parents, etc.) is left as-is.
    let local_ids: HashSet<String> = entries
        .iter()
        .filter_map(|(_, e)| match e {
            ExportEntry::Definition(d) => Some(d.identifier.clone()),
            ExportEntry::Namespace(_) => None,
        })
        .collect();

    let n = entries.len();

    // Map each entry's identifier to its index for parent lookups, and recurse
    // into namespace bodies up front so the ordering inside each namespace is
    // also fixed.
    let mut name_to_idx: std::collections::HashMap<String, usize> =
        std::collections::HashMap::with_capacity(n);
    let mut entries: Vec<(String, ExportEntry)> = entries
        .into_iter()
        .map(|(name, entry)| match entry {
            ExportEntry::Namespace(mut ns) => {
                let inner = std::mem::take(&mut ns.ns);
                // Replace the inner contents with a topo-sorted `Vec` so
                // emission walks them in dependency order.
                ns.ns = topo_sort_class_entries(inner);
                (name, ExportEntry::Namespace(ns))
            }
            other => (name, other),
        })
        .collect();
    for (i, (_, e)) in entries.iter().enumerate() {
        if let ExportEntry::Definition(d) = e {
            name_to_idx.insert(d.identifier.clone(), i);
        }
    }

    // Stable topological sort: process indices in original order, defer those
    // whose parent (if local) hasn't been emitted yet.
    let mut emitted: Vec<bool> = vec![false; n];
    let mut out: Vec<(String, ExportEntry)> = Vec::with_capacity(n);

    // Repeatedly scan; at least one item makes progress each pass unless
    // there's a cycle (impossible: extends produces a tree because each class
    // declares at most one parent and self-reference is rejected upstream).
    loop {
        let mut progress = false;
        for i in 0..n {
            if emitted[i] {
                continue;
            }
            let parent_local_idx = match &entries[i].1 {
                ExportEntry::Definition(d) => d
                    .parent_identifier
                    .as_ref()
                    .filter(|p| local_ids.contains(*p))
                    .and_then(|p| name_to_idx.get(p).copied()),
                _ => None,
            };
            let ready = match parent_local_idx {
                Some(p) => emitted[p],
                None => true,
            };
            if ready {
                // Take the entry out by replacing with a sentinel; entries[i]
                // is no longer accessed after this.
                let placeholder = (String::new(), ExportEntry::Namespace(Default::default()));
                let taken = std::mem::replace(&mut entries[i], placeholder);
                out.push(taken);
                emitted[i] = true;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }

    // If any items remain (shouldn't happen in practice), append them in
    // original order so output is total.
    for i in 0..n {
        if !emitted[i] {
            let placeholder = (String::new(), ExportEntry::Namespace(Default::default()));
            let taken = std::mem::replace(&mut entries[i], placeholder);
            out.push(taken);
        }
    }

    out
}

/// Defines an export with an optional namespace path segment
/// Namespaces are defined into the exports map as needed via ExportEntry::Namespace
fn define_export(
    exports: &mut BTreeMap<String, ExportEntry>,
    export_name: &str,
    ns_path: &[String],
    export: ExportEntry,
) -> Result<(), Error> {
    // Namespaces are only defined into exports by this function, not by consumers of this function.
    assert!(!matches!(export, ExportEntry::Namespace(_)));

    if ns_path.is_empty() {
        match exports.entry(export_name.to_string()) {
            Entry::Vacant(e) => {
                e.insert(export);
            }
            Entry::Occupied(mut e) => {
                if let ExportEntry::Definition(def) = export {
                    if def.definition.is_empty() {
                        if let ExportEntry::Namespace(ns) = e.get_mut() {
                            if ns.id.is_none() {
                                ns.id = Some(def.identifier);
                                return Ok(());
                            }
                        }
                    }
                }
                bail!("Cannot define export over existing namespace {export_name}");
            }
        };
    } else {
        let export_entry = exports
            .entry(ns_path[0].to_string())
            .or_insert(ExportEntry::Namespace(Default::default()));
        let ns = match export_entry {
            ExportEntry::Namespace(ns) => ns,
            ExportEntry::Definition(def) if def.definition.is_empty() => {
                *export_entry = ExportEntry::Namespace(ExportedNamespace {
                    id: Some(def.identifier.to_string()),
                    ns: Vec::new(),
                });
                let ExportEntry::Namespace(ns) = export_entry else {
                    unreachable!();
                };
                ns
            }
            _ => {
                bail!("Cannot to define namespace export over existing definition {export_name}");
            }
        };
        define_namespace_export(&mut ns.ns, export_name, &ns_path[1..], export)?;
    }
    Ok(())
}

/// `define_export` for namespace contents — the inner storage is a `Vec`
/// (not a `BTreeMap`) so the topo-sort imposed at emission time survives.
/// Linear-scans for an existing entry by name; namespaces are small.
fn define_namespace_export(
    exports: &mut Vec<(String, ExportEntry)>,
    export_name: &str,
    ns_path: &[String],
    export: ExportEntry,
) -> Result<(), Error> {
    assert!(!matches!(export, ExportEntry::Namespace(_)));

    if ns_path.is_empty() {
        if let Some(slot) = exports.iter_mut().find(|(k, _)| k == export_name) {
            if let ExportEntry::Definition(def) = export {
                if def.definition.is_empty() {
                    if let ExportEntry::Namespace(ns) = &mut slot.1 {
                        if ns.id.is_none() {
                            ns.id = Some(def.identifier);
                            return Ok(());
                        }
                    }
                }
            }
            bail!("Cannot define export over existing namespace {export_name}");
        }
        exports.push((export_name.to_string(), export));
    } else {
        let idx = match exports.iter().position(|(k, _)| k == &ns_path[0]) {
            Some(i) => i,
            None => {
                exports.push((
                    ns_path[0].clone(),
                    ExportEntry::Namespace(Default::default()),
                ));
                exports.len() - 1
            }
        };
        let entry = &mut exports[idx].1;
        let ns = match entry {
            ExportEntry::Namespace(ns) => ns,
            ExportEntry::Definition(def) if def.definition.is_empty() => {
                *entry = ExportEntry::Namespace(ExportedNamespace {
                    id: Some(def.identifier.to_string()),
                    ns: Vec::new(),
                });
                let ExportEntry::Namespace(ns) = entry else {
                    unreachable!();
                };
                ns
            }
            _ => {
                bail!("Cannot to define namespace export over existing definition {export_name}");
            }
        };
        define_namespace_export(&mut ns.ns, export_name, &ns_path[1..], export)?;
    }
    Ok(())
}

fn property_accessor(name: &str) -> String {
    if is_computed_key(name) {
        return name.to_string();
    }
    name.split('.')
        .map(|segment| {
            if is_valid_ident(segment) {
                format!(".{segment}")
            } else {
                format!("[\"{}\"]", segment.escape_default())
            }
        })
        .collect()
}

/// Returns whether `name` is a computed-key form, i.e. starts with `[`
/// and ends with `]`. Used to opt into JS bracket-style property keys
/// like `[Symbol.iterator]` from a string literal.
fn is_computed_key(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2 && bytes[0] == b'[' && bytes[bytes.len() - 1] == b']'
}

impl ExportedClass {
    fn push(&mut self, function_name: &str, function_prefix: &str, js_docs: &str, js: &str) {
        self.contents.push_str(js_docs);
        self.contents.push_str(function_prefix);
        self.contents.push_str(function_name);
        self.contents.push_str(js);
        self.contents.push('\n');
    }
}

struct MemView {
    name: Cow<'static, str>,
    num: usize,
}

impl MemView {
    /// Formats the MemView specifically for accessing the memory buffer directly
    fn access(&self) -> String {
        format!("{self}()")
    }
}

impl fmt::Display for MemView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.name, self.num)
    }
}

fn write_es_import(dest: &mut String, module: &str, items: &[(String, Option<String>)]) {
    dest.push_str("import { ");
    for (i, (item, rename)) in items.iter().enumerate() {
        if i > 0 {
            dest.push_str(", ");
        }
        if is_valid_ident(item) {
            dest.push_str(item);
        } else {
            dest.push('\'');
            dest.push_str(item);
            dest.push('\'');
        }
        if let Some(other) = rename {
            dest.push_str(" as ");
            dest.push_str(other);
        }
    }
    dest.push_str(" } from '");
    dest.push_str(module);
    dest.push_str("';\n");
}
