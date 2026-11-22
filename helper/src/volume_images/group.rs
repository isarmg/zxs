//! Keep the checker tests in their original private harness.
pub(super) use crate::isolated_group::{prepare,Group};

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File,io::{Read,Write},process::{Command,Stdio},time::{Instant,Duration}};
    const N:&str="volume_images::group::tests::";
    fn own_test(name:&str)->Command{
        let mut p=Command::new(std::env::current_exe().unwrap());
        p.args(["--ignored","--exact",&format!("{N}{name}"),"--nocapture"]);p
    }
    fn selected(name:&str)->bool{
        // Running the ordinary harness (even with just --ignored) must never
        // change process-wide signals or start a fixture descendant.
        let target=format!("{N}{name}");
        let args:Vec<_>=std::env::args_os().collect();
        args.iter().any(|a|a.as_os_str()==std::ffi::OsStr::new("--exact"))
            && args.iter().any(|a|a.as_os_str()==std::ffi::OsStr::new(&target))
    }
    fn pause_fixture(){std::thread::sleep(Duration::from_secs(7));}
    // These are separately selected, bounded fixture processes. No shell,
    // check tool, storage or system service is involved.
    #[test] #[ignore="internal process-domain fixture"]
    fn fixture_early_setup(){
        if !selected("fixture_early_setup"){return;}
        println!("STAGE-PROCESS-READY");std::io::stdout().flush().unwrap();pause_fixture();
    }
    #[test] #[ignore="internal process-domain fixture"]
    fn fixture_in_monitor(){
        if !selected("fixture_in_monitor"){return;}
        let mut child=own_test("fixture_early_setup").spawn().unwrap();
        pause_fixture();let _=child.wait();
    }
    #[test] #[ignore="internal process-domain fixture"]
    fn fixture_before_payload(){
        if !selected("fixture_before_payload"){return;}
        let mut child=own_test("fixture_in_monitor").spawn().unwrap();
        pause_fixture();let _=child.wait();
    }
    #[test] #[ignore="internal dedicated supervisor fixture"]
    fn fixture_supervisor(){
        if !selected("fixture_supervisor"){return;}
        prepare().unwrap();
        for (stage,min) in [("fixture_in_monitor",2),("fixture_before_payload",3)]{
            let mut p=own_test(stage);p.stdout(Stdio::piped());p.stderr(Stdio::piped());
            let exe=File::open(std::env::current_exe().unwrap()).unwrap();
            helper_linux_evidence::diagnostic_process::prepare_diagnostic_exec(&mut p,&exe).unwrap();
            let mut group=Group::new(p.spawn().unwrap());drop(p);
            let mut report=group.child.stdout.take().unwrap();
            let f=rustix::fs::fcntl_getfl(&report).unwrap();
            rustix::fs::fcntl_setfl(&report,f|rustix::fs::OFlags::NONBLOCK).unwrap();
            let start=Instant::now();let mut seen=Vec::new();let mut b=[0;1024];
            while start.elapsed()<Duration::from_secs(5)&&seen.len()<8192&&!seen.windows(19).any(|s|s==b"STAGE-PROCESS-READY"){
                match report.read(&mut b){Ok(0)=>break,Ok(n)=>seen.extend_from_slice(&b[..n]),
                    Err(e) if matches!(e.kind(),std::io::ErrorKind::WouldBlock|std::io::ErrorKind::Interrupted)=>std::thread::sleep(Duration::from_millis(5)),Err(e)=>panic!("{e}"),}
            }
            let ready=seen.windows(19).any(|s|s==b"STAGE-PROCESS-READY");
            let outcome=group.finish().unwrap();
            assert!(ready);assert!(outcome.members>=min);assert_eq!(outcome.leader_code,None);
        }
        println!("\nSUPERVISOR-FIXTURE-COMPLETE");
    }
    #[test]
    fn original_group_is_collected_during_setup_and_after_a_third_generation_appears(){
        // Only the isolated child changes signal handling or becomes a
        // subreaper. The parent harness does not adopt or signal other tests.
        let mut child=own_test("fixture_supervisor").stdin(Stdio::null())
            .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let start=Instant::now();
        let status=loop{
            if let Some(status)=child.try_wait().unwrap(){break status;}
            if start.elapsed()>Duration::from_secs(18){
                let _=child.kill();let _=child.wait();panic!("isolated supervisor exceeded its fixture deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success(),"isolated supervisor fixture failed");
        let mut out=Vec::new();
        child.stdout.take().unwrap().take(4097).read_to_end(&mut out).unwrap();
        assert!(out.len()<=4096 && out.split(|b|*b==b'\n').any(|line|line==b"SUPERVISOR-FIXTURE-COMPLETE"));
    }
}
