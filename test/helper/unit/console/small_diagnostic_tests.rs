use super::*;
use std::cell::Cell;

#[test]
fn exact_ceiling_and_atomic_escapes_leave_a_valid_utf8_marker() {
    assert_eq!(SmallDiagnostic::display("x".repeat(MAX_MESSAGE_BYTES)).into_string(),
        "x".repeat(MAX_MESSAGE_BYTES));
    assert_eq!(SmallDiagnostic::display("a\n\u{1b}😀").into_string(), "a\\n\\u{1b}😀");
    let input = format!("{}\u{1b}😀", "x".repeat(MAX_MESSAGE_BYTES - 5));
    let result = SmallDiagnostic::display(input).into_string();
    assert!(result.len() <= MAX_MESSAGE_BYTES);
    assert_eq!(result, format!("{}…", "x".repeat(MAX_MESSAGE_BYTES - 5)));
}

struct Stream(Cell<usize>);
impl fmt::Display for Stream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for _ in 0..100_000 {
            self.0.set(self.0.get() + 1);
            formatter.write_str("escape \"\\ 源\n")?;
        }
        Ok(())
    }
}

#[test]
fn variable_display_stops_rendering_before_building_an_unbounded_string() {
    let value = Stream(Cell::new(0));
    let message = SmallDiagnostic::display(&value).into_string();
    assert!(message.len() <= MAX_MESSAGE_BYTES);
    assert!(message.ends_with(SHORTENED));
    assert!(!message.contains('\n'));
    assert!(value.0.get() < MAX_MESSAGE_BYTES);
}

#[derive(Debug)]
struct Cycle(Cell<usize>);
impl fmt::Display for Cycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.set(self.0.get() + 1);
        formatter.write_str("real cause")
    }
}
impl Error for Cycle {
    fn source(&self) -> Option<&(dyn Error + 'static)> { Some(self) }
}
#[test]
fn a_short_message_per_node_cannot_make_an_unbounded_or_cyclic_cause_chain() {
    let cycle = Cycle(Cell::new(0));
    let message = SmallDiagnostic::chain(&cycle).into_string();
    assert_eq!(cycle.0.get(), MAX_CAUSES);
    assert_eq!(message.matches("real cause").count(), MAX_CAUSES);
    assert!(message.ends_with(SHORTENED));
    assert_eq!(SmallDiagnostic::chain(&std::io::Error::other("short")).into_string(), "short");
}
