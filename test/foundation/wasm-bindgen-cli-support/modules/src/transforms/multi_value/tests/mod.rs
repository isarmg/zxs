//! Explicit fixture tests compare independent, immutable `CHECK-ALL` expectations.

use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};
use walrus::ModuleConfig;
use wasm_text_parser::parser::{Parse, Parser};

fn runtest(test: &Test) -> Result<String> {
    let wasm = crate::transforms::parse_wat_file(&test.file)?;
    let mut walrus = ModuleConfig::new()
        .generate_producers_section(false)
        .parse(&wasm)?;
    let mut exports = Vec::new();
    let mut xforms = Vec::new();
    for directive in test.directives.iter() {
        let export = walrus
            .exports
            .iter()
            .find(|e| e.name == directive.name)
            .unwrap();
        let id = match export.item {
            walrus::ExportItem::Function(id) => id,
            _ => panic!("must be function export"),
        };
        exports.push(export.id());
        xforms.push((id, 0, directive.tys.clone()));
    }
    let memory = walrus.memories.iter().next().unwrap().id();
    let stack_pointer = walrus.globals.iter().next().unwrap().id();
    let ret = super::run(&mut walrus, memory, stack_pointer, &xforms)?;
    for (export, id) in exports.into_iter().zip(ret) {
        walrus.exports.get_mut(export).item = walrus::ExportItem::Function(id);
    }
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
fn align() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/multi_value/tests/align.wat"
    )))
}

#[test]
fn many() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/multi_value/tests/many.wat"
    )))
}

#[test]
fn simple() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/multi_value/tests/simple.wat"
    )))
}

struct Test {
    file: PathBuf,
    directives: Vec<Directive>,
    assertion: Option<String>,
}

struct Directive {
    name: String,
    tys: Vec<walrus::ValType>,
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
        use wasm_text_parser::{core::ValType, kw};

        parser.parse::<kw::export>()?;
        let name = parser.parse()?;
        let mut tys = Vec::new();
        parser.parens(|p| {
            while !p.is_empty() {
                tys.push(match p.parse()? {
                    ValType::I32 => walrus::ValType::I32,
                    ValType::I64 => walrus::ValType::I64,
                    ValType::F32 => walrus::ValType::F32,
                    ValType::F64 => walrus::ValType::F64,
                    _ => panic!(),
                });
            }
            Ok(())
        })?;
        Ok(Directive { name, tys })
    }
}

#[test]
fn round_up_to_alignment_works() {
    for &(n, align, expected) in &[
        (0, 1, 0),
        (1, 1, 1),
        (2, 1, 2),
        (0, 2, 0),
        (1, 2, 2),
        (2, 2, 2),
        (3, 2, 4),
        (0, 4, 0),
        (1, 4, 4),
        (2, 4, 4),
        (3, 4, 4),
        (4, 4, 4),
        (5, 4, 8),
    ] {
        let actual = super::round_up_to_alignment(n, align);
        println!(
            "round_up_to_alignment(n = {n}, align = {align}) = {actual} (expected {expected})"
        );
        assert_eq!(actual, expected);
    }
}
