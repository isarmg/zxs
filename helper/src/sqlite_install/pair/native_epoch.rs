//! Plain, nonsecret values shared with the independent native Samba adapter.
//! A matching description classifies an already observed row; it never proves
//! original media, that a password was written, or permission to activate SQL.
use super::{Id, bad};
use std::io;

const ID_WIRE: usize = 32;
const GEN_WIRE: usize = 16;
const SEPARATORS: [usize; 5] = [4, 37, 70, 103, 136];
pub(super) const NATIVE_WIDTH: usize = 154;
const LIMIT: u64 = i64::MAX as u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Epoch {
    pool: Id,
    instance: Id,
    account: Id,
    op: Id,
    generation: u64,
}

fn glyph(value: u8) -> u8 { b"0123456789abcdef"[usize::from(value)] }
fn nibble(value: u8) -> io::Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(bad("native account epoch is not canonical lowercase hex")),
    }
}

impl Epoch {
    pub(super) fn new(pool: Id, instance: Id, account: Id, op: Id, generation: u64) -> io::Result<Self> {
        if [pool, instance, account, op].contains(&[0; 16]) || !(1..=LIMIT).contains(&generation) {
            return Err(bad("native account epoch requires nonzero original IDs and an in-range generation"));
        }
        Ok(Self { pool, instance, account, op, generation })
    }

    /// The authoritative native input is the explicitly bounded B field, before
    /// it passes through a NUL-terminated setter or getter. strlen is not proof.
    pub(super) fn parse_native(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() != NATIVE_WIDTH || &bytes[..4] != b"ZXS1" || bytes[NATIVE_WIDTH - 1] != 0
            || SEPARATORS.iter().any(|&n| bytes[n] != b':') {
            return Err(bad("wrong native account epoch field size, separators or terminator"));
        }
        let id = |at| -> io::Result<Id> {
            let mut out = [0; 16];
            for (dst, pair) in out.iter_mut().zip(bytes[at..at + ID_WIRE].chunks_exact(2)) {
                *dst = (nibble(pair[0])? << 4) | nibble(pair[1])?;
            }
            Ok(out)
        };
        let start = SEPARATORS[4] + 1;
        let mut generation = 0;
        for &b in &bytes[start..start + GEN_WIRE] {
            generation = (generation << 4) | u64::from(nibble(b)?);
        }
        Self::new(id(5)?, id(38)?, id(71)?, id(104)?, generation)
    }

    pub(super) fn native(self) -> [u8; NATIVE_WIDTH] {
        let mut bytes = [0; NATIVE_WIDTH];
        bytes[..4].copy_from_slice(b"ZXS1");
        for at in SEPARATORS { bytes[at] = b':'; }
        for (id, separator) in [self.pool, self.instance, self.account, self.op].into_iter().zip(SEPARATORS) {
            let start = separator + 1;
            for (b, out) in id.into_iter().zip(bytes[start..start + ID_WIRE].chunks_exact_mut(2)) {
                out[0] = glyph(b >> 4);
                out[1] = glyph(b & 15);
            }
        }
        for (i, b) in bytes[137..153].iter_mut().enumerate() {
            *b = glyph(((self.generation >> ((15 - i) * 4)) & 15) as u8);
        }
        bytes
    }
}

pub(super) struct ExpectedMarkers { previous: Option<Epoch>, target: Epoch }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Match { Target, Previous }

impl ExpectedMarkers {
    /// The caller will independently obtain these expectations from the exact
    /// pending SQL request. Missing files, empty directories and unknown rows
    /// are deliberately not arguments of this comparison.
    pub(super) fn new(previous: Option<Epoch>, target: Epoch) -> io::Result<Self> {
        let ordered = match previous {
            None => target.generation == 1,
            Some(p) => p.pool == target.pool && p.instance == target.instance && p.account == target.account
                && p.op != target.op && p.generation.checked_add(1) == Some(target.generation),
        };
        if !ordered { return Err(bad("native target is not the exact first or successive account epoch")); }
        Ok(Self { previous, target })
    }

