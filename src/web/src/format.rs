use json_codec::Value;
use web_interface::i18n::t;

pub fn bytes(value: u64) -> String {
    if value < 1024 {
        return format!("{value} B");
    }
    let mut n = value as f64;
    let mut index: usize = 0;
    let units = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    while n >= 1024. && index < units.len() {
        n /= 1024.;
        index += 1;
    }
    format!("{n:.1} {}", units[index - 1])
}
pub fn parse_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => {
            if s == "0" || !s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit()) {
                s.parse().ok()
            } else {
                None
            }
        }
        _ => None,
    }
}
pub fn timestamp(v: &str) -> String {
    if v == "—" || v.is_empty() {
        t("尚无记录", "No record yet")
    } else {
        v.to_owned()
    }
}
#[cfg(test)]
#[path = "../../../test/web/unit/format/tests.rs"]
mod tests;
