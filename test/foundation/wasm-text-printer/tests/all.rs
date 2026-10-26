mod common;

#[test]
fn no_oom() {
    // Whatever is printed here, it shouldn't take more than 500MB to print
    // since it's only 20k functions.
    let mut s = String::new();
    s.push_str("(module\n");
    for _ in 0..20_000 {
        s.push_str("(func i32.const 0 if i32.const 0 else i32.const 0 end)\n");
    }
    s.push(')');
    let bytes = common::parse_wat(&s).unwrap();
    let wat = wasm_text_printer::print_bytes(&bytes).unwrap();
    assert!(wat.len() < 500_000_000);
}
