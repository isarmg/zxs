//! Pin the same live root socket peer and independently born child.
use super::bad;
use rustix::{net::{sockopt,AddressFamily,SocketType},process::{getppid,parent_process_death_signal,Pid,Signal}};
use std::{fs::File,io};

pub(super) fn original_parent(fd:&File,expected:Pid)->io::Result<()>{
    let who=sockopt::socket_peercred(fd)?;
    if who.pid!=expected||!who.uid.is_root()||!who.gid.is_root()||getppid()!=Some(expected)
        ||parent_process_death_signal()?!=Some(Signal::TERM)||sockopt::socket_domain(fd)?!=AddressFamily::UNIX
        ||sockopt::socket_type(fd)?!=SocketType::SEQPACKET||!sockopt::socket_passcred(fd)?{
        return Err(bad("not the live original root account supervisor"));
    }Ok(())
}
