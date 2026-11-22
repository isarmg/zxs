use super::*;
fn map(parent:u64,logical:u64,changed:u64,runs:&[(u8,u64)])->Map{
    let mut b=(runs.len()as u32).to_be_bytes().to_vec();
    for &(tag,n) in runs{b.push(tag);b.extend_from_slice(&n.to_be_bytes());}
    Map::decode(&b,runs.len()as u32,parent,logical,changed).unwrap()
}
#[test]
fn second_generation_inherits_changes_only_from_its_immediate_parent_and_never_untruncates_full(){
    let first=map(10,5,2,&[(1,2),(2,2),(1,1)]);
    let second=map(5,9,1,&[(1,3),(2,1),(1,1),(3,4)]);
    let mut parts=Vec::new();Map::resolve(10,[&first,&second],2,7,|s,o,n|{parts.push((s,o,n));Ok(())}).unwrap();
    assert_eq!(parts,[(Source::Pack(0),0,1),(Source::Pack(1),0,1),(Source::Full,4,1),(Source::Zero,0,4)]);
    let mut bytes=Vec::new();Map::resolve(10,[&first,&second],0,9,|s,o,n|{
        if s==Source::Zero{bytes.resize(bytes.len()+n as usize,0);}else{
            let src:&[u8]=match s{Source::Pack(0)=>b"XY",Source::Pack(1)=>b"Z",Source::Full=>b"abcdefghij",_=>unreachable!()};
            bytes.extend_from_slice(&src[o as usize..(o+n)as usize]);}Ok(())
    }).unwrap();assert_eq!(bytes,b"abXZe\0\0\0\0");
    // Short reads map dense offsets in each independent original pack,
    // not into the larger FULL or the other generation's dense bytes.
    let mut middle=Vec::new();Map::resolve(10,[&first],3,1,|s,o,n|{middle.push((s,o,n));Ok(())}).unwrap();
    assert_eq!(middle,[(Source::Pack(0),1,1)]);
    assert!(Map::resolve(9,[&first,&second],0,0,|_,_,_|panic!("bad bound emitted bytes")).is_err());
    assert!(Map::resolve(10,[&second],0,9,|_,_,_|panic!("skipped parent emitted bytes")).is_err());
    assert!(Map::resolve(10,[&first,&second],9,1,|_,_,_|panic!("past end emitted bytes")).is_err());
    let empty=map(9,0,0,&[]);assert!(Map::resolve(10,[&first,&empty],0,0,|_,_,_|unreachable!()).is_err());
}
#[test]
fn deep_overlap_uses_the_right_original_dense_offset_and_only_the_selected_ancestry(){
    // F0=abcdefghij; I1=abXYe, I2=abXZe + four zeroes;
    // I3 masks offsets 2..5; I4 masks 3..4; I5 truncates to 7 then I6 grows to 12.
    let maps=[map(10,5,2,&[(1,2),(2,2),(1,1)]),map(5,9,1,&[(1,3),(2,1),(1,1),(3,4)]),
        map(9,9,3,&[(1,2),(2,3),(1,4)]),map(9,9,1,&[(1,3),(2,1),(1,5)]),
        map(9,7,0,&[(1,7)]),map(7,12,0,&[(1,7),(3,5)])];
    let ranges=|n:usize,off,len|{let mut v=Vec::new();Map::resolve(10,&maps[..n],off,len,|s,o,l|{v.push((s,o,l));Ok(())}).unwrap();v};
    assert_eq!(ranges(4,2,4),[(Source::Pack(2),0,1),(Source::Pack(3),0,1),(Source::Pack(2),2,1),(Source::Zero,0,1)]);
    assert_eq!(ranges(6,0,12),[(Source::Full,0,2),(Source::Pack(2),0,1),(Source::Pack(3),0,1),(Source::Pack(2),2,1),(Source::Zero,0,7)]);
    assert_eq!(ranges(2,2,3),[(Source::Pack(0),0,1),(Source::Pack(1),0,1),(Source::Full,4,1)]);
    assert_eq!(ranges(0,8,2),[(Source::Full,8,2)]);
    assert!(Map::resolve(10,&maps[..],u64::MAX,2,|_,_,_|panic!("bad range emitted bytes")).is_err());
}
#[test]
fn ordinals_are_not_byte_tags_and_a_fully_overwritten_layer_cannot_hide_a_broken_edge(){
    let layers:Vec<_>=(0..300).map(|_|map(3,3,1,&[(1,1),(2,1),(1,1)])).collect();
    let mut got=Vec::new();Map::resolve(3,&layers,0,3,|s,o,n|{got.push((s,o,n));Ok(())}).unwrap();
    assert_eq!(got,[(Source::Full,0,1),(Source::Pack(299),0,1),(Source::Full,2,1)]);
    let corrupt=map(4,3,3,&[(2,3)]);let cover=map(3,3,3,&[(2,3)]);
    assert!(Map::resolve(3,layers.iter().chain([&corrupt,&cover]),0,3,|_,_,_|panic!("broken ancestor emitted bytes")).is_err());
}
