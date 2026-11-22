impl<'owner,'raw> CheckedBatch<'owner,'raw> {
    pub(in crate::smb::connection) fn into_move_escrow(mut self,capture:MoveCapture<'_,'_>)->PeerResult<MoveEscrow>{
        if !capture.matches(self.original.frame){return Err(Fault::WrongBatch);}
        let prepared=self.qualify_ordinary_cross_filesystem_move().map_err(|e|match e{
            MoveQualificationError::Connection(e)=>e,
            _=>Fault::WrongReply,
        })?.ok_or(Fault::WrongReply)?;
        self.retain_prepared_move(capture,prepared)
    }
}
