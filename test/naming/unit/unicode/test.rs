// Copyright 2012-2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use super::UnicodeNormalization;
use super::decompose::Decompositions;

fn nfd(input: &str) -> String {
    Decompositions::new_canonical(input.chars()).collect()
}

#[test]
fn test_nfd() {
    macro_rules! t {
        ($input: expr, $expected: expr) => {
            assert_eq!(nfd($input), $expected);
            // An iterator that is not std::str::Chars directly.
            assert_eq!(
                Decompositions::new_canonical($input.chars().map(|c| c)).collect::<String>(),
                $expected
            );
        };
    }
    t!("abc", "abc");
    t!("\u{1e0b}\u{1c4}", "d\u{307}\u{1c4}");
    t!("\u{2026}", "\u{2026}");
    t!("\u{2126}", "\u{3a9}");
    t!("\u{1e0b}\u{323}", "d\u{323}\u{307}");
    t!("\u{1e0d}\u{307}", "d\u{323}\u{307}");
    t!("a\u{301}", "a\u{301}");
    t!("\u{301}a", "\u{301}a");
    t!("\u{d4db}", "\u{1111}\u{1171}\u{11b6}");
    t!("\u{ac1c}", "\u{1100}\u{1162}");
}

#[test]
fn test_nfc() {
    macro_rules! t {
        ($input: expr, $expected: expr) => {
            assert_eq!($input.nfc().collect::<String>(), $expected);
        };
    }
    t!("abc", "abc");
    t!("\u{1e0b}\u{1c4}", "\u{1e0b}\u{1c4}");
    t!("\u{2026}", "\u{2026}");
    t!("\u{2126}", "\u{3a9}");
    t!("\u{1e0b}\u{323}", "\u{1e0d}\u{307}");
    t!("\u{1e0d}\u{307}", "\u{1e0d}\u{307}");
    t!("a\u{301}", "\u{e1}");
    t!("\u{301}a", "\u{301}a");
    t!("\u{d4db}", "\u{d4db}");
    t!("\u{ac1c}", "\u{ac1c}");
    t!(
        "a\u{300}\u{305}\u{315}\u{5ae}b",
        "\u{e0}\u{5ae}\u{305}\u{315}b"
    );
}

#[test]
fn test_normalize_char() {
    assert_eq!(nfd("\u{2126}"), "\u{3a9}")
}
