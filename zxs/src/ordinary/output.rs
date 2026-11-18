//! A checked sink never retransmits accepted bytes when its post-write observation is interrupted.
use std::io::{self,Write};
pub(super) struct CheckedOutput<W,C>{pub(super) out:W,pub(super) check:C}
fn after_output(check:&mut impl FnMut()->io::Result<()>)->io::Result<()>{
    // The sink has already accepted bytes. An interrupted fstat can retry
    // only that observation, never signal write_all to repeat the payload.
    loop{match check(){Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,r=>return r}}
}
impl<W:Write,C:FnMut()->io::Result<()>> Write for CheckedOutput<W,C>{
    fn write(&mut self,bytes:&[u8])->io::Result<usize>{
        (self.check)()?;let n=self.out.write(bytes)?;after_output(&mut self.check)?;Ok(n)
    }
    fn flush(&mut self)->io::Result<()>{
        (self.check)()?;self.out.flush()?;after_output(&mut self.check)
    }
}
#[cfg(test)]
mod checked_output_tests{
    use super::*;
    #[test]
    fn interrupted_post_write_check_retries_observation_not_already_delivered_bytes(){
        let mut seen=Vec::new();let mut checks=0u8;
        {
            let mut out=CheckedOutput{out:&mut seen,check:||{
                checks+=1;if checks==2{return Err(io::Error::from(io::ErrorKind::Interrupted));}Ok(())
            }};
            out.write_all(b"one payload").unwrap();
        }
        assert_eq!(checks,3);assert_eq!(seen,b"one payload");
    }
}
