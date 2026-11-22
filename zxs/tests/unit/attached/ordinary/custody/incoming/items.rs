impl<'v> IncomingSelection<'v> {
    pub(super) fn prepare(system:&Path,source:&'v Verified,r:Request)->io::Result<PreparedIncoming<'v>>{
        Self::prepare_checked(system,source,r,|_|Ok(()))
    }
}
