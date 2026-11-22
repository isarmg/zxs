// ZXS adaptation of RustCrypto comparisons; see tag_choice/ORIGIN.md.
//! Byte choice for SMB tag equality only.

mod backends;
mod slice;

type Condition = u8;

trait CmovEq {
    /// Move if both inputs are not equal.
    ///
    /// Moves `input` to `output` in constant-time if `self` and `rhs` are NOT equal.
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        let mut tmp = 1u8;
        self.cmoveq(rhs, 0u8, &mut tmp);
        tmp.cmoveq(&1u8, input, output);
    }

    /// Move if both inputs are equal.
    ///
    /// Moves `input` to `output` in constant-time if `self` and `rhs` are equal.
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        let mut tmp = 1u8;
        self.cmovne(rhs, 0u8, &mut tmp);
        tmp.cmoveq(&1, input, output);
    }
}

impl CmovEq for u8 {
    #[inline]
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        u16::from(*self).cmoveq(&u16::from(*rhs), input, output);
    }

    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        u16::from(*self).cmovne(&u16::from(*rhs), input, output);
    }
}

#[inline]
pub(super) fn tag_equal_choice(expected: &[u8], supplied: &[u8]) -> u8 {
    let mut choice = 0;
    expected.cmoveq(supplied, 1, &mut choice);
    choice
}

#[cfg(test)]
#[path = "../../../tests/unit/attached/smb/tag_choice/tests.rs"]
mod tests;
