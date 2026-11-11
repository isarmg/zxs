#![forbid(unsafe_code)]
//! Derived from wasm-bindgen-cli 0.2.129's wasm_bindgen module and binary.
//! Exact source mapping: compliance/provenance/packages/zxs-bindgen-0.2.129/PROVENANCE.json.
//! Upstream licenses: compliance/licenses/upstream/zxs-bindgen-0.2.129/.
//! ZXS only generates browser bindings; test servers and other targets are absent.
use anyhow::{bail, Result};
use command_line::{Parser, ValueEnum};
use std::{ffi::OsString, path::PathBuf};
use wasm_bindgen_cli_support::Bindgen;

#[derive(Debug, Clone, ValueEnum)]
enum Target {
    Web,
}

#[derive(Debug, Parser)]
#[command(name = "wasm-bindgen", version, about)]
struct Args {
    input: PathBuf,
    #[arg(long, value_enum, required = true)]
    target: Target,
    #[arg(long, value_name = "DIR")]
    out_dir: PathBuf,
    #[arg(
        long,
        required = true,
        help = "Generate JavaScript without TypeScript declarations"
    )]
    no_typescript: bool,
    #[arg(long, value_name = "NAME", value_parser = output_name)]
    out_name: String,
}

fn output_name(value: &str) -> std::result::Result<String, String> {
    // The upstream generator joins this stem into output paths and embeds it
    // in a quoted JavaScript URL. A path or script fragment is not a name.
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("output name must contain only ASCII letters, digits, '_' or '-'".into());
    }
    Ok(value.to_owned())
}

fn run<I, T>(arguments: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = match Args::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error) => match error.kind() {
            command_line::error::ErrorKind::DisplayHelp
            | command_line::error::ErrorKind::DisplayVersion => {
                print!("{error}");
                return Ok(());
            }
            _ => bail!(error),
        },
    };
    let mut bindgen = Bindgen::new();
    let Target::Web = args.target;
    debug_assert!(args.no_typescript, "required by the CLI output contract");
    // Match upstream CLI defaults exactly for the formal ZXS invocation.
    // Bindgen::new() differs here: it initially omits the default Wasm URL and
    // reads an externref environment override that the upstream CLI clears.
    #[allow(deprecated)]
    bindgen
        .input_path(&args.input)
        .out_name(&args.out_name)
        .omit_default_module_path(false)
        .reference_types(false);
    bindgen.generate(&args.out_dir)
}

fn main() {
    logging::init();
    if let Err(error) = run(std::env::args_os()) {
        eprintln!("error: {error:?}");
        std::process::exit(1);
    }
}

#[cfg(test)]
#[path = "../../../test/tools/web-bindings/unit/main_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/tools/web-bindings/unit/logging_tests.rs"]
mod logging_tests;
