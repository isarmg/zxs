use super::*;

#[test]
fn assert_instruction_not_too_large() {
    let size = std::mem::size_of::<Instruction<'_>>();
    let pointer = std::mem::size_of::<u64>();
    assert!(size <= pointer * 11);
}
