use super::*;

fn hex<const N:usize>(s:&str)->[u8;N]{
    assert_eq!(s.len(),2*N);
    std::array::from_fn(|i|u8::from_str_radix(&s[2*i..2*i+2],16).unwrap())
}

#[test]
fn bundled_argon2_accepts_exact_legacy_verifiers(){
    // Use the existing SQLite fixture to check verifier interoperability.
    let text=include_str!("../../../sqlite/first-account-v1.txt");
    let value=|key:&str|text.lines().filter_map(|s|s.split_once('=')).find(|(k,_)|*k==key).unwrap().1;
    let salt=hex(value("web_salt"));let tag=hex(value("web_verifier"));
    let legacy=Verifier::new(salt,tag);
    assert!(legacy.matches("correct horse battery staple").unwrap());
    assert!(!legacy.matches("correct horse battery stapler").unwrap());
    let mut wrong_tag=tag;wrong_tag[31]^=1;
    assert!(!Verifier::new(salt,wrong_tag).matches("correct horse battery staple").unwrap());

    // Independently recorded from the authenticated Debian trixie DSO,
    // preserving byte length, embedded NUL and exact UTF-8 spelling.
    let unicode=Verifier::new(hex("000102030405060708090a0b0c0d0e0f"),
        hex("0225c3954d8493e58cba41547b19de49f3079a9507b30b4b687779eb13ee7fe8"));
    assert!(unicode.matches("café\0中文").unwrap());
    assert!(!unicode.matches("café").unwrap());
    assert!(!unicode.matches("cafe\u{301}\0中文").unwrap());
    assert!(!unicode.matches("").unwrap());
    assert!(!unicode.matches(&"x".repeat(1025)).unwrap());
}
