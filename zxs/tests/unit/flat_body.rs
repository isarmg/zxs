use super::*;
use crate::{FileId,NodeId,PoolId,VersionId,hash,ordinary::{content::BLOCK,flat::{Build,stored::{encode,VersionKey},write_candidate_checksums}},test_support::PrivateTemp};
use std::{fs,io::Write,os::unix::fs::FileExt,path::{Path,PathBuf}};

const P:PoolId=PoolId([1;16]);const F:FileId=FileId([2;16]);const OP:OperationId=OperationId([5;16]);
const TWO:ReadBudget=ReadBudget{file_descriptors:4,buffer_bytes:8*1024*1024};
const ONE:ReadBudget=ReadBudget{file_descriptors:2,buffer_bytes:512};
const NONE:ReadBudget=ReadBudget{file_descriptors:0,buffer_bytes:0};
fn material(slot:u32,len:u64)->Material{Material::declared(F,OP,slot,len).unwrap()}
fn create(path:&Path,data:&[u8])->File{
    let mut f=fs::OpenOptions::new().create_new(true).read(true).write(true).open(path).unwrap();
    f.write_all(data).unwrap();f.sync_all().unwrap();f
}
struct Parts{r:MaterialRef,body:PathBuf,table:PathBuf}
impl Parts{
    fn stage(tmp:&PrivateTemp,name:&str,m:Material,bytes:&[u8])->Self{
        let body=tmp.0.join(format!("{name}.body"));let table=tmp.0.join(format!("{name}.checks"));
        let a=create(&body,bytes);let b=create(&table,&[]);
        let r=write_candidate_checksums(P,m,&a,&b).unwrap();
        Self{r,body,table}
    }
    fn pair(&self)->io::Result<(File,File)>{Ok((File::open(&self.body)?,File::open(&self.table)?))}
}
fn view(tmp:&PrivateTemp,i:&Index,name:&str,lookup:impl FnMut(Material)->io::Result<MaterialRef>)->io::Result<Contents>{
    let key=VersionKey{pool:P,file:F,node:NodeId([3;16]),version:VersionId([4;16]),representation:0};
    let(d,bytes)=encode(key,i)?;let path=tmp.0.join(name);
    drop(create(&path,&bytes));File::open(&tmp.0)?.sync_all()?;Contents::open(d,File::open(path)?,lookup)
}
fn hex(b:&[u8])->String{b.iter().map(|x|format!("{x:02x}")).collect()}

