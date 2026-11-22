//! One bounded original Core cold lane for each fixed obligation. Their
//! readers are opened locally under the same actual supervised startup.
use super::{Fault,InheritedRun,Result,entry::stopping};
use crate::system::reader::smb::runtime::QualifiedRustInstall;
use std::{sync::{Arc,mpsc::{SyncSender,sync_channel}},thread::{self,Scope,ScopedJoinHandle}};

pub(super) enum Task{Retirement,Rename,DirectoryRelabel}
impl Task{
    fn name(&self)->&str{
        match self{
            Self::Retirement=>"source-retire",
            Self::Rename=>"rename-recover",
            Self::DirectoryRelabel=>"relabel-recover",
        }
    }
}
pub(super) fn prepare<'s,'e>(
    scope:&'s Scope<'s,'e>,original:Arc<InheritedRun>,task:Task,
)->Result<(SyncSender<()>,ScopedJoinHandle<'s,Result<()>>)>{
    let (prepared,selected)=sync_channel(1);
    let (release,begin)=sync_channel(1);
    let job=thread::Builder::new().name(task.name().into()).spawn_scoped(scope,move||{
        // No live Connection or SQLite handle crosses a thread. This is
        // the exact inherited parent/paths/plan, not an alternate authority.
        let site=QualifiedRustInstall::attach(original).map_err(|_|Fault::Handoff)?;
        prepared.send(()).map_err(|_|Fault::Handoff)?;
        drop(prepared);
        if begin.recv().is_err()||stopping(){return Ok(());}
        match task{
            Task::Retirement=>site.finish_confirmed_sources(stopping),
            Task::Rename=>site.settle_interrupted_file_renames(stopping),
            Task::DirectoryRelabel=>site.settle_interrupted_directory_relabels(stopping),
        }.map_err(|_|Fault::Handoff)
    }).map_err(|_|Fault::Resources)?;
    if selected.recv().is_err(){let _=job.join();return Err(Fault::Handoff);}
    Ok((release,job))
}
