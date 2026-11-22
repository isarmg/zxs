impl RunningServerGuid {
    pub(super) fn fixture_revoke_provider(&self) {
        self.run.verifier.as_ref().expect("the fixture holds a provider").fixture_revoke();
    }
}

impl SelectionSend<'_,'_> {
    pub(in crate::smb) fn bytes(&self)->&[u8] {self.tx.bytes()}

    pub(in crate::smb) fn submitted(self)->Result<()> {self.submitted_at(Instant::now())}

    fn submitted_at(self,now:Instant)->Result<()> {
        let Self{tx,claim,next}=self;tx.publish_with(|c|Self::install(c,&claim,next,now))
    }
}
