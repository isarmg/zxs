use super::*;
const FIXTURE:&str=include_str!("../vectors/maintenance-genesis-v1.txt");
fn raw(key:&str)->Vec<u8>{
    let hex=FIXTURE.lines().find_map(|s|s.strip_prefix(key).and_then(|s|s.strip_prefix('='))).unwrap();
    assert_eq!(hex.len()%2,0);(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect()
}
struct Sample{image:Vec<u8>,lower:Vec<u8>,labels:Vec<Vec<u8>>}
impl Sample{
    fn load()->Self{Self{image:raw("genesis"),lower:raw("lower_bound"),
        labels:["label_system","label_recycle","label_file_0","label_file_1"].map(raw).to_vec()}}
    fn read(&self,uid:u32)->io::Result<PoolInfo>{
        read_empty(&self.image,uid,&self.lower,&self.labels.iter().map(Vec::as_slice).collect::<Vec<_>>())
    }
    // These are self-consistent changed byte samples; rejection must not be
    // an incidental mismatch with the old checksum, SYSTEM label or floor.
    fn recalculate(&mut self){
        let end=self.image.len()-32;let h=crate::hash::digest(&self.image[..end]).unwrap();
        self.image[end..].copy_from_slice(&h);self.lower[16..].copy_from_slice(&h);
        let off=self.labels[0].len()-32;self.labels[0][off..].copy_from_slice(&h);
    }
}
#[test]
fn the_fixed_four_root_image_and_its_native_byte_paths_are_not_a_legacy_business_genesis(){
    let t=Sample::load();assert_eq!(t.image.len(),423);assert_eq!(&t.image[391..],raw("sha256_prefix").as_slice());
    let view=t.read(1001).unwrap();assert_eq!(view.id.0,[0x11;16]);
    assert_eq!(view.initialized_by.0,[0x22;16]);assert_eq!(view.volumes.len(),4);
    assert_eq!(view.volumes[3].volume.role,Role::File);assert_eq!(view.volumes[3].volume.path.as_os_str().as_bytes(),b"/m/\xffvol");
    assert!(Record::prefix(&t.image).is_err());assert!(t.read(0).is_err());
    let mut wrong=t;wrong.image.splice(..GENESIS.len(),MAGIC.iter().copied());wrong.recalculate();
    assert!(wrong.read(1001).is_err());
}
#[test]
fn a_rehashed_alias_or_nested_root_cannot_be_an_empty_registration(){
    for swap in 0..5{
        let mut t=Sample::load();
        // Represented rows begin at 105,176,247,319 (fixed literal vector);
        // an Entity begins 17 bytes in, its 28B Birth content 34 bytes in.
        match swap{
            0=>t.image.copy_within(58..74,105+17), // SYSTEM claims register's Entity
            1=>t.image.copy_within(75..103,105+34),// SYSTEM claims register's Birth
            2=>t.image.copy_within(247+34..247+62,319+34),// two F alias by handle
            3=>{ // /m/sys is an actual component ancestor of /m/sys/x.
                t.image.splice(319+63..391,[0,8,b'/',b'm',b'/',b's',b'y',b's',b'/',b'x']);
            },
            _=>t.image[391-7..391].copy_from_slice(b"/m/data"), // duplicate original path
        }
        t.recalculate();assert!(t.read(1001).is_err());
    }
    let mut ok=Sample::load();ok.image[391-7..391].copy_from_slice(b"/m/sysx");ok.recalculate();
    assert!(ok.read(1001).is_ok()); // lexical prefix only: distinct sibling
}
#[test]
fn one_checksum_does_not_explain_another_role_frontier_or_trailing_frame(){
    let mut t=Sample::load();t.labels.swap(1,2);assert!(t.read(1001).is_err());
    let mut t=Sample::load();t.labels[0].pop();assert!(t.read(1001).is_err());
    let mut t=Sample::load();t.lower[7]=1;assert!(t.read(1001).is_err());
    let mut t=Sample::load();t.image[54]^=1;assert!(t.read(1001).is_err());
    let mut t=Sample::load();t.image.push(0);assert!(t.read(1001).is_err());
}
