// Adapted from Paho Lurie-Gregg and contributors (MIT OR Apache-2.0); see ORIGIN.md.
//! The original unsigned aliases for the admitted array widths.

use super::{B0, B1, UInt, UTerm};

pub type True = B1;
pub type False = B0;
pub type U0 = UTerm;
pub type U1 = UInt<UTerm, B1>;
pub type U2 = UInt<UInt<UTerm, B1>, B0>;
pub type U3 = UInt<UInt<UTerm, B1>, B1>;
pub type U4 = UInt<UInt<UInt<UTerm, B1>, B0>, B0>;
pub type U5 = UInt<UInt<UInt<UTerm, B1>, B0>, B1>;
pub type U6 = UInt<UInt<UInt<UTerm, B1>, B1>, B0>;
pub type U7 = UInt<UInt<UInt<UTerm, B1>, B1>, B1>;
pub type U8 = UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>;
pub type U9 = UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>;
pub type U10 = UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>;
pub type U11 = UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>;
pub type U12 = UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>;
pub type U13 = UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>;
pub type U14 = UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>;
pub type U15 = UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>;
pub type U16 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>;
pub type U17 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>;
pub type U18 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>;
pub type U19 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>;
pub type U20 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>;
pub type U21 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>;
pub type U22 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>;
pub type U23 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>;
pub type U24 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>;
pub type U25 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>;
pub type U26 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>;
pub type U27 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>;
pub type U28 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>;
pub type U29 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>;
pub type U30 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>;
pub type U31 = UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>;
pub type U32 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>;
pub type U33 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>;
pub type U34 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>;
pub type U35 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>;
pub type U36 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>;
pub type U37 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>;
pub type U38 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>;
pub type U39 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>;
pub type U40 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>;
pub type U41 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>;
pub type U42 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>;
pub type U43 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>;
pub type U44 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>;
pub type U45 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>;
pub type U46 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>;
pub type U47 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>;
pub type U48 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>;
pub type U49 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>;
pub type U50 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>;
pub type U51 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>;
pub type U52 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>;
pub type U53 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>;
pub type U54 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>;
pub type U55 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>;
pub type U56 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>;
pub type U57 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>;
pub type U58 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>;
pub type U59 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>;
pub type U60 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>;
pub type U61 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>;
pub type U62 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>;
pub type U63 = UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>;
pub type U64 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>;
pub type U65 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B1>;
pub type U66 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B0>;
pub type U67 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B1>;
pub type U68 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B0>;
pub type U69 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B1>;
pub type U70 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B0>;
pub type U71 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B1>;
pub type U72 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B0>;
pub type U73 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B1>;
pub type U74 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B0>;
pub type U75 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B1>;
pub type U76 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B0>;
pub type U77 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B1>;
pub type U78 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B0>;
pub type U79 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B1>;
pub type U80 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B0>;
pub type U81 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B1>;
pub type U82 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B0>;
pub type U83 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B1>;
pub type U84 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B0>;
pub type U85 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B1>;
pub type U86 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B0>;
pub type U87 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B1>;
pub type U88 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B0>;
pub type U89 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B1>;
pub type U90 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B0>;
pub type U91 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B1>;
pub type U92 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B0>;
pub type U93 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B1>;
pub type U94 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B0>;
pub type U95 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B1>;
pub type U96 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B0>;
pub type U97 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B1>;
pub type U98 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B0>;
pub type U99 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B1>;
pub type U100 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B0>;
pub type U101 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B1>;
pub type U102 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B0>;
pub type U103 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B1>;
pub type U104 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B0>;
pub type U105 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B1>;
pub type U106 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B0>;
pub type U107 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B1>;
pub type U108 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B0>;
pub type U109 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B1>;
pub type U110 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B0>;
pub type U111 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B1>;
pub type U112 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B0>;
pub type U113 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B1>;
pub type U114 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B0>;
pub type U115 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B1>;
pub type U116 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B0>;
pub type U117 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B1>;
pub type U118 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B0>;
pub type U119 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B1>;
pub type U120 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B0>;
pub type U121 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B1>;
pub type U122 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B0>;
pub type U123 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B1>;
pub type U124 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B0>;
pub type U125 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B1>;
pub type U126 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B0>;
pub type U127 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B1>;
pub type U128 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B0>;
pub type U129 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B1>;
pub type U130 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B1>, B0>;
pub type U131 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B1>, B1>;
pub type U132 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B0>, B0>;
pub type U133 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B0>, B1>;
pub type U134 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B1>, B0>;
pub type U135 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B1>, B1>;
pub type U136 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B0>, B0>;
pub type U137 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B0>, B1>;
pub type U138 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B1>, B0>;
pub type U139 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B1>, B1>;
pub type U140 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B0>, B0>;
pub type U141 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B0>, B1>;
pub type U142 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B1>, B0>;
pub type U143 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B1>, B1>;
pub type U144 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B0>, B0>;
pub type U145 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B0>, B1>;
pub type U146 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B1>, B0>;
pub type U147 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B1>, B1>;
pub type U148 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B0>, B0>;
pub type U149 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B0>, B1>;
pub type U150 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B1>, B0>;
pub type U151 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B1>, B1>;
pub type U152 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B0>, B0>;
pub type U153 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B0>, B1>;
pub type U154 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B1>, B0>;
pub type U155 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B1>, B1>;
pub type U156 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B0>, B0>;
pub type U157 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B0>, B1>;
pub type U158 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B1>, B0>;
pub type U159 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B1>, B1>;
pub type U160 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B0>, B0>;
pub type U161 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B0>, B1>;
pub type U162 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B1>, B0>;
pub type U163 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B1>, B1>;
pub type U164 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B0>, B0>;
pub type U165 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B0>, B1>;
pub type U166 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B1>, B0>;
pub type U167 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B1>, B1>;
pub type U168 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B0>, B0>;
pub type U169 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B0>, B1>;
pub type U170 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B1>, B0>;
pub type U171 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B1>, B1>;
pub type U172 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B0>, B0>;
pub type U173 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B0>, B1>;
pub type U174 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B1>, B0>;
pub type U175 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B1>, B1>;
pub type U176 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B0>, B0>;
pub type U177 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B0>, B1>;
pub type U178 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B1>, B0>;
pub type U179 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B1>, B1>;
pub type U180 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B0>, B0>;
pub type U181 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B0>, B1>;
pub type U182 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B1>, B0>;
pub type U183 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B1>, B1>;
pub type U184 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B0>, B0>;
pub type U185 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B0>, B1>;
pub type U186 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B1>, B0>;
pub type U187 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B1>, B1>;
pub type U188 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B0>, B0>;
pub type U189 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B0>, B1>;
pub type U190 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B1>, B0>;
pub type U191 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B1>, B1>;
pub type U192 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B0>, B0>;
pub type U193 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B0>, B1>;
pub type U194 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B1>, B0>;
pub type U195 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B1>, B1>;
pub type U196 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B0>, B0>;
pub type U197 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B0>, B1>;
pub type U198 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B1>, B0>;
pub type U199 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B1>, B1>;
pub type U200 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B0>, B0>;
pub type U201 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B0>, B1>;
pub type U202 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B1>, B0>;
pub type U203 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B1>, B1>;
pub type U204 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B0>, B0>;
pub type U205 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B0>, B1>;
pub type U206 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B1>, B0>;
pub type U207 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B1>, B1>;
pub type U208 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B0>, B0>;
pub type U209 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B0>, B1>;
pub type U210 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B1>, B0>;
pub type U211 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B1>, B1>;
pub type U212 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B0>, B0>;
pub type U213 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B0>, B1>;
pub type U214 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B1>, B0>;
pub type U215 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B1>, B1>;
pub type U216 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B0>, B0>;
pub type U217 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B0>, B1>;
pub type U218 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B1>, B0>;
pub type U219 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B1>, B1>;
pub type U220 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B0>, B0>;
pub type U221 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B0>, B1>;
pub type U222 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B1>, B0>;
pub type U223 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B1>, B1>;
pub type U224 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B0>, B0>;
pub type U225 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B0>, B1>;
pub type U226 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B1>, B0>;
pub type U227 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B1>, B1>;
pub type U228 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B0>, B0>;
pub type U229 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B0>, B1>;
pub type U230 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B1>, B0>;
pub type U231 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B1>, B1>;
pub type U232 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B0>, B0>;
pub type U233 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B0>, B1>;
pub type U234 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B1>, B0>;
pub type U235 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B1>, B1>;
pub type U236 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B0>, B0>;
pub type U237 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B0>, B1>;
pub type U238 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B1>, B0>;
pub type U239 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B1>, B1>;
pub type U240 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B0>, B0>;
pub type U241 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B0>, B1>;
pub type U242 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B1>, B0>;
pub type U243 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B1>, B1>;
pub type U244 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B0>, B0>;
pub type U245 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B0>, B1>;
pub type U246 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B1>, B0>;
pub type U247 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B1>, B1>;
pub type U248 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B0>, B0>;
pub type U249 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B0>, B1>;
pub type U250 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B1>, B0>;
pub type U251 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B1>, B1>;
pub type U252 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B0>, B0>;
pub type U253 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B0>, B1>;
pub type U254 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B1>, B0>;
pub type U255 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B1>, B1>;
pub type U256 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B0>, B0>;
pub type U257 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B0>, B1>;
pub type U258 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B1>, B0>;
pub type U259 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B1>, B1>;
pub type U260 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B1>, B0>, B0>;
pub type U261 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B1>, B0>, B1>;
pub type U262 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B1>, B1>, B0>;
pub type U263 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B1>, B1>, B1>;
pub type U264 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B0>, B0>, B0>;
pub type U265 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B0>, B0>, B1>;
pub type U266 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B0>, B1>, B0>;
pub type U267 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B0>, B1>, B1>;
pub type U268 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B1>, B0>, B0>;
pub type U269 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B1>, B0>, B1>;
pub type U270 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B1>, B1>, B0>;
pub type U271 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B1>, B1>, B1>;
pub type U272 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B0>, B0>, B0>;
pub type U273 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B0>, B0>, B1>;
pub type U274 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B0>, B1>, B0>;
pub type U275 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B0>, B1>, B1>;
pub type U276 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B1>, B0>, B0>;
pub type U277 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B1>, B0>, B1>;
pub type U278 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B1>, B1>, B0>;
pub type U279 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B1>, B1>, B1>;
pub type U280 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B0>, B0>, B0>;
pub type U281 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B0>, B0>, B1>;
pub type U282 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B0>, B1>, B0>;
pub type U283 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B0>, B1>, B1>;
pub type U284 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B1>, B0>, B0>;
pub type U285 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B1>, B0>, B1>;
pub type U286 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B1>, B1>, B0>;
pub type U287 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B1>, B1>, B1>;
pub type U288 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B0>, B0>, B0>;
pub type U289 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B0>, B0>, B1>;
pub type U290 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B0>, B1>, B0>;
pub type U291 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B0>, B1>, B1>;
pub type U292 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B1>, B0>, B0>;
pub type U293 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B1>, B0>, B1>;
pub type U294 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B1>, B1>, B0>;
pub type U295 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B1>, B1>, B1>;
pub type U296 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B0>, B0>, B0>;
pub type U297 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B0>, B0>, B1>;
pub type U298 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B0>, B1>, B0>;
pub type U299 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B0>, B1>, B1>;
pub type U300 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B1>, B0>, B0>;
pub type U301 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B1>, B0>, B1>;
pub type U302 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B1>, B1>, B0>;
pub type U303 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B1>, B1>, B1>;
pub type U304 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B0>, B0>, B0>;
pub type U305 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B0>, B0>, B1>;
pub type U306 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B0>, B1>, B0>;
pub type U307 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B0>, B1>, B1>;
pub type U308 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B1>, B0>, B0>;
pub type U309 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B1>, B0>, B1>;
pub type U310 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B1>, B1>, B0>;
pub type U311 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B1>, B1>, B1>;
pub type U312 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B0>, B0>, B0>;
pub type U313 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B0>, B0>, B1>;
pub type U314 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B0>, B1>, B0>;
pub type U315 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B0>, B1>, B1>;
pub type U316 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B1>, B0>, B0>;
pub type U317 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B1>, B0>, B1>;
pub type U318 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B1>, B1>, B0>;
pub type U319 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B1>, B1>, B1>;
pub type U320 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B0>, B0>, B0>;
pub type U321 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B0>, B0>, B1>;
pub type U322 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B0>, B1>, B0>;
pub type U323 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B0>, B1>, B1>;
pub type U324 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B1>, B0>, B0>;
pub type U325 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B1>, B0>, B1>;
pub type U326 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B1>, B1>, B0>;
pub type U327 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B1>, B1>, B1>;
pub type U328 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B0>, B0>, B0>;
pub type U329 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B0>, B0>, B1>;
pub type U330 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B0>, B1>, B0>;
pub type U331 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B0>, B1>, B1>;
pub type U332 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B1>, B0>, B0>;
pub type U333 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B1>, B0>, B1>;
pub type U334 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B1>, B1>, B0>;
pub type U335 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B1>, B1>, B1>;
pub type U336 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B0>, B0>, B0>;
pub type U337 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B0>, B0>, B1>;
pub type U338 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B0>, B1>, B0>;
pub type U339 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B0>, B1>, B1>;
pub type U340 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B1>, B0>, B0>;
pub type U341 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B1>, B0>, B1>;
pub type U342 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B1>, B1>, B0>;
pub type U343 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B1>, B1>, B1>;
pub type U344 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B0>, B0>, B0>;
pub type U345 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B0>, B0>, B1>;
pub type U346 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B0>, B1>, B0>;
pub type U347 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B0>, B1>, B1>;
pub type U348 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B1>, B0>, B0>;
pub type U349 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B1>, B0>, B1>;
pub type U350 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B1>, B1>, B0>;
pub type U351 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B1>, B1>, B1>;
pub type U352 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B0>, B0>, B0>;
pub type U353 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B0>, B0>, B1>;
pub type U354 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B0>, B1>, B0>;
pub type U355 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B0>, B1>, B1>;
pub type U356 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B1>, B0>, B0>;
pub type U357 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B1>, B0>, B1>;
pub type U358 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B1>, B1>, B0>;
pub type U359 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B1>, B1>, B1>;
pub type U360 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B0>, B0>, B0>;
pub type U361 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B0>, B0>, B1>;
pub type U362 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B0>, B1>, B0>;
pub type U363 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B0>, B1>, B1>;
pub type U364 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B1>, B0>, B0>;
pub type U365 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B1>, B0>, B1>;
pub type U366 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B1>, B1>, B0>;
pub type U367 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B1>, B1>, B1>;
pub type U368 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B0>, B0>, B0>;
pub type U369 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B0>, B0>, B1>;
pub type U370 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B0>, B1>, B0>;
pub type U371 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B0>, B1>, B1>;
pub type U372 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B1>, B0>, B0>;
pub type U373 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B1>, B0>, B1>;
pub type U374 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B1>, B1>, B0>;
pub type U375 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B1>, B1>, B1>;
pub type U376 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B0>, B0>, B0>;
pub type U377 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B0>, B0>, B1>;
pub type U378 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B0>, B1>, B0>;
pub type U379 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B0>, B1>, B1>;
pub type U380 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B1>, B0>, B0>;
pub type U381 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B1>, B0>, B1>;
pub type U382 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B1>, B1>, B0>;
pub type U383 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B1>, B1>, B1>;
pub type U384 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B0>;
pub type U385 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B1>;
pub type U386 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B0>, B1>, B0>;
pub type U387 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B0>, B1>, B1>;
pub type U388 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B1>, B0>, B0>;
pub type U389 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B1>, B0>, B1>;
pub type U390 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B1>, B1>, B0>;
pub type U391 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B1>, B1>, B1>;
pub type U392 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B0>, B0>, B0>;
pub type U393 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B0>, B0>, B1>;
pub type U394 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B0>, B1>, B0>;
pub type U395 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B0>, B1>, B1>;
pub type U396 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B1>, B0>, B0>;
pub type U397 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B1>, B0>, B1>;
pub type U398 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B1>, B1>, B0>;
pub type U399 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B1>, B1>, B1>;
pub type U400 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B0>, B0>, B0>;
pub type U401 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B0>, B0>, B1>;
pub type U402 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B0>, B1>, B0>;
pub type U403 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B0>, B1>, B1>;
pub type U404 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B1>, B0>, B0>;
pub type U405 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B1>, B0>, B1>;
pub type U406 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B1>, B1>, B0>;
pub type U407 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B1>, B1>, B1>;
pub type U408 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B0>, B0>, B0>;
pub type U409 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B0>, B0>, B1>;
pub type U410 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B0>, B1>, B0>;
pub type U411 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B0>, B1>, B1>;
pub type U412 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B1>, B0>, B0>;
pub type U413 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B1>, B0>, B1>;
pub type U414 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B1>, B1>, B0>;
pub type U415 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B1>, B1>, B1>;
pub type U416 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B0>, B0>, B0>;
pub type U417 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B0>, B0>, B1>;
pub type U418 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B0>, B1>, B0>;
pub type U419 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B0>, B1>, B1>;
pub type U420 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B1>, B0>, B0>;
pub type U421 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B1>, B0>, B1>;
pub type U422 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B1>, B1>, B0>;
pub type U423 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B1>, B1>, B1>;
pub type U424 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B0>, B0>, B0>;
pub type U425 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B0>, B0>, B1>;
pub type U426 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B0>, B1>, B0>;
pub type U427 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B0>, B1>, B1>;
pub type U428 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B1>, B0>, B0>;
pub type U429 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B1>, B0>, B1>;
pub type U430 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B1>, B1>, B0>;
pub type U431 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B1>, B1>, B1>;
pub type U432 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B0>, B0>, B0>;
pub type U433 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B0>, B0>, B1>;
pub type U434 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B0>, B1>, B0>;
pub type U435 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B0>, B1>, B1>;
pub type U436 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B1>, B0>, B0>;
pub type U437 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B1>, B0>, B1>;
pub type U438 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B1>, B1>, B0>;
pub type U439 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B1>, B1>, B1>;
pub type U440 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B0>, B0>, B0>;
pub type U441 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B0>, B0>, B1>;
pub type U442 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B0>, B1>, B0>;
pub type U443 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B0>, B1>, B1>;
pub type U444 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B1>, B0>, B0>;
pub type U445 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B1>, B0>, B1>;
pub type U446 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B1>, B1>, B0>;
pub type U447 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B1>, B1>, B1>;
pub type U448 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B0>, B0>, B0>;
pub type U449 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B0>, B0>, B1>;
pub type U450 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B0>, B1>, B0>;
pub type U451 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B0>, B1>, B1>;
pub type U452 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B1>, B0>, B0>;
pub type U453 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B1>, B0>, B1>;
pub type U454 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B1>, B1>, B0>;
pub type U455 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B1>, B1>, B1>;
pub type U456 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B0>, B0>, B0>;
pub type U457 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B0>, B0>, B1>;
pub type U458 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B0>, B1>, B0>;
pub type U459 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B0>, B1>, B1>;
pub type U460 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B1>, B0>, B0>;
pub type U461 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B1>, B0>, B1>;
pub type U462 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B1>, B1>, B0>;
pub type U463 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B1>, B1>, B1>;
pub type U464 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B0>, B0>, B0>;
pub type U465 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B0>, B0>, B1>;
pub type U466 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B0>, B1>, B0>;
pub type U467 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B0>, B1>, B1>;
pub type U468 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B1>, B0>, B0>;
pub type U469 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B1>, B0>, B1>;
pub type U470 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B1>, B1>, B0>;
pub type U471 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B1>, B1>, B1>;
pub type U472 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B0>, B0>, B0>;
pub type U473 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B0>, B0>, B1>;
pub type U474 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B0>, B1>, B0>;
pub type U475 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B0>, B1>, B1>;
pub type U476 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B1>, B0>, B0>;
pub type U477 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B1>, B0>, B1>;
pub type U478 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B1>, B1>, B0>;
pub type U479 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B1>, B1>, B1>;
pub type U480 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B0>, B0>, B0>;
pub type U481 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B0>, B0>, B1>;
pub type U482 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B0>, B1>, B0>;
pub type U483 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B0>, B1>, B1>;
pub type U484 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B1>, B0>, B0>;
pub type U485 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B1>, B0>, B1>;
pub type U486 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B1>, B1>, B0>;
pub type U487 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B1>, B1>, B1>;
pub type U488 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B0>, B0>, B0>;
pub type U489 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B0>, B0>, B1>;
pub type U490 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B0>, B1>, B0>;
pub type U491 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B0>, B1>, B1>;
pub type U492 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B1>, B0>, B0>;
pub type U493 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B1>, B0>, B1>;
pub type U494 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B1>, B1>, B0>;
pub type U495 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B1>, B1>, B1>;
pub type U496 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B0>, B0>, B0>;
pub type U497 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B0>, B0>, B1>;
pub type U498 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B0>, B1>, B0>;
pub type U499 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B0>, B1>, B1>;
pub type U500 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B1>, B0>, B0>;
pub type U501 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B1>, B0>, B1>;
pub type U502 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B1>, B1>, B0>;
pub type U503 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B1>, B1>, B1>;
pub type U504 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B0>, B0>, B0>;
pub type U505 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B0>, B0>, B1>;
pub type U506 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B0>, B1>, B0>;
pub type U507 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B0>, B1>, B1>;
pub type U508 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B1>, B0>, B0>;
pub type U509 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B1>, B0>, B1>;
pub type U510 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B1>, B1>, B0>;
pub type U511 = UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B1>, B1>, B1>;
pub type U512 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U513 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B0>, B0>,
    B1,
