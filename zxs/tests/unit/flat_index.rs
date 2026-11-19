use super::*;

const FILE: FileId = FileId([1; 16]);
const OP: OperationId = OperationId([2; 16]);
fn m(slot: u32, size: u64) -> Material { Material::declared(FILE, OP, slot, size).unwrap() }
fn segments(index: &Index, off: u64, n: u64) -> Vec<(Leaf,u64,u64)> {
    let mut result=Vec::new();
    index.visit(off,n,|leaf,at,len| { result.push((leaf,at,len)); Ok(()) }).unwrap();
    result
}

#[test]
fn d_writes_across_three_terminal_materials_without_mutating_any_selected_old_version() {
    let data: [&[u8];4] = [b"0123456789abcdefghij",b"B123",b"C123",b"++deltaROW++"];
    let mats: Vec<_> = data.iter().enumerate().map(|(i,b)|m(i as u32,b.len() as u64)).collect();
    let a=Index::full(mats[0]).unwrap();
    let b=a.edit(4,Some((mats[1],0,4)),20).unwrap();
    let c=b.edit(8,Some((mats[2],0,4)),16).unwrap();
    assert_eq!(segments(&c,6,8),[(Leaf::Data(mats[1]),2,2),(Leaf::Data(mats[2]),0,4),(Leaf::Data(mats[0]),12,2)]);
    let d=c.edit(6,Some((mats[3],2,8)),20).unwrap();
    let read=|i:&Index| {
        let mut out=Vec::new();
        i.visit(0,i.len(),|src,at,n| {
            match src {
                Leaf::Zero=>out.resize(out.len()+n as usize,0),
                Leaf::Data(mat)=>out.extend_from_slice(&data[mat.slot() as usize][at as usize..(at+n) as usize]),
            } Ok(())
        }).unwrap();out
    };
    assert_eq!(read(&a),data[0]);
    assert_eq!(read(&b),b"0123B12389abcdefghij");
    assert_eq!(read(&c),b"0123B123C123cdef");
    assert_eq!(read(&d),b"0123B1deltaROWef\0\0\0\0");
    assert_eq!(segments(&d,8,3),[(Leaf::Data(mats[3]),4,3)]);
    drop((a,b,c));
    assert_eq!(read(&d),b"0123B1deltaROWef\0\0\0\0");
}

#[test]
fn normalized_cuts_are_by_identity_and_contiguous_physical_offset_not_similar_bytes() {
    let same_size=m(9,8);let another=m(10,8);
    let mut b=Build::new(FILE).unwrap();
    b.add(Leaf::Data(same_size),2,2).unwrap();
    b.add(Leaf::Data(same_size),4,1).unwrap();
    b.add(Leaf::Data(same_size),1,1).unwrap(); // same material but not contiguous
    b.add(Leaf::Data(another),2,1).unwrap(); // even equal bytes would be a different material
    b.add(Leaf::Zero,0,2).unwrap();b.add(Leaf::Zero,0,4).unwrap();
    let i=b.finish(11).unwrap();
    assert_eq!(segments(&i,0,11),[(Leaf::Data(same_size),2,3),(Leaf::Data(same_size),1,1),
        (Leaf::Data(another),2,1),(Leaf::Zero,0,6)]);
    let shrunk=i.edit(0,None,0).unwrap();assert!(segments(&shrunk,0,0).is_empty());
    assert_eq!(segments(&shrunk.edit(0,None,11).unwrap(),0,11),[(Leaf::Zero,0,11)]);
}

#[test]
fn invalid_or_foreign_extent_never_creates_a_replacement_view() {
    let old=m(1,9);let a=Index::full(old).unwrap();
    let foreign=Material::declared(FileId([3;16]),OP,1,9).unwrap();
    assert!(a.edit(0,Some((foreign,0,9)),9).is_err());
    assert!(a.edit(0,Some((m(1,10),1,9)),9).is_err()); // even when fully replacing old
    assert!(a.edit(7,Some((old,9,1)),9).is_err());
    assert!(a.edit(0,Some((old,u64::MAX,2)),9).is_err());
    assert!(a.edit(0,Some((old,0,0)),9).is_err());
    assert!(a.edit(8,Some((old,0,2)),9).is_err());
    assert!(a.visit(9,1,|_,_,_|panic!("must reject before first byte")).is_err());
    assert!(a.visit(u64::MAX,2,|_,_,_|unreachable!()).is_err());
    assert!(segments(&a,9,0).is_empty());
    let mut b=Build::new(FILE).unwrap();
    assert!(b.inherit(&Index::full(foreign).unwrap(),0,0).is_err());
    assert!(b.add(Leaf::Zero,1,1).is_err());b.add(Leaf::Data(old),1,2).unwrap();
    assert!(b.add(Leaf::Data(m(1,10)),3,1).is_err());
    assert_eq!(segments(&b.finish(2).unwrap(),0,2),[(Leaf::Data(old),1,2)]);
    assert_eq!(segments(&a,0,9),[(Leaf::Data(old),0,9)]);
}

#[test]
fn three_hundred_changes_can_name_three_hundred_distinct_terminal_materials_without_prior_maps() {
    let mut i=Index::full(m(0,0)).unwrap();
    for n in 0..300u32 {
        // Reassignment drops each earlier index. Bound is on actual rows,
        // not an on-disk one-byte role or the number of formal parents.
        i=i.edit(u64::from(n),Some((m(n+1,2),1,1)),u64::from(n)+1).unwrap();
    }
    assert_eq!(i.materials.len(),300);assert_eq!(i.rows.len(),300);
    assert_eq!(segments(&i,298,2),[(Leaf::Data(m(299,2)),1,1),(Leaf::Data(m(300,2)),1,1)]);
    let fully_replaced=i.edit(0,Some((m(900,300),0,300)),300).unwrap();
    assert_eq!(fully_replaced.rows.len(),1);assert_eq!(fully_replaced.materials.len(),1);
    assert_eq!(segments(&i,253,1),[(Leaf::Data(m(254,2)),1,1)]);
}
