impl TreeTable {
    pub(in crate::smb::connection::sessions) fn count(&self)->usize {self.live.len()}
}