#[test]
fn one_known_payload_has_an_independent_exact_header_directory_and_original_reference(){
    let t=PrivateTemp::under(&std::env::temp_dir());let m=material(260,3);
    let x=Parts::stage(&t,"sample",m,b"abc");
    assert_eq!(hex(&x.r.directory_sha()),"87159d90039d4ba4e47bca264b1e015211117a06313401247bfb9d39d0e03ffb");
    let table=fs::read(&x.table).unwrap();assert_eq!(table.len(),159);
    assert_eq!(hex(&table[95..127]),"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(hex(&table[127..]),"4f8b42c22dd3729b519ba6f68d2da7cc5b2d606d05daed5ad5128cc03e6c6358");
    let(a,b)=x.pair().unwrap();let ro=Reader::open(x.r,a,b).unwrap();let mut out=Vec::new();let mut buffers=Buffers::new(32,3).unwrap();
    ro.range(2,1,&mut buffers,&mut out).unwrap();assert_eq!(out,b"c");assert!(ro.range(3,1,&mut buffers,&mut out).is_err());
    let empty=Parts::stage(&t,"empty",material(99,0),b"");assert_eq!(fs::metadata(&empty.table).unwrap().len(),95);
    let(a,b)=empty.pair().unwrap();Reader::open(empty.r,a,b).unwrap().range(0,0,&mut Buffers::new(0,0).unwrap(),&mut Vec::new()).unwrap();
}

#[test]
fn only_selected_real_materials_open_and_same_bytes_cannot_reuse_another_original_reference(){
    let t=PrivateTemp::under(&std::env::temp_dir());
    let m=material(1,8);let n=material(260,8);let a=Parts::stage(&t,"first",m,b"abcdefgh");let b=Parts::stage(&t,"next",n,b"abcdefgh");
    assert_ne!(a.r.directory_sha(),b.r.directory_sha()); // same body, different issuer role
    let mut plan=Build::new(F).unwrap();
    for (leaf,off,len) in [(Leaf::Data(m),2,2),(Leaf::Zero,0,2),(Leaf::Data(n),5,2),(Leaf::Data(m),0,2)]{plan.add(leaf,off,len).unwrap();}
    let i=plan.finish(8).unwrap();
    assert!(view(&t,&i,"wrong-original",|_|Ok(b.r)).is_err());
    let get=|id:Material|->io::Result<MaterialRef>{if id==m{Ok(a.r)}else if id==n{Ok(b.r)}else{Err(invalid("not issued"))}};
    let held=view(&t,&i,"selected",get).unwrap();
    let mut out=Vec::new();held.range_to(2,2,NONE,|_|panic!("zero range opened a payload"),&mut out).unwrap();
    assert_eq!(out,b"\0\0");out.clear();
    let mut visits=Vec::new();
    held.range_to(1,7,TWO,|wanted|{visits.push(wanted.material);if wanted==a.r{a.pair()}else{b.pair()}},&mut out).unwrap();
    assert_eq!(visits,[m,n]);assert_eq!(out,b"d\0\0fgab");out.clear();
    held.range_to(6,2,ONE,|v|{assert_eq!(v,a.r);a.pair()},&mut out).unwrap();assert_eq!(out,b"ab");
    let wrong=view(&t,&i,"wrong-file",get).unwrap();out.clear();
    assert!(wrong.range_to(0,1,ONE,|_|b.pair(),&mut out).is_err());assert!(out.is_empty());
}

#[test]
fn damage_is_local_before_delivery_and_a_late_failure_never_retransmits_an_accepted_prefix(){
    let t=PrivateTemp::under(&std::env::temp_dir());let m=material(1,BLOCK+3);
    let bytes=vec![b'A';(BLOCK+3) as usize];let p=Parts::stage(&t,"two-blocks",m,&bytes);drop(bytes);
    let raw=fs::OpenOptions::new().write(true).open(&p.body).unwrap();raw.write_all_at(b"X",BLOCK+2).unwrap();raw.sync_all().unwrap();
    let h=view(&t,&Index::full(m).unwrap(),"range",|_|Ok(p.r)).unwrap();let mut got=Vec::new();
    h.range_to(0,1,TWO,|_|p.pair(),&mut got).unwrap();assert_eq!(got,b"A"); // bad unselected block wasn't read
    got.clear();assert!(h.range_to(BLOCK-1,2,TWO,|_|p.pair(),&mut got).is_err());assert_eq!(got,b"A");
    got.clear();assert!(h.range_to(BLOCK,1,TWO,|_|p.pair(),&mut got).is_err());assert!(got.is_empty());
    // Damage outside the requested byte but inside its verified block rejects
    // BEFORE that byte. It is not a test of every byte in some earlier version.
}

#[test]
fn short_last_page_and_a_nonselected_corrupt_checksum_page_are_distinguishable(){
    // A sparse zero payload for this source fixture; no production zero/data
    // decisions are inferred from holes. Only one 4MiB block is actually read.
    let t=PrivateTemp::under(&std::env::temp_dir());let size=1024*BLOCK+1;let m=material(12,size);
    let raw=t.0.join("large");let f=create(&raw,&[]);f.set_len(size).unwrap();f.sync_all().unwrap();
    let mut hdr=b"ZXS-PORTABLE-1/BLOCK-TABLE\0".to_vec();
    for identity in [P.0,F.0,OP.0]{hdr.extend_from_slice(&identity);}
    hdr.extend_from_slice(&12u32.to_be_bytes());hdr.extend_from_slice(&size.to_be_bytes());
    hdr.extend_from_slice(&(BLOCK as u32).to_be_bytes());hdr.extend_from_slice(&1024u32.to_be_bytes());
    let mut first=hash::digest(&vec![0;BLOCK as usize]).unwrap().repeat(1024);
    let tail=hash::digest(&[0]).unwrap();
    let mut directory=hash::digest(&first).unwrap().to_vec();directory.extend_from_slice(&hash::digest(&tail).unwrap());
    let mut meta=hdr.clone();meta.extend_from_slice(&directory);
    let expected=MaterialRef::declared(P,m,hash::digest(&meta).unwrap()).unwrap();
    first[0]^=1;let mut actual=hdr;actual.extend_from_slice(&first);actual.extend_from_slice(&tail);actual.extend_from_slice(&directory);
    let index_path=t.0.join("large-checks");drop(create(&index_path,&actual));
    let reader=Reader::open(expected,File::open(&raw).unwrap(),File::open(index_path).unwrap()).unwrap();
    let mut out=Vec::new();reader.range(size-1,1,&mut Buffers::new(32,1).unwrap(),&mut out).unwrap();assert_eq!(out,[0]);
    out.clear();assert!(reader.range(0,1,&mut Buffers::new(1024*32,BLOCK as usize).unwrap(),&mut out).is_err());assert!(out.is_empty());
}

#[test]
fn index_writer_does_not_overwrite_its_own_payload_or_adopt_an_existing_output(){
    let t=PrivateTemp::under(&std::env::temp_dir());
    for(bytes,name) in [(b"abc".as_slice(),"filled"),(b"".as_slice(),"empty")]{
        let path=t.0.join(name);let raw=create(&path,bytes);
        let alias=t.0.join(format!("{name}-link"));fs::hard_link(&path,&alias).unwrap();
        let second=fs::OpenOptions::new().read(true).write(true).open(&alias).unwrap();
        let m=material(3,bytes.len() as u64);
        assert!(write_candidate_checksums(P,m,&raw,&raw).is_err());
        assert!(write_candidate_checksums(P,m,&raw,&second).is_err());
        assert_eq!(fs::read(&path).unwrap(),bytes);assert_eq!(fs::read(alias).unwrap(),bytes);
    }
    let raw=create(&t.0.join("source"),b"x");let outside=t.0.join("occupied");
    let occupied=create(&outside,b"not a candidate");
    assert!(write_candidate_checksums(P,material(4,1),&raw,&occupied).is_err());
    assert_eq!(fs::read(outside).unwrap(),b"not a candidate");
}

#[test]
fn disjoint_requests_get_fresh_pairs_within_one_pair_budget_and_release_them_after_flush(){
    let t=PrivateTemp::under(&std::env::temp_dir());let mut plan=Build::new(F).unwrap();plan.add(Leaf::Zero,0,1).unwrap();
    let mut parts=Vec::new();
    for i in 0..8u32{
        let m=material(i,1);parts.push(Parts::stage(&t,&format!("s{i}"),m,&[i as u8]));
        plan.add(Leaf::Data(m),0,1).unwrap();
    }
    let h=view(&t,&plan.finish(9).unwrap(),"many",|m|Ok(parts[m.slot() as usize].r)).unwrap();
    let pair=|r:MaterialRef|->io::Result<(File,File)>{
        let part=&parts[r.material.slot() as usize];assert_eq!(r,part.r);let(raw,checks)=part.pair()?;
        raw.try_lock_shared().map_err(io::Error::from)?;checks.try_lock_shared().map_err(io::Error::from)?;Ok((raw,checks))
    };
    for _ in 0..2 {
        for(i,p) in parts.iter().enumerate(){
            let probe=File::open(&p.body).unwrap();let mut bytes=Vec::new();
            let mut sink=ProbeSink{out:&mut bytes,flush_hook:||{
                assert_eq!(io::Error::from(probe.try_lock().unwrap_err()).kind(),io::ErrorKind::WouldBlock);Ok(())
            }};
            h.range_to(1+i as u64,1,ONE,pair,&mut sink).unwrap();assert_eq!(bytes,[i as u8]);
            probe.try_lock().unwrap();probe.unlock().unwrap();
        }
    }
    let mut zero=Vec::new();h.range_to(0,1,NONE,|_|panic!("opened for zero"),&mut zero).unwrap();assert_eq!(zero,[0]);
    for (n,budget) in [(3,ONE),(2,ReadBudget{file_descriptors:2,buffer_bytes:64})]{
        let mut bytes=Vec::new();assert!(h.range_to(0,n,budget,|_|panic!("must reject before opening"),&mut bytes).is_err());
        assert!(bytes.is_empty()); // even the leading zero was not delivered
    }
    let mut bytes=Vec::new();let mut opened=0;
    assert!(h.range_to(0,3,TWO,|r|{
        opened+=1;if opened==2{return Err(io::Error::from(io::ErrorKind::PermissionDenied));}pair(r)
    },&mut bytes).is_err());assert_eq!(opened,2);assert!(bytes.is_empty());
    File::open(&parts[0].body).unwrap().try_lock().unwrap(); // earlier pair released on later open error
    let mut reject=ProbeSink{out:&mut bytes,flush_hook:||Err(io::Error::from(io::ErrorKind::BrokenPipe))};
    assert!(h.range_to(8,1,ONE,pair,&mut reject).is_err());assert_eq!(bytes,[7]);
    File::open(&parts[7].body).unwrap().try_lock().unwrap(); // also released on failed flush
}

struct ProbeSink<'a,F>{out:&'a mut Vec<u8>,flush_hook:F}
impl<F:FnMut()->io::Result<()>> Write for ProbeSink<'_,F>{
    fn write(&mut self,b:&[u8])->io::Result<usize>{self.out.extend_from_slice(b);Ok(b.len())}
    fn flush(&mut self)->io::Result<()>{(self.flush_hook)()}
}

#[test]
fn change_to_an_earlier_selected_material_during_flush_is_not_success_or_a_second_delivery(){
    let t=PrivateTemp::under(&std::env::temp_dir());let a=Parts::stage(&t,"a",material(1,2),b"ab");
    let b=Parts::stage(&t,"b",material(2,2),b"cd");let mut plan=Build::new(F).unwrap();
    for p in [&a,&b]{plan.add(Leaf::Data(p.r.material),0,2).unwrap();}
    let h=view(&t,&plan.finish(4).unwrap(),"all",|m|Ok(if m==a.r.material{a.r}else{b.r})).unwrap();
    let mut bytes=Vec::new();let mut flushes=0;
    let mut sink=ProbeSink{out:&mut bytes,flush_hook:||{
        flushes+=1;let altered=fs::OpenOptions::new().write(true).open(&a.body)?;
        altered.write_all_at(b"changed",0)?;altered.sync_all()
    }};
    assert!(h.range_to(0,4,TWO,|r|if r==a.r{a.pair()}else{b.pair()},&mut sink).is_err());
    assert_eq!(flushes,1);assert_eq!(bytes,b"abcd");
}
