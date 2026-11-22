//! SHA-256 as specified by FIPS 180-4. This implementation is used to detect
//! corruption. An unkeyed digest does not authenticate an untrusted principal.
const K: [u32;64] = [
    0x428a_2f98,0x7137_4491,0xb5c0_fbcf,0xe9b5_dba5,0x3956_c25b,0x59f1_11f1,0x923f_82a4,0xab1c_5ed5,
    0xd807_aa98,0x1283_5b01,0x2431_85be,0x550c_7dc3,0x72be_5d74,0x80de_b1fe,0x9bdc_06a7,0xc19b_f174,
    0xe49b_69c1,0xefbe_4786,0x0fc1_9dc6,0x240c_a1cc,0x2de9_2c6f,0x4a74_84aa,0x5cb0_a9dc,0x76f9_88da,
    0x983e_5152,0xa831_c66d,0xb003_27c8,0xbf59_7fc7,0xc6e0_0bf3,0xd5a7_9147,0x06ca_6351,0x1429_2967,
    0x27b7_0a85,0x2e1b_2138,0x4d2c_6dfc,0x5338_0d13,0x650a_7354,0x766a_0abb,0x81c2_c92e,0x9272_2c85,
    0xa2bf_e8a1,0xa81a_664b,0xc24b_8b70,0xc76c_51a3,0xd192_e819,0xd699_0624,0xf40e_3585,0x106a_a070,
    0x19a4_c116,0x1e37_6c08,0x2748_774c,0x34b0_bcb5,0x391c_0cb3,0x4ed8_aa4a,0x5b9c_ca4f,0x682e_6ff3,
    0x748f_82ee,0x78a5_636f,0x84c8_7814,0x8cc7_0208,0x90be_fffa,0xa450_6ceb,0xbef9_a3f7,0xc671_78f2,
];

pub(crate) struct Sha256 {
    h: [u32;8], block: [u8;64], used: usize, bytes: u64,
}
impl Sha256 {
    pub(crate) fn new() -> Self {
        Self { h:[0x6a09_e667,0xbb67_ae85,0x3c6e_f372,0xa54f_f53a,0x510e_527f,0x9b05_688c,0x1f83_d9ab,0x5be0_cd19],
            block:[0;64], used:0, bytes:0 }
    }
    fn compress(&mut self, block: &[u8;64]) {
        let mut schedule = [0_u32;64];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            schedule[i] = u32::from_be_bytes(*word);
        }
        for t in 16..64 {
            let x=schedule[t-15]; let y=schedule[t-2];
            let s0=x.rotate_right(7)^x.rotate_right(18)^(x>>3);
            let s1=y.rotate_right(17)^y.rotate_right(19)^(y>>10);
            schedule[t]=schedule[t-16].wrapping_add(s0).wrapping_add(schedule[t-7]).wrapping_add(s1);
        }
        // Working registers A through H from FIPS 180-4.
        let [mut reg_a,mut reg_b,mut reg_c,mut reg_d,mut reg_e,mut reg_f,mut reg_g,mut reg_h]=self.h;
        for t in 0..64 {
            let ch=(reg_e&reg_f)^(!reg_e&reg_g);
            let maj=(reg_a&reg_b)^(reg_a&reg_c)^(reg_b&reg_c);
            let s1=reg_e.rotate_right(6)^reg_e.rotate_right(11)^reg_e.rotate_right(25);
            let s0=reg_a.rotate_right(2)^reg_a.rotate_right(13)^reg_a.rotate_right(22);
            let t1=reg_h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[t]).wrapping_add(schedule[t]);
            let t2=s0.wrapping_add(maj);
            reg_h=reg_g;reg_g=reg_f;reg_f=reg_e;reg_e=reg_d.wrapping_add(t1);reg_d=reg_c;reg_c=reg_b;reg_b=reg_a;reg_a=t1.wrapping_add(t2);
        }
        for (v,add) in self.h.iter_mut().zip([reg_a,reg_b,reg_c,reg_d,reg_e,reg_f,reg_g,reg_h]) { *v=v.wrapping_add(add); }
    }
    pub(crate) fn update(&mut self, mut input:&[u8]) -> std::io::Result<()> {
        self.bytes=self.bytes.checked_add(input.len() as u64).filter(|b|*b<=u64::MAX/8)
            .ok_or_else(||std::io::Error::new(std::io::ErrorKind::InvalidInput,"material exceeds the SHA-256 length bound"))?;
        if self.used != 0 {
            let n=(64-self.used).min(input.len());
            self.block[self.used..self.used+n].copy_from_slice(&input[..n]);
            self.used+=n;
            if self.used<64 {return Ok(());}
            let b=self.block; self.compress(&b); self.used=0; input=&input[n..];
        }
        let (blocks,rest)=input.as_chunks::<64>();
        for b in blocks { self.compress(b); }
        self.block[..rest.len()].copy_from_slice(rest); self.used=rest.len();
        Ok(())
    }
    pub(crate) fn finish(mut self) -> [u8;32] {
        self.block[self.used]=0x80;
        if self.used>=56 {
            self.block[self.used+1..].fill(0);
            let b=self.block;self.compress(&b); self.block=[0;64];
        } else { self.block[self.used+1..56].fill(0); }
        self.block[56..].copy_from_slice(&(self.bytes*8).to_be_bytes());
        let b=self.block;self.compress(&b);
        let mut out=[0u8;32];
        for (to,h) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.h) { *to=h.to_be_bytes(); }
        out
    }
}
pub(crate) fn digest(bytes:&[u8]) -> std::io::Result<[u8;32]> {
    let mut h=Sha256::new(); h.update(bytes)?; Ok(h.finish())
}
