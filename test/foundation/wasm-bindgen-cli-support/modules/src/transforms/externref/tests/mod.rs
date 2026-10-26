//! Explicit fixture tests compare independent, immutable `CHECK-ALL` expectations.

use anyhow::{anyhow, bail, Result};
use std::fs;
use std::path::{Path, PathBuf};
use walrus::ModuleConfig;
use wasm_text_parser::parser::{Parse, Parser};

fn runtest(test: &Test) -> Result<String> {
    let wasm = crate::transforms::parse_wat_file(&test.file)?;
    let mut walrus = ModuleConfig::new()
        .generate_producers_section(false)
        .parse(&wasm)?;
    let mut cx = super::Context::new(&mut walrus)?;
    for directive in test.directives.iter() {
        match &directive.kind {
            DirectiveKind::Export(name) => {
                let export = walrus
                    .exports
                    .iter()
                    .find(|e| e.name == *name)
                    .ok_or_else(|| anyhow!("failed to find export"))?;
                cx.export_xform(export.id(), &directive.args, directive.ret_externref);
            }
            DirectiveKind::Import(module, field) => {
                let import = walrus
                    .imports
                    .iter()
                    .find(|e| e.module == *module && e.name == *field)
                    .ok_or_else(|| anyhow!("failed to find export"))?;
                cx.import_xform(import.id(), &directive.args, directive.ret_externref);
            }
        }
    }
    cx.run(&mut walrus)?;
    walrus::passes::gc::run(&mut walrus);
    let printed = wasm_text_printer::print_bytes(&walrus.emit_wasm())?;
    Ok(printed)
}

fn run_test(test: &Path) -> Result<()> {
    let expected = Test::from_file(test)?;
    let actual = runtest(&expected)?;
    expected.check(&actual)
}

#[test]
fn anyref_param_owned() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/anyref-param-owned.wat"
    )))
}

#[test]
fn anyref_param() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/anyref-param.wat"
    )))
}

#[test]
fn clone_ref_intrinsic() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/clone-ref-intrinsic.wat"
    )))
}

#[test]
fn drop_ref_intrinsic() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/drop-ref-intrinsic.wat"
    )))
}

#[test]
fn import_anyref_owned() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/import-anyref-owned.wat"
    )))
}

#[test]
fn import_anyref_ret() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/import-anyref-ret.wat"
    )))
}

#[test]
fn import_anyref() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/import-anyref.wat"
    )))
}

#[test]
fn mixed_export() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/mixed-export.wat"
    )))
}

#[test]
fn mixed() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/mixed.wat"
    )))
}

#[test]
fn ret_anyref() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/ret-anyref.wat"
    )))
}

#[test]
fn table_grow_intrinsic() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/table-grow-intrinsic.wat"
    )))
}

#[test]
fn table_set_null_intrinsic() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/table-set-null-intrinsic.wat"
    )))
}

#[test]
fn tee_before_grow() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/externref/tests/tee-before-grow.wat"
    )))
}

struct Test {
    file: PathBuf,
    directives: Vec<Directive>,
    assertion: Option<String>,
}

struct Directive {
    args: Vec<(usize, bool)>,
    ret_externref: bool,
    kind: DirectiveKind,
}

enum DirectiveKind {
    Import(String, String),
    Export(String),
}

impl Test {
    fn from_file(path: &Path) -> Result<Test> {
        let contents = fs::read_to_string(path)?;
        let mut iter = contents.lines();
        let mut assertion = None;
        let mut directives = Vec::new();
        while let Some(line) = iter.next() {
            if line.starts_with("(; CHECK-ALL:") {
                let mut pattern = String::new();
                for line in iter.by_ref() {
                    if line == ";)" {
                        break;
                    }
                    pattern.push_str(line);
                    pattern.push('\n');
                }
                if iter.next().is_some() {
                    bail!("CHECK-ALL must be at the end of the file");
                }
                assertion = Some(pattern);
                continue;
            }

            if !line.starts_with(";; @xform") {
                continue;
            }
            let directive = &line[9..];
            let buf = wasm_text_parser::parser::ParseBuffer::new(directive)?;
            directives.push(wasm_text_parser::parser::parse::<Directive>(&buf)?);
        }
        Ok(Test {
            file: path.to_path_buf(),
            directives,
            assertion,
        })
    }

    fn check(&self, output: &str) -> Result<()> {
        if let Some(pattern) = &self.assertion {
            if output == pattern {
                return Ok(());
            }
            bail!(
                "expected\n    {}\n\nactual\n    {}",
                pattern.replace('\n', "\n    "),
                output.replace('\n', "\n    ")
            );
        } else {
            bail!("no CHECK-ALL assertion in fixture");
        }
    }
}

impl<'a> Parse<'a> for Directive {
    fn parse(parser: Parser<'a>) -> wasm_text_parser::parser::Result<Self> {
        use wasm_text_parser::kw;
        wasm_text_parser::custom_keyword!(externref_owned);
        wasm_text_parser::custom_keyword!(externref_borrowed);
        wasm_text_parser::custom_keyword!(other);

        let kind = if parser.peek::<kw::import>()? {
            parser.parse::<kw::import>()?;
            DirectiveKind::Import(parser.parse()?, parser.parse()?)
        } else {
            parser.parse::<kw::export>()?;
            DirectiveKind::Export(parser.parse()?)
        };
        let mut args = Vec::new();
        parser.parens(|p| {
            let mut i = 0;
            while !p.is_empty() {
                if parser.peek::<externref_owned>()? {
                    parser.parse::<externref_owned>()?;
                    args.push((i, true));
                } else if parser.peek::<externref_borrowed>()? {
                    parser.parse::<externref_borrowed>()?;
                    args.push((i, false));
                } else {
                    parser.parse::<other>()?;
                }
                i += 1;
            }
            Ok(())
        })?;

        let ret_externref = parser.parse::<Option<externref_owned>>()?.is_some();
        Ok(Directive {
            args,
            ret_externref,
            kind,
        })
    }
}
