use super::*;
pub(crate) fn small() -> Vec<u8> {
    let mut b = vec![0; 224];
    b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    b[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
    b[18..20].copy_from_slice(&62u16.to_le_bytes());
    b[20..24].copy_from_slice(&1u32.to_le_bytes());
    b[24..32].copy_from_slice(&0x0040_0080_u64.to_le_bytes());
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[52..54].copy_from_slice(&64u16.to_le_bytes());
    b[54..56].copy_from_slice(&56u16.to_le_bytes());
    b[56..58].copy_from_slice(&2u16.to_le_bytes());
    b[64..68].copy_from_slice(&PT_LOAD.to_le_bytes());
    b[68..72].copy_from_slice(&PF_X.to_le_bytes());
    b[80..88].copy_from_slice(&0x0040_0000_u64.to_le_bytes());
    b[96..104].copy_from_slice(&224u64.to_le_bytes());
    b[104..112].copy_from_slice(&224u64.to_le_bytes());
    b
}
fn check(b: &[u8]) -> io::Result<()> {
    standalone(b.len() as u64, |o, out| {
        let o = usize::try_from(o).map_err(|_| denied())?;
        out.copy_from_slice(b.get(o..o + out.len()).ok_or_else(denied)?);
        Ok(())
    })
}
#[test]
fn standalone_image_rejects_interpreter_loader_and_ambiguous_programs() {
    let b = small();
    check(&b).unwrap();
    let mut bad = b.clone();
    bad[120..124].copy_from_slice(&PT_INTERP.to_le_bytes());
    assert!(check(&bad).is_err());
    let mut bad = b.clone();
    bad[68..72].fill(0);
    assert!(check(&bad).is_err());
    let mut bad = b.clone();
    bad[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
    assert!(check(&bad).is_err());
    let mut pie = b;
    pie[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
    pie[120..124].copy_from_slice(&PT_DYNAMIC.to_le_bytes());
    pie[128..136].copy_from_slice(&176u64.to_le_bytes());
    pie[136..144].copy_from_slice(&0x0040_00b0_u64.to_le_bytes());
    pie[152..160].copy_from_slice(&32u64.to_le_bytes());
    pie[160..168].copy_from_slice(&32u64.to_le_bytes());
    pie[176..184].copy_from_slice(&0x6fff_fffbu64.to_le_bytes());
    pie[184..192].copy_from_slice(&0x0800_0000u64.to_le_bytes());
    check(&pie).unwrap();
    pie[192..200].copy_from_slice(&1u64.to_le_bytes());
    assert!(check(&pie).is_err());
}
