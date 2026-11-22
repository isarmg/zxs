impl Invocation {
    fn at(system:&Path,r:Request)->io::Result<Self>{let(p,_,_)=LocalPool::with_snapshot(system,Access::Read)?;Self::new(&p,r)}
}

pub(super) fn decide(system:&Path,r:Request,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<Moved>{
    decide_for(system,&Invocation::at(system,r)?,hook)
}

/// Complete this private pair only; an already published historical outcome
/// is a receipt, not a request to reopen later media or spend a newer credit.
pub(super) fn perform(system:&Path,r:Request,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<super::command::MoveReceipt>{
    perform_for(system,&Invocation::at(system,r)?,hook)
}