    pub(super) fn fields(&self) -> (Option<[u8; NATIVE_WIDTH]>, [u8; NATIVE_WIDTH]) {
        (self.previous.map(Epoch::native), self.target.native())
    }
    pub(super) fn classify(&self, field: &[u8]) -> io::Result<Match> {
        let observed = Epoch::parse_native(field)?;
        if observed == self.target { return Ok(Match::Target); }
        if Some(observed) == self.previous { return Ok(Match::Previous); }
        Err(bad("the account marker matches neither this target nor its exact predecessor"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const VECTORS: &str = include_str!("../../../tests/sqlite/smb-native-epoch-v1.txt");
    fn sample(label: &str) -> Vec<u8> {
        let mut bytes = VECTORS.lines().filter_map(|row| row.split_once(" = "))
            .find(|(name, _)| *name == label).expect("fixed sample").1.as_bytes().to_vec();
        bytes.push(0);
        bytes
    }
    fn range(base: u8) -> Id { std::array::from_fn(|index| base + index as u8) }

    #[test]
    fn matches_the_shared_public_samples_for_birth_and_reset() {
        for (label, op, generation) in [("gen1", 0x31, 1), ("gen2", 0x41, 2)] {
            let e = Epoch::new(range(1), range(0x11), range(0x21), range(op), generation).unwrap();
            let expected = sample(label);
            assert_eq!(e.native().as_slice(), expected.as_slice());
            assert_eq!(Epoch::parse_native(&expected).unwrap(), e);
        }
    }

    #[test]
    fn native_fixed_width_cannot_hide_an_embedded_nul_suffix_or_unknown_format() {
        let bytes = sample("gen1");
        assert!(Epoch::parse_native(&bytes[..153]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(Epoch::parse_native(&longer).is_err());
        for (at, b) in [(0, b'z'), (37, b'/'), (11, 0), (34, b'A'), (68, 0xff), (153, b'!')] {
            let mut invalid = bytes.clone();
            invalid[at] = b;
            assert!(Epoch::parse_native(&invalid).is_err());
        }
        for replacement in [b"0000000000000000", b"8000000000000000"] {
            let mut invalid = bytes.clone();
            invalid[137..153].copy_from_slice(replacement);
            assert!(Epoch::parse_native(&invalid).is_err());
        }
        let mut valid = bytes.clone();
        valid[137..153].copy_from_slice(b"7fffffffffffffff");
        assert_eq!(Epoch::parse_native(&valid).unwrap().native().as_slice(), valid.as_slice());
        for start in [5, 38, 71, 104] {
            let mut invalid = bytes.clone();
            invalid[start..start + 32].fill(b'0');
            assert!(Epoch::parse_native(&invalid).is_err());
        }
    }

    #[test]
    fn only_the_exact_target_or_real_predecessor_matches_no_guess_for_absence() {
        let (p, t) = (sample("gen1"), sample("gen2"));
        let (a, b) = (Epoch::parse_native(&p).unwrap(), Epoch::parse_native(&t).unwrap());
        let first = ExpectedMarkers::new(None, a).unwrap();
        assert_eq!(first.classify(&p).unwrap(), Match::Target);
        assert!(first.classify(&[]).is_err());
        assert!(first.classify(&t).is_err());
        let reset = ExpectedMarkers::new(Some(a), b).unwrap();
        assert_eq!(reset.classify(&p).unwrap(), Match::Previous);
        assert_eq!(reset.classify(&t).unwrap(), Match::Target);
        assert!(reset.classify(&[]).is_err());
        for other in [Epoch { pool: [7; 16], ..b }, Epoch { instance: [7; 16], ..b },
            Epoch { account: [7; 16], ..b }, Epoch { generation: 3, ..b }, Epoch { op: a.op, ..b }] {
            assert!(ExpectedMarkers::new(Some(a), other).is_err());
            assert!(reset.classify(&other.native()).is_err());
        }
        assert!(ExpectedMarkers::new(None, b).is_err());
    }
}
