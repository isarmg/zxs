impl<'t,'s,'r> Current<'t,'s,'r> {
    pub(super) fn filesystem_capacity(&self,after_sample:impl FnOnce()->io::Result<()>)
        ->io::Result<(Option<super::super::volume::Capacity>,bool)>{
        let cut=self.capacity_cut()?;cut.physical()?;
        let measured=cut.sample().ok().filter(|s|s.total>=4096);
        after_sample()?;cut.physical()?;
        self.tree.session.source.at(|db,c|{self.placement_tree().require(db,&c,false)?;cut.logical(db,&c)})?;
        Ok((measured,self.setting.mode==super::share_mode::Mode::ReadWrite))
    }

    pub(in crate::system::reader) fn check_at_file(&self,cut:impl FnOnce()->io::Result<()>)->io::Result<()>{
        self.with_read(|reader|reader.check_with(cut,|_|Ok(())))
    }
}
