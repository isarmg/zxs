impl WebAdmin<'_> {
    pub(crate) fn share_setting(&self,share:ShareId)->io::Result<Setting>{
        self.source.at(|db,c|{self.current(db,&c)?;current(db,&c,share)})
    }

    pub(crate) fn set_share_mode(&self,op:OperationId,share:ShareId,previous:OperationId,mode:Mode)->io::Result<Setting>{
        let request=Change::new(self,op,share,previous,mode)?;
        self.metadata_change(|db,c|request.select(db,c),||Ok(()),|db,next,()|request.insert(db,next))
    }
}
