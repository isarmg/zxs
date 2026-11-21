//! Only one real non-secret reset challenge and one quiet-mode transition
//! on a disposable PTY. Never launch a worker or read credential input.
use super::{PublicConfirmation,begin_held_quiet,same_mode};
use rustix::{fs::{OFlags,fcntl_getfl,fcntl_setfl},io::Errno,process::{getpid,getuid,ioctl_tiocsctty,setsid},
    pty::{OpenptFlags,grantpt,ioctl_tiocgptpeer,openpt,unlockpt},termios::{LocalModes,tcgetattr,tcsetpgrp}};
use std::{fs::File,io,os::fd::AsFd,process::{Command,Stdio},time::{Duration,Instant}};
const CHILD:&str="HELPER_UNIT_SHARED_RESET_PTY_CHILD";
const TEST:&str="sqlite_install::pair::terminal::shared_reset_terminal::shared_account_reset_real_public_challenge_releases_the_mask_before_hidden_mode";
const DONE:&[u8]=b"SMB-RESET-FIXTURE-MASK-AND-TTY-RESTORED";

#[test]
fn shared_account_reset_real_public_challenge_releases_the_mask_before_hidden_mode(){
    if getuid().is_root(){return;} // scope: ordinary unit subprocess only
    if std::env::var_os(CHILD).is_some(){
        let stdin=io::stdin();setsid().unwrap();
        ioctl_tiocsctty(&stdin).unwrap();tcsetpgrp(&stdin,getpid()).unwrap();
        let tty=PublicConfirmation::select().unwrap();
        let original=tcgetattr(&stdin).unwrap();
        tty.confirm_shared_account_reset("synthetic offline reset preview, no password or vault\n".into()).unwrap();
        let (mut silent,signals)=begin_held_quiet(stdin.as_fd()).unwrap();
        signals.check().unwrap();silent.verify().unwrap();
        assert!(!tcgetattr(&stdin).unwrap().local_modes.intersects(LocalModes::ICANON|LocalModes::ECHO));
        silent.restore().unwrap();signals.check().unwrap();signals.finish().unwrap();
        assert!(same_mode(&original,&tcgetattr(&stdin).unwrap()));
        tty.report(std::str::from_utf8(DONE).unwrap()).unwrap();
        return;
    }
    let flags=OpenptFlags::RDWR|OpenptFlags::NOCTTY|OpenptFlags::CLOEXEC;
    let master=File::from(openpt(flags).unwrap());
    grantpt(&master).unwrap();unlockpt(&master).unwrap();
    let slave=File::from(ioctl_tiocgptpeer(&master,flags).unwrap());
    fcntl_setfl(&master,fcntl_getfl(&master).unwrap()|OFlags::NONBLOCK).unwrap();
    let mut sub=Command::new(std::env::current_exe().unwrap()).args(["--exact",TEST,"--nocapture"])
        .env(CHILD,"1").stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap())).stderr(Stdio::from(slave)).spawn().unwrap();
    let result=(||->io::Result<()>{
        let end=Instant::now()+Duration::from_secs(8);
        let marker="输入 RESET-ZXS-ACCOUNT ".as_bytes();
        let mut out=Vec::new();let mut answered=false;let mut buf=[0u8;2048];
        while Instant::now()<end{
            match rustix::io::read(&master,&mut buf){
                Ok(n)=>out.extend_from_slice(&buf[..n]),
                Err(Errno::AGAIN|Errno::INTR|Errno::IO)=>(),
                Err(e)=>return Err(e.into()),
            }
            if out.len()>32768{return Err(io::Error::other("reset PTY report exceeded its budget"));}
            if !answered {
                if let Some(start)=out.windows(marker.len()).position(|v|v==marker).map(|p|p+marker.len()){
                    if let Some(key)=out.get(start..start+32){
                        if !key.iter().all(|b|b.is_ascii_digit()||matches!(b,b'a'..=b'f')){
                            return Err(io::Error::other("not a real public reset challenge"));
                        }
                        let answer=[b"RESET-ZXS-ACCOUNT ",key,b"\n"].concat();
                        if rustix::io::write(&master,answer.as_slice())?!=answer.len(){
                            return Err(io::Error::other("short disposable challenge response"));
                        }
                        answered=true;
                    }
                }
            }
            if let Some(status)=sub.try_wait()?{
                return if status.success()&&answered&&out.windows(DONE.len()).any(|v|v==DONE){Ok(())}
                    else{Err(io::Error::other(format!("reset PTY fixture did not close: {}",String::from_utf8_lossy(&out))))};
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::other("reset PTY fixture timed out"))
    })();
    if result.is_err(){let _=sub.kill();let _=sub.wait();}
    result.unwrap();
}
