//! Real non-secret prompt and phase order on a newly created disposable PTY.
//! No ZXS authority, host credentials, pool paths or prepared file writes.
use super::{filesystem_phase,PublicConfirmation};
use rustix::{fs::{OFlags,fcntl_getfl,fcntl_setfl},io::Errno,
    process::{getpid,getuid,ioctl_tiocsctty,setsid},
    pty::{OpenptFlags,grantpt,ioctl_tiocgptpeer,openpt,unlockpt},termios::tcsetpgrp};
use std::{fs::File,io,process::{Command,Stdio},time::{Duration,Instant}};

const CHILD:&str="HELPER_UNIT_FRESH_PROMPT_CHILD";
const TEST:&str="sqlite_install::pair::fresh_issuer::terminal_tests::fresh_v18_one_real_consent_allows_original_scoped_phase_without_second_prompt";

#[test]
fn fresh_v18_one_real_consent_allows_original_scoped_phase_without_second_prompt(){
    if std::env::var_os(CHILD).is_some(){
        assert!(!getuid().is_root());
        let stdin=io::stdin();setsid().unwrap();
        ioctl_tiocsctty(&stdin).unwrap();tcsetpgrp(&stdin,getpid()).unwrap();
        let tty=PublicConfirmation::select().unwrap();
        tty.confirm_fresh_zxs("fixture initial preview (no site, no disk, no password)\n".into()).unwrap();
        filesystem_phase(&||tty.check(),|check|{check()?;check()}).unwrap();
        filesystem_phase(&||tty.check(),|check|check()).unwrap();
        tty.report("FRESH-FIXTURE-ONE-PROMPT-CLOSED\n").unwrap();
        return;
    }
    if getuid().is_root(){return;} // this fixture must never turn into root maintenance
    let flags=OpenptFlags::RDWR|OpenptFlags::NOCTTY|OpenptFlags::CLOEXEC;
    let master=File::from(openpt(flags).unwrap());
    grantpt(&master).unwrap();unlockpt(&master).unwrap();
    let slave=File::from(ioctl_tiocgptpeer(&master,flags).unwrap());
    fcntl_setfl(&master,fcntl_getfl(&master).unwrap()|OFlags::NONBLOCK).unwrap();
    let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact",TEST,"--nocapture"])
        .env(CHILD,"1").stdin(Stdio::from(slave.try_clone().unwrap())).stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave)).spawn().unwrap();
    let pass=(||->io::Result<()>{
        let stop=Instant::now()+Duration::from_secs(12);
        let mut observed=Vec::new();let mut stage=0;let mut buf=[0;2048];
        let stages=[b"ISSUE-ZXS-V18".as_slice()];
        while Instant::now()<stop{
            match rustix::io::read(&master,&mut buf[..]){
                Ok(n)=>observed.extend_from_slice(&buf[..n]),
                Err(Errno::INTR|Errno::AGAIN|Errno::IO)=>(),
                Err(e)=>return Err(e.into()),
            }
            if observed.len()>32768{return Err(io::Error::other("unbounded PTY fixture"));}
            if let Some(verb)=stages.get(stage){
                let mut prefix=b"\xe8\xbe\x93\xe5\x85\xa5 ".to_vec(); // the actual prompt's non-secret “输入 ”
                prefix.extend(*verb);prefix.push(b' ');
                if let Some(i)=observed.windows(prefix.len()).position(|x|x==prefix){
                    let start=i+prefix.len();
                    if let Some(key)=observed.get(start..start+32){
                        if !key.iter().all(|&v|v.is_ascii_digit()||matches!(v,b'a'..=b'f')){
                            return Err(io::Error::other("wrong fresh non-secret challenge"));
                        }
                        let answer=[*verb,b" ",key,b"\n"].concat();
                        if rustix::io::write(&master,answer.as_slice())?!=answer.len(){
                            return Err(io::Error::other("short fixture response"));
                        }
                        observed.clear();stage+=1;
                    }
                }
            }
            if let Some(done)=child.try_wait()?{
                return if done.success()&&stage==1&&observed.windows(b"FRESH-FIXTURE-ONE-PROMPT-CLOSED".len()).any(|v|v==b"FRESH-FIXTURE-ONE-PROMPT-CLOSED"){
                    Ok(())
                }else{Err(io::Error::other(format!("fixture child {done}, answered {stage}: {}",String::from_utf8_lossy(&observed))))};
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::other(format!("fixture timed out at {stage}: {}",String::from_utf8_lossy(&observed))))
    })();
    if pass.is_err(){let _=child.kill();let _=child.wait();}
    pass.unwrap();
}
