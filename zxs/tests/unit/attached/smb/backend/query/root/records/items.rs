impl FullRootRecords {
    pub(in crate::smb::backend) fn written_for_fixture<H>(self, open: &mut crate::smb::backend::Held<H>) -> bool {
        self.proposal.written_for_fixture(open)
    }
}
