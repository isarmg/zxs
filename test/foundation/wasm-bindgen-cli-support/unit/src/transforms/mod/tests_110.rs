use std::path::Path;

pub(crate) fn parse_wat_text(
    text: &str,
    path: Option<&Path>,
) -> Result<Vec<u8>, wasm_text_parser::Error> {
    let result = (|| {
        let buffer = wasm_text_parser::parser::ParseBuffer::new(text)?;
        let mut wat = wasm_text_parser::parser::parse::<wasm_text_parser::Wat>(&buffer)?;
        wat.encode()
    })();
    result.map_err(|mut error| {
        error.set_text(text);
        if let Some(path) = path {
            error.set_path(path);
        }
        error
    })
}

pub(crate) fn parse_wat_file(path: &Path) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context;
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read WAT fixture {}", path.display()))?;
    Ok(parse_wat_text(&text, Some(path))?)
}

pub(crate) fn parse_wat(wat: &str) -> walrus::Module {
    let wasm = parse_wat_text(wat, None).unwrap();
    walrus::ModuleConfig::new()
        .generate_producers_section(false)
        .parse(&wasm)
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_eh_version_none() {
        let wat = r#"
            (module
                (func $foo
                    i32.const 1
                    drop
                )
                (export "foo" (func $foo))
            )
        "#;
        let module = parse_wat(wat);
        assert_eq!(
            detect_exception_handling_version(&module, false),
            ExceptionHandlingVersion::None
        );
    }

    #[test]
    fn detect_eh_version_legacy() {
        // Legacy EH uses try/catch_all/end syntax
        let wat = r#"
            (module
                (func $foo
                    try
                        i32.const 1
                        drop
                    catch_all
                    end
                )
                (export "foo" (func $foo))
            )
        "#;
        let module = parse_wat(wat);
        assert_eq!(
            detect_exception_handling_version(&module, false),
            ExceptionHandlingVersion::Legacy
        );
    }

    #[test]
    fn detect_eh_version_modern() {
        // Modern EH uses try_table with catch_all targeting a block label
        let wat = r#"
            (module
                (func $foo
                    (block $catch
                        (try_table (catch_all $catch)
                            i32.const 1
                            drop
                        )
                    )
                )
                (export "foo" (func $foo))
            )
        "#;
        let module = parse_wat(wat);
        assert_eq!(
            detect_exception_handling_version(&module, false),
            ExceptionHandlingVersion::Modern
        );
    }

    #[test]
    fn detect_eh_version_both_prefers_modern() {
        // If a module somehow has both, we prefer modern
        let wat = r#"
            (module
                (func $foo
                    try
                        i32.const 1
                        drop
                    catch_all
                    end
                )
                (func $bar
                    (block $catch
                        (try_table (catch_all $catch)
                            i32.const 1
                            drop
                        )
                    )
                )
                (export "foo" (func $foo))
                (export "bar" (func $bar))
            )
        "#;
        let module = parse_wat(wat);
        assert_eq!(
            detect_exception_handling_version(&module, false),
            ExceptionHandlingVersion::Modern
        );
    }
}
