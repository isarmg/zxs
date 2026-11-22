// ZXS adaptation of RustCrypto comparisons; see tag_choice/ORIGIN.md.
use super::super::{CmovEq, Condition};
use core::arch::asm;

macro_rules! cmov_eq {
    ($xor:expr, $instruction:expr, $lhs:expr, $rhs:expr, $condition:expr, $dst:expr) => {
        let mut tmp = u16::from(*$dst);
        let condition = u16::from($condition);
        // SAFETY: the original register-only compare has no memory or stack operands.
        unsafe {
            asm! {
                $xor,
                $instruction,
                inout(reg) *$lhs => _,
                in(reg) *$rhs,
                inlateout(reg) tmp,
                in(reg) condition,
                options(pure, nomem, nostack),
            };
        }

        *$dst = (tmp & 0xFF) as u8;
    };
}

impl CmovEq for u16 {
    #[inline]
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cmov_eq!(
            "xor {0:x}, {1:x}",
            "cmovz {2:e}, {3:e}",
            self,
            rhs,
            input,
            output
        );
    }

    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cmov_eq!(
            "xor {0:x}, {1:x}",
            "cmovnz {2:e}, {3:e}",
            self,
            rhs,
            input,
            output
        );
    }
}

impl CmovEq for u32 {
    #[inline]
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cmov_eq!(
            "xor {0:e}, {1:e}",
            "cmovz {2:e}, {3:e}",
            self,
            rhs,
            input,
            output
        );
    }

    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cmov_eq!(
            "xor {0:e}, {1:e}",
            "cmovnz {2:e}, {3:e}",
            self,
            rhs,
            input,
            output
        );
    }
}

#[cfg(target_arch = "x86")]
impl CmovEq for u64 {
    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        let lo = (*self & u32::MAX as u64) as u32;
        let hi = (*self >> 32) as u32;

        let mut tmp = 1u8;
        lo.cmovne(&((*rhs & u32::MAX as u64) as u32), 0, &mut tmp);
        hi.cmovne(&((*rhs >> 32) as u32), 0, &mut tmp);
        tmp.cmoveq(&0, input, output);
    }

    #[inline]
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        let lo = (*self & u32::MAX as u64) as u32;
        let hi = (*self >> 32) as u32;

        let mut tmp = 1u8;
        lo.cmovne(&((*rhs & u32::MAX as u64) as u32), 0, &mut tmp);
        hi.cmovne(&((*rhs >> 32) as u32), 0, &mut tmp);
        tmp.cmoveq(&1, input, output);
    }
}

#[cfg(target_arch = "x86_64")]
impl CmovEq for u64 {
    #[inline]
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cmov_eq!(
            "xor {0:r}, {1:r}",
            "cmovz {2:r}, {3:r}",
            self,
            rhs,
            input,
            output
        );
    }

    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        cmov_eq!(
            "xor {0:r}, {1:r}",
            "cmovnz {2:r}, {3:r}",
            self,
            rhs,
            input,
            output
        );
    }
}
