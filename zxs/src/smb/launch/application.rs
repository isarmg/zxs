//! The normal installed ZXS startup. Web and Rust SMB are fixed parts of
//! this one foreground product, not separately configured or started.
use super::{custody::{self,Parent},site::{SiteLease,Terminal,media::Media}};
use std::{io,thread,time::Duration};

const LEGACY_ENV:[&str;4]=[
    "ZXS_SYSTEM_ROOT","ZXS_POOL_ID","ZXS_WEB_TRUSTED_PROXY","ZXS_WEB_PUBLIC_ORIGIN",
];

fn reject_legacy_selection(present:impl Fn(&str)->bool)->io::Result<()>{
    if LEGACY_ENV.into_iter().any(present){
        return Err(io::Error::new(io::ErrorKind::Unsupported,
            "this fresh ZXS deployment must use its protected selection; legacy Web or proxy environment is not accepted"));
    }
    Ok(())
}
fn no()->io::Error{
    io::Error::other("both original ZXS components did not complete the same verified startup or drain")
}

/// Internal binary glue only: one fixed protected site and its own two
/// lowered roles. There are no caller-supplied settings or selectable
/// backends in this start.
pub fn start_original_zxs()->io::Result<()>{
    reject_legacy_selection(|key|std::env::var_os(key).is_some())?;
    let parent=Parent::pin().map_err(|_|no())?;
    let terminal=Terminal::select(&parent)?;
    let site=SiteLease::select(&parent)?;
    let selected=custody::prepare_site(&parent,&site,&terminal)?;
    let media=Media::select(&site,&selected)?;
    let mut accepted=custody::authorize(&parent,&site,&terminal,&selected,&media)?;
    let mut smb=custody::launch_internal_smb(&mut accepted)?;
    let mut web=match custody::launch_internal_web(&mut accepted,&mut smb){
        Ok(web)=>web,
        Err(e)=>{
            // This SMB reached its own PREPARED; it uses the original
            // normal drain even when the whole application did not start.
            let _=smb.begin_drain();
            let _=smb.finish();
            return Err(e);
        }
    };
    let mut result=(||{
        accepted.release_pair(&smb,&web)?;
        terminal.report(
            "ZXS：本次 Web 和内置 SMB 已核对同一原 SYSTEM 与凭据代，已共同放行。尚未确认客户端互通或旧单恢复完成。\n",
            ||{
                if custody::stopping()||web.exited()?.is_some()||smb.exited()?.is_some(){return Err(no());}
                Ok(())
            },
        )?;
        // The actual deployment/S/site holds stay owned by both Running
        // handles. Avoid filesystem I/O in this stop/exit monitor; each
        // lowered business path still rechecks its own original media.
        loop{
            if custody::stopping(){return Ok(());}
            if web.exited()?.is_some()||smb.exited()?.is_some(){return Err(no());}
            terminal.require()?;
            thread::sleep(Duration::from_millis(50));
        }
    })();
    // Request TERM from BOTH original, genuinely PREPARED births before
    // waiting on either. No rollback/replay or purported tidy exit after
    // an uncertain handoff. Each handle owns its real birth through reap.
    smb.begin_pair(&mut web);
    for receipt in [smb.finish(),web.finish()]{
        if result.is_ok(){result=receipt;}
    }
    if result.is_ok(){
        let _=terminal.report("ZXS：本次 Web 和内置 SMB 的两个原进程已确认退出；不表示每个文件已确认写入。\n",
            ||Ok(()));
    }
    result
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn standalone_never_converts_legacy_site_or_proxy_input_into_another_start(){
        assert!(reject_legacy_selection(|_|false).is_ok());
        for name in LEGACY_ENV{
            assert_eq!(reject_legacy_selection(|key|key==name).unwrap_err().kind(),io::ErrorKind::Unsupported);
        }
    }
}
