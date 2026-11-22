impl Reader<'_,'_,'_> {
    pub(super) fn current_standard_eof(&self)->io::Result<u64>{
        self.drive.with_site(|site|ReadAt{site,file:&self.original,previous_after_restore:false}.standard_eof())
    }
}
