//! A single terminal-selected operand. The HTTP process has no command builder
//! with user options, and never installs the checker's process-wide limits.
use super::{Outcome,Report,Request,image::Sealed,runner,runtime::Runtime};
use crate::{fs_tools::RuntimeFile,pinned::invalid};
use rustix::{
    io::fcntl_dupfd_cloexec,pipe::{PipeFlags,pipe_with},
    process::{Pid,Signal,kill_process,parent_process_death_signal}};
use std::{fs::File,io::{self,Read,Write},process::{Child,Command,Stdio}};

#[derive(Clone,Copy)]
pub(crate) struct Info {pub bytes:u64,pub sector:u16,pub clusters:u32,pub digest:[u8;32]}
pub(crate) struct Operand {image:Sealed,expected:[u8;32],installation:RuntimeFile}

impl Operand {
    pub(crate) fn capture(job:Request)->io::Result<Self>{
        let installation=RuntimeFile::open(&["usr","bin","helper"],true,64<<20)?;
        installation.require_running_helper()?;
        let image=Sealed::capture(&job.image,&job.digest)?;
        installation.recheck()?;
        Ok(Self{image,expected:job.digest,installation})
    }
    pub(crate) fn info(&self)->Info{
        let g=self.image.geometry();
        Info{bytes:g.length,sector:g.sector_bytes,clusters:g.data_clusters,digest:self.expected}
    }
    pub(crate) fn launch(self)->io::Result<Running>{
        let (read,write)=pipe_with(PipeFlags::CLOEXEC|PipeFlags::NONBLOCK)?;
        let mut cmd=Command::new("/no-path-execution-for-helper-worker");
        cmd.env_clear().current_dir("/").stdin(Stdio::from(self.image.file().try_clone()?))
            .stdout(Stdio::from(File::from(write))).stderr(Stdio::piped());
        helper_linux_evidence::diagnostic_process::require_waitable_children()?;
        self.installation.recheck()?;self.installation.require_running_helper()?;
        helper_linux_evidence::diagnostic_process::prepare_panel_worker_exec(&mut cmd,&self.installation.file,&self.expected)?;
        let child=cmd.spawn()?;
        Ok(Running{child,read:File::from(read),clusters:self.image.geometry().data_clusters})
    }
}

// Wire over a dedicated server-owned pipe: magic, result kind, 3 big-endian
// counts. A valid packet and a matching *actual* exit status are both required.
// stdout of fsck itself never reaches the panel.
const SIZE:usize=17;
fn packet(r:&Report)->[u8;SIZE]{
    let mut p=[0;SIZE];p[..4].copy_from_slice(b"FCR1");
    p[4]=match r.state{Outcome::NoFindingsReported=>0,Outcome::ReportedMessages=>1,Outcome::FindingsOrToolError=>2};
    p[5..9].copy_from_slice(&r.files.to_be_bytes());p[9..13].copy_from_slice(&r.used.to_be_bytes());
    p[13..17].copy_from_slice(&r.total.to_be_bytes());p
}
fn result(p:&[u8],status:Option<i32>,clusters:u32)->io::Result<Report>{
    let bad=||invalid("no complete result from the selected copy checker");
    if p.len()!=SIZE||&p[..4]!=b"FCR1"{return Err(bad());}
    let state=match (p[4],status){(0,Some(0))=>Outcome::NoFindingsReported,(1,Some(1))=>Outcome::ReportedMessages,
        (2,Some(1))=>Outcome::FindingsOrToolError,_=>return Err(bad())};
    let field=|i:usize|u32::from_be_bytes(p[i..i+4].try_into().expect("four-byte field"));
    let(files,used,total)=(field(5),field(9),field(13));if used>total||total!=clusters{return Err(bad());}
    Ok(Report{state,files,used,total})
}

pub(crate) fn worker(expected:[u8;32])->io::Result<u8>{
    if parent_process_death_signal()?!=Some(Signal::TERM){return Err(invalid("no supervised copy-check parent"));}
    runner::prepare_worker()?;
    let file=File::from(fcntl_dupfd_cloexec(std::io::stdin(),3)?);
    let image=Sealed::from_sealed(file,&expected)?;
    let runtime=Runtime::open()?;
    let report=runner::check(&runtime,&image)?;
    let mut out=io::stdout().lock();out.write_all(&packet(&report))?;out.flush()?;Ok(report.exit())
}

pub(crate) struct Running{child:Child,read:File,clusters:u32}
impl Running{
    fn waitable(&mut self)->io::Result<Option<std::process::ExitStatus>>{
        loop{match self.child.try_wait(){Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,r=>return r}}
    }
    fn read_result(&mut self,exit:std::process::ExitStatus)->io::Result<Report>{
        let mut bytes=Vec::with_capacity(SIZE+1);
        (&mut self.read).take((SIZE+1) as u64).read_to_end(&mut bytes)?;
        result(&bytes,exit.code(),self.clusters)
    }
    pub(crate) fn poll(&mut self)->io::Result<Option<Report>>{
        let Some(exit)=self.waitable()? else{return Ok(None);};
        self.read_result(exit).map(Some)
    }
    fn stop_and_wait(&mut self)->io::Result<std::process::ExitStatus>{
        if let Some(exit)=self.waitable()?{return Ok(exit);}
        // The single-threaded panel is the sole parent/reaper. This still
        // unreaped child's identity cannot be reused before the wait below.
        let pid=Pid::from_raw(self.child.id() as i32).ok_or_else(||invalid("invalid checker child identity"))?;
        match kill_process(pid,Signal::TERM){
            Ok(())|Err(rustix::io::Errno::SRCH)=>{},Err(e)=>return Err(e.into()),
        }
        loop{match self.child.wait(){Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,r=>return r}}
    }
    pub(crate) fn finish(&mut self)->io::Result<()>{
        let exit=self.stop_and_wait()?;
        // Merely observing the worker exit (e.g. SIGKILL or code 2) does not
        // prove it observed its sandbox's complete termination and reap.
        self.read_result(exit).map(|_|())
    }
}
impl Drop for Running{
    fn drop(&mut self){let _=self.stop_and_wait();} // Last-resort cleanup; never used as a positive receipt.
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn unknown_code_partial_extra_and_wrong_geometry_never_confirm_a_check(){
        let r=Report{state:Outcome::ReportedMessages,files:4,used:8,total:65530};let p=packet(&r);
        assert_eq!(result(&p,Some(1),65530).unwrap().files,4);
        assert!(result(&p[..16],Some(1),65530).is_err());assert!(result(&[p.as_slice(),&[0]].concat(),Some(1),65530).is_err());
        assert!(result(&p,None,65530).is_err());assert!(result(&p,Some(0),65530).is_err());assert!(result(&p,Some(1),65529).is_err());
        let mut bad=p;bad[4]=3;assert!(result(&bad,Some(1),65530).is_err());
        let mut bad=p;bad[9..13].copy_from_slice(&65531u32.to_be_bytes());assert!(result(&bad,Some(1),65530).is_err());
    }
}
