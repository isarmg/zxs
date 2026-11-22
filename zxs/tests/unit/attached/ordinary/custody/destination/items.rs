impl<'s> Destination<'s> {
    pub(super) fn select(system:&Path,src:&'s Verified,to:VolumeId)->io::Result<Self>{Self::select_at(system,src,to,ParentChoice::Roots)}
}
