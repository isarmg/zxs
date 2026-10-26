//! Explicit fixture tests compare independent, immutable `CHECK-ALL` expectations.

use crate::transforms::unstart_start_function;
use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};
use walrus::ModuleConfig;

fn runtest(test: &Test) -> Result<String> {
    let wasm = crate::transforms::parse_wat_file(&test.file)?;
    let mut module = ModuleConfig::new()
        .generate_producers_section(false)
        .parse(&wasm)?;

    super::run(&mut module)?;
    walrus::passes::gc::run(&mut module);

    // We add an extra parameter to the start function, making it invalid for the start section.
    // It's only valid in combination with the "unstart" step.
    unstart_start_function(&mut module);

    let features = wasmparser::WasmFeatures::default() | wasmparser::WasmFeatures::THREADS;

    wasmparser::Validator::new_with_features(features).validate_all(&module.emit_wasm())?;

    let printed = wasm_text_printer::print_bytes(&module.emit_wasm())?;

    Ok(printed)
}

fn run_test(test: &Path) -> Result<()> {
    let expected = Test::from_file(test)?;
    let actual = runtest(&expected)?;
    expected.check(&actual)
}

#[test]
fn basic() -> Result<()> {
    run_test(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-bindgen-cli-support/modules/src/transforms/threads/tests/basic.wat"
    )))
}

struct Test {
    file: PathBuf,
    assertion: Option<String>,
}

impl Test {
    fn from_file(path: &Path) -> Result<Test> {
        let contents = fs::read_to_string(path)?;
        let mut iter = contents.lines();
        let mut assertion = None;
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
        }
        Ok(Test {
            file: path.to_path_buf(),
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
