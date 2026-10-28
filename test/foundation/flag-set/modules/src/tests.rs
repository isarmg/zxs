#[path = "tests/all.rs"]
mod all;
#[path = "tests/all_named.rs"]
mod all_named;
#[path = "tests/bitflags_match.rs"]
mod bitflags_match;
#[path = "tests/bits.rs"]
mod bits;
#[path = "tests/clear.rs"]
mod clear;
#[path = "tests/complement.rs"]
mod complement;
#[path = "tests/contains.rs"]
mod contains;
#[path = "tests/difference.rs"]
mod difference;
#[path = "tests/empty.rs"]
mod empty;
#[path = "tests/eq.rs"]
mod eq;
#[path = "tests/extend.rs"]
mod extend;
#[path = "tests/flag_name.rs"]
mod flag_name;
#[path = "tests/flags.rs"]
mod flags;
#[path = "tests/fmt.rs"]
mod fmt;
#[path = "tests/from_bits.rs"]
mod from_bits;
#[path = "tests/from_bits_retain.rs"]
mod from_bits_retain;
#[path = "tests/from_bits_truncate.rs"]
mod from_bits_truncate;
#[path = "tests/from_name.rs"]
mod from_name;
#[path = "tests/insert.rs"]
mod insert;
#[path = "tests/intersection.rs"]
mod intersection;
#[path = "tests/intersects.rs"]
mod intersects;
#[path = "tests/is_all.rs"]
mod is_all;
#[path = "tests/is_empty.rs"]
mod is_empty;
#[path = "tests/iter.rs"]
mod iter;
#[path = "tests/iter_equal_names.rs"]
mod iter_equal_names;
#[path = "tests/known_bits.rs"]
mod known_bits;
#[path = "tests/parser.rs"]
mod parser;
#[path = "tests/remove.rs"]
mod remove;
#[path = "tests/symmetric_difference.rs"]
mod symmetric_difference;
#[path = "tests/truncate.rs"]
mod truncate;
#[path = "tests/union.rs"]
mod union;
#[path = "tests/unknown.rs"]
mod unknown;
#[path = "tests/unknown_bits.rs"]
mod unknown_bits;

mod custom {
    pub const NAME: &'static str = "custom";
}

bitflags! {
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    pub struct TestFlags: u8 {
        /// 1
        const A = 1;

        /// 1 << 1
        const B = 1 << 1;

        /// 1 << 2
        const C = 1 << 2;

        /// 1 | (1 << 1) | (1 << 2)
        const ABC = Self::A.bits() | Self::B.bits() | Self::C.bits();
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    pub struct TestFlagsInvert: u8 {
        /// 1 | (1 << 1) | (1 << 2)
        const ABC = Self::A.bits() | Self::B.bits() | Self::C.bits();

        /// 1
        const A = 1;

        /// 1 << 1
        const B = 1 << 1;

        /// 1 << 2
        const C = 1 << 2;
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    pub struct TestZero: u8 {
        /// 0
        const ZERO = 0;
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    pub struct TestZeroOne: u8 {
        /// 0
        const ZERO = 0;

        /// 1
        const ONE = 1;
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    pub struct TestUnicode: u8 {
        /// 1
        const 一 = 1;

        /// 2
        const 二 = 1 << 1;
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    pub struct TestEmpty: u8 {}

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    pub struct TestOverlapping: u8 {
        /// 1 | (1 << 1)
        const AB = 1 | (1 << 1);

        /// (1 << 1) | (1 << 2)
        const BC = (1 << 1) | (1 << 2);
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    pub struct TestOverlappingFull: u8 {
        /// 1
        const A = 1;

        /// 1
        const B = 1;

        /// 1
        const C = 1;

        /// 2
        const D = 1 << 1;
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    pub struct TestExternal: u8 {
        /// 1
        const A = 1;

        /// 1 << 1
        const B = 1 << 1;

        /// 1 << 2
        const C = 1 << 2;

        /// 1 | (1 << 1) | (1 << 2)
        const ABC = Self::A.bits() | Self::B.bits() | Self::C.bits();

        /// External
        const _ = !0;
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    pub struct TestExternalFull: u8 {
        /// External
        const _ = !0;
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    pub struct TestRenamed: u8 {
        /// 1
        #[bitflags(flag_name = "a")]
        #[bitflags(flag_name = custom::NAME)]
        const A = 1;
        /// 1 << 1
        #[bitflags(flag_name = "custom")]
        const B = 1 << 1;
        /// 1 << 2
        #[bitflags(flag_name = "c")]
        const C = 1 << 2;
        /// 1 << 3
        #[bitflags(flag_name = "custom | e")]
        const D = 1 << 3;
    }
}
