//! Immutable original ZXS release plan and escaped local preview.
use std::fmt::Write as _;
pub(crate) mod first;
pub(crate) mod rust_first;
pub(crate) fn escaped(out: &mut String, name: &[u8]) {
    match std::str::from_utf8(name) {
        Ok(name) => {
            for c in name.chars() {
                if c.is_control()
                    || matches!(c, '\u{061c}'|'\u{200b}'..='\u{200f}'|'\u{2028}'..='\u{202e}'|'\u{2060}'..='\u{206f}'|'\u{feff}')
                {
                    write!(out, "\\u{{{:x}}}", c as u32).expect("writing a String");
                } else {
                    if matches!(c, '\\' | '"') {
                        out.push('\\');
                    }
                    out.push(c);
                }
            }
        }
        Err(_) => out.extend(
            name.iter()
                .copied()
                .flat_map(std::ascii::escape_default)
                .map(char::from),
        ),
    }
}