>;
pub type U521 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B1>, B0>, B0>,
    B1,
>;
pub type U522 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B1>, B0>, B1>,
    B0,
>;
pub type U528 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U536 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B1>, B1>, B0>, B0>,
    B0,
>;
pub type U544 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U560 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U568 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B1>, B1>, B1>, B0>, B0>,
    B0,
>;
pub type U576 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U592 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B0>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U608 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U624 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B1>, B1>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U640 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U656 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B0>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U672 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U688 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B0>, B1>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U704 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U720 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B0>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U736 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U752 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B1>, B1>, B1>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U768 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U784 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B0>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U800 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U816 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B0>, B1>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U832 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U848 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B0>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U864 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U880 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B0>, B1>, B1>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U896 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U912 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B0>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U928 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U944 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B0>, B1>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U960 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U976 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B0>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U992 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B0>, B0>, B0>, B0>,
    B0,
>;
pub type U1008 = UInt<
    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B1>, B1>, B1>, B1>, B1>, B0>, B0>, B0>,
    B0,
>;
pub type U1024 = UInt<
    UInt<
        UInt<
            UInt<UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>, B0>,
            B0,
        >,
        B0,
    >,
    B0,
>;
pub type U2048 = UInt<
    UInt<
        UInt<
            UInt<
                UInt<
                    UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>,
                    B0,
                >,
                B0,
            >,
            B0,
        >,
        B0,
    >,
    B0,
>;
pub type U4096 = UInt<
    UInt<
        UInt<
            UInt<
                UInt<
                    UInt<
                        UInt<UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>, B0>,
                        B0,
                    >,
                    B0,
                >,
                B0,
            >,
            B0,
        >,
        B0,
    >,
    B0,
>;
pub type U8192 = UInt<
    UInt<
        UInt<
            UInt<
                UInt<
                    UInt<
                        UInt<
                            UInt<
                                UInt<UInt<UInt<UInt<UInt<UInt<UTerm, B1>, B0>, B0>, B0>, B0>, B0>,
                                B0,
                            >,
                            B0,
                        >,
                        B0,
                    >,
                    B0,
                >,
                B0,
            >,
            B0,
        >,
        B0,
    >,
    B0,
>;
