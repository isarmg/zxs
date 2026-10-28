// Copyright 2012-2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Functions for computing canonical and compatible decompositions for Unicode characters.
use crate::unicode::lookups::{canonical_fully_decomposed, composition_table};

use core::char;

/// Compute canonical Unicode decomposition for character.
/// See [Unicode Standard Annex #15](http://www.unicode.org/reports/tr15/)
/// for more information.
#[inline]
pub fn decompose_canonical<F>(c: char, emit_char: F)
where
    F: FnMut(char),
{
    decompose(c, canonical_fully_decomposed, emit_char)
}

#[inline]
fn decompose<D, F>(c: char, decompose_char: D, mut emit_char: F)
where
    D: Fn(char) -> Option<&'static [char]>,
    F: FnMut(char),
{
    // 7-bit ASCII never decomposes
    if c <= '\x7f' {
        emit_char(c);
        return;
    }

    // Perform decomposition for Hangul
    if is_hangul_syllable(c) {
        decompose_hangul(c, emit_char);
        return;
    }

    if let Some(decomposed) = decompose_char(c) {
        for &d in decomposed {
            emit_char(d);
        }
        return;
    }

    // Finally bottom out.
    emit_char(c);
}

/// Compose two characters into a single character, if possible.
/// See [Unicode Standard Annex #15](http://www.unicode.org/reports/tr15/)
/// for more information.
pub fn compose(a: char, b: char) -> Option<char> {
    compose_hangul(a, b).or_else(|| composition_table(a, b))
}

// Constants from Unicode 9.0.0 Section 3.12 Conjoining Jamo Behavior
// http://www.unicode.org/versions/Unicode9.0.0/ch03.pdf#M9.32468.Heading.310.Combining.Jamo.Behavior
const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

const S_LAST: u32 = S_BASE + S_COUNT - 1;
const L_LAST: u32 = L_BASE + L_COUNT - 1;
const V_LAST: u32 = V_BASE + V_COUNT - 1;
const T_LAST: u32 = T_BASE + T_COUNT - 1;

// Composition only occurs for `TPart`s in `U+11A8 ..= U+11C2`,
// i.e. `T_BASE + 1 ..= T_LAST`.
const T_FIRST: u32 = T_BASE + 1;

// Safety-usable invariant: This ensures that c is a valid Hangul Syllable character (U+AC00..U+D7AF)
pub(crate) fn is_hangul_syllable(c: char) -> bool {
    // Safety: This checks the range 0xAC00 (S_BASE) to 0xD7A4 (S_BASE + S_COUNT), upholding the safety-usable invariant
    (c as u32) >= S_BASE && (c as u32) < (S_BASE + S_COUNT)
}

// Decompose a precomposed Hangul syllable
// Called only after checking the Hangul syllable range U+AC00..U+D7AF.
#[inline(always)]
fn decompose_hangul<F>(s: char, mut emit_char: F)
where
    F: FnMut(char),
{
    debug_assert!(is_hangul_syllable(s));
    // This will be at most 0x2baf, the size of the Hangul Syllables block
    let s_index = s as u32 - S_BASE;
    // This will be at most 0x2baf / (21 * 28), 19
    let l_index = s_index / N_COUNT;
    // Each Hangul Jamo result remains below the surrogate range. Checked
    // conversions retain that invariant without unsafe code in naming.
    emit_char(char::from_u32(L_BASE + l_index).expect("valid leading Hangul Jamo"));

    let v_index = (s_index % N_COUNT) / T_COUNT;
    emit_char(char::from_u32(V_BASE + v_index).expect("valid vowel Hangul Jamo"));

    let t_index = s_index % T_COUNT;
    if t_index > 0 {
        emit_char(char::from_u32(T_BASE + t_index).expect("valid trailing Hangul Jamo"));
    }
}

// Compose a pair of Hangul Jamo
#[inline(always)]
#[allow(ellipsis_inclusive_range_patterns)]
fn compose_hangul(a: char, b: char) -> Option<char> {
    let (a, b) = (a as u32, b as u32);
    match (a, b) {
        // Compose a leading consonant and a vowel together into an LV_Syllable
        (L_BASE..=L_LAST, V_BASE..=V_LAST) => {
            // Safety: based on the above bounds, l_index will be less than or equal to L_COUNT (19)
            // and v_index will be <= V_COUNT (21)
            let l_index = a - L_BASE;
            let v_index = b - V_BASE;
            // Safety: This will be <= 19 * (20 * 21) + (21 * 20), which is 8400.
            let lv_index = l_index * N_COUNT + v_index * T_COUNT;
            // Safety: This is between 0xAC00 and 0xCCD0, which are in range for Hangul Syllables (U+AC00..U+D7AF) and also in range
            // for BMP unicode
            let s = S_BASE + lv_index;
            // Safety: We've verified this is in-range
            char::from_u32(s)
        }
        // Compose an LV_Syllable and a trailing consonant into an LVT_Syllable
        (S_BASE..=S_LAST, T_FIRST..=T_LAST) if (a - S_BASE).is_multiple_of(T_COUNT) => {
            // Safety: a is between 0xAC00 and (0xAC00 + 19 * 21 * 28). b - T_BASE is between 0 and 19.
            // Adding a number 0 to 19 to a number that is at largest 0xD7A4 will not go out of bounds to 0xD800 (where the
            // surrogates start), so this is safe.
            char::from_u32(a + (b - T_BASE))
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "../../../../test/naming/unit/unicode/normalize/tests.rs"]
mod tests;
