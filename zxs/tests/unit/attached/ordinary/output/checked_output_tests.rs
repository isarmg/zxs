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
