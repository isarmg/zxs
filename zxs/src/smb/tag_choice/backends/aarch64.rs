// ZXS adaptation of RustCrypto comparisons; see tag_choice/ORIGIN.md.
use super::super::{CmovEq, Condition};
use core::arch::asm;

macro_rules! cseleq {
    ($eor:expr, $cmp:expr, $instruction:expr, $lhs:expr, $rhs:expr, $condition:expr, $dst:expr) => {
        let mut tmp = u16::from(*$dst);
        let condition = u16::from($condition);
        // SAFETY: the original register-only select has no memory or stack operands.
        unsafe {
            asm! {
                $eor,
                $cmp,
                $instruction,
                out(reg) _,
                in(reg) *$lhs,
                in(reg) *$rhs,
                inlateout(reg) tmp,
                in(reg) condition,
                in(reg) tmp,
                options(pure, nomem, nostack),
            };
        };

        *$dst = (tmp & 0xFF) as u8;
    };
}

macro_rules! cseleq16 {
    ($instruction:expr, $lhs:expr, $rhs:expr, $condition:expr, $dst:expr) => {
        cseleq!(
            "eor {0:w}, {1:w}, {2:w}",
            "tst {0:w}, 0xffff",
            $instruction,
            $lhs,
            $rhs,
            $condition,
            $dst
        )
    };
}

macro_rules! cseleq32 {
    ($instruction:expr, $lhs:expr, $rhs:expr, $condition:expr, $dst:expr) => {
        cseleq!(
            "eor {0:w}, {1:w}, {2:w}",
            "cmp {0:w}, 0",
            $instruction,
            $lhs,
            $rhs,
            $condition,
            $dst
        )
    };
}

macro_rules! cseleq64 {
    ($instruction:expr, $lhs:expr, $rhs:expr, $condition:expr, $dst:expr) => {
        cseleq!(
            "eor {0:x}, {1:x}, {2:x}",
            "cmp {0:x}, 0",
            $instruction,
            $lhs,
            $rhs,
            $condition,
            $dst
        )
    };
}

impl CmovEq for u16 {
    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cseleq16!("csel {3:w}, {4:w}, {5:w}, NE", self, rhs, input, output);
    }

    #[inline]
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cseleq16!("csel {3:w}, {4:w}, {5:w}, EQ", self, rhs, input, output);
    }
}

impl CmovEq for u32 {
    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cseleq32!("csel {3:w}, {4:w}, {5:w}, NE", self, rhs, input, output);
    }

    #[inline]
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cseleq32!("csel {3:w}, {4:w}, {5:w}, EQ", self, rhs, input, output);
    }
}

impl CmovEq for u64 {
    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cseleq64!("csel {3:x}, {4:x}, {5:x}, NE", self, rhs, input, output);
    }

    #[inline]
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cseleq64!("csel {3:x}, {4:x}, {5:x}, EQ", self, rhs, input, output);
    }
}
