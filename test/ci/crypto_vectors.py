"""Bounded reproducible crypto vectors, checked independently with Python hashlib.

Preserved blob data and Argon2 test sources are never edited. Generated Rust
files live only in an external temporary workspace; no registry crates needed.
"""
from pathlib import Path
import hashlib
import re


def generate(live, work):
    b=work; (b/'tests').mkdir(exist_ok=True)
    def arr(x):return '&['+','.join(map(str,x))+']'
    def blobs(d):
     assert len(d) <= 65536, "Fixture exceeds reviewed bound"
     i=0
     def vlq():
      nonlocal i
      b=d[i];i+=1;n=b&127
      while b&128:b=d[i];i+=1;n=((n+1)<<7)|(b&127)
      return n
     dic=[]
     for _ in range(vlq()):
      n=vlq();dic.append(d[i:i+n]);i+=n
     out=[]
     while i<len(d):
      n=vlq()
      if n&1:out.append(dic[n>>1])
      else:n>>=1;out.append(d[i:i+n]);i+=n
     assert i==len(d) and len(out) <= 3072
     return out
    head='''use blake2::{Blake2bVar, Blake2sVar, VariableOutput, cryptographic_core::{Update, FixedOutput, KeyInit, consts::{U1,U16,U32,U64}}};
    fn check_b(input:&[u8], expected:&[u8]) { for chunk in [1,3,64,128,129,2048] { let mut h=Blake2bVar::new(expected.len()).unwrap();for c in input.chunks(chunk) { h.update(c); } let mut out=vec![0;expected.len()];h.finalize_variable(&mut out).unwrap();assert_eq!(out,expected); } }
    fn check_s(input:&[u8], expected:&[u8]) { for chunk in [1,3,64,128,129,2048] { let mut h=Blake2sVar::new(expected.len()).unwrap();for c in input.chunks(chunk) { h.update(c); } let mut out=vec![0;expected.len()];h.finalize_variable(&mut out).unwrap();assert_eq!(out,expected); } }
    '''
    lines=[head,'#[test] fn upstream_known_vectors() {'];counts={}
    for p in (live/'test/foundation/blake2/tests/data').rglob('*.blb'):
     v=blobs(p.read_bytes());mac=p.stem=='mac';group=3 if mac else 2;assert len(v)%group==0;counts[str(p.relative_to(live))]=len(v)//group
     for k in range(0,len(v),group):
      if mac:
       key,data,out=v[k:k+3];h=getattr(hashlib,p.parent.name)(data,key=key).digest();assert h==out
       ty='Blake2bMac512' if p.parent.name=='blake2b' else 'Blake2sMac256'
       lines.append('{ let mut h=blake2::'+ty+'::new_from_slice('+arr(key)+').unwrap(); h.update('+arr(data)+'); assert_eq!(&h.finalize_fixed()[..],'+arr(out)+'); }')
      else:
       data,out=v[k:k+2];assert getattr(hashlib,p.parent.name)(data,digest_size=len(out)).digest()==out
       lines.append('check_'+('b' if p.parent.name=='blake2b' else 's')+'('+arr(data)+','+arr(out)+');')
    lines+=['}','#[test] fn exhaustive_output_lengths_and_boundaries() {']
    for a,maxn in [('b',64),('s',32)]:
     for n in [0,1,63,64,65,127,128,129,255,256,257,1000]:
      data=bytes((i*37+11)%256 for i in range(n));lines.append('{let data:Vec<u8>=(0..'+str(n)+'usize).map(|i| ((i*37+11)%256) as u8).collect();')
      for size in range(1,maxn+1):lines.append('check_'+a+'(&data,'+arr(getattr(hashlib,'blake2'+a)(data,digest_size=size).digest())+');')
      lines.append('}')
    lines+=['}','#[test] fn keyed_personalized_truncated_vectors() {']
    for a,m in [('b',64),('s',32)]:
     for size in [1,16,m]:
      for kl in [0,1,m//2,m]:
       for sl in [0,1,m//4]:
        key=bytes(range(kl));salt=bytes(range(sl));pers=bytes(range(m//4));data=bytes(range(255));out=getattr(hashlib,'blake2'+a)(data,digest_size=size,key=key,salt=salt,person=pers).digest()
        lines.append('{let mut h=blake2::Blake2'+a+'Mac::<U'+str(size)+'>::new_with_salt_and_personal('+arr(key)+','+arr(salt)+','+arr(pers)+').unwrap();h.update('+arr(data)+');assert_eq!(&h.finalize_fixed()[..],'+arr(out)+');}')
    lines+=['}','#[test] fn empty_key_message_boundaries() {']
    for a,m,block in [('b',64,128),('s',32,64)]:
     for size in [1,16,m]:
      for n in [0,1,block-1,block,block+1,2*block+1]:
       data=bytes((i*37+11)%256 for i in range(n))
       for personalized in [False,True]:
        salt=bytes(range(m//4)) if personalized else b''
        pers=bytes(reversed(range(m//4))) if personalized else b''
        out=getattr(hashlib,'blake2'+a)(data,digest_size=size,key=b'',salt=salt,person=pers).digest()
        ty='blake2::Blake2'+a+'Mac::<U'+str(size)+'>'
        lines.append('{let mut h='+ty+'::new_with_salt_and_personal(&[],'+arr(salt)+','+arr(pers)+').unwrap();h.update('+arr(data)+');assert_eq!(&h.finalize_fixed()[..],'+arr(out)+');}')
        if not personalized:
         lines.append('{let mut h='+ty+'::new_from_slice(&[]).unwrap();h.update('+arr(data)+');assert_eq!(&h.finalize_fixed()[..],'+arr(out)+');}')
    lines+=['}']
    generated='\n'.join(lines)
    assert len(generated.encode()) < 1048576
    assert sum(counts.values()) == 518, 'Preserved fixture inventory changed; review vectors explicitly'
    (b/'tests/vectors.rs').write_text(generated)
    a=work;(a/'tests').mkdir(exist_ok=True)
    for name in ['kat','phc_strings']:
     s=(live/f'test/foundation/argon2/tests/{name}.rs').read_text().replace('use hex_literal::hex;','')
     s=re.sub(r'hex!\(\s*"([0-9a-fA-F\s]+)"\s*\)',lambda m:arr(bytes.fromhex(m[1]))[1:],s)
     (a/f'tests/{name}.rs').write_text(s)
    return {'upstream_vectors':sum(counts.values()),'independent_vectors':1152+72+72}
