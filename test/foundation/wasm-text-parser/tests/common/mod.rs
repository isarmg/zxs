pub fn parse_wat(text: &str) -> Result<Vec<u8>, wasm_text_parser::Error> {
    let result = (|| {
        let buffer = wasm_text_parser::parser::ParseBuffer::new(text)?;
        let mut wat = wasm_text_parser::parser::parse::<wasm_text_parser::Wat>(&buffer)?;
        wat.encode()
    })();
    result.map_err(|mut error| {
        error.set_text(text);
        error
    })
}
