impl Connection {
    fn negotiation(&self) -> Result<&ConnectionHash> {
        self.alive()?; Ok(&self.origin.hash)
    }
}

impl ReplySend<'_,'_,'_> {
    /// Only after successful submission of these bytes on THIS transport.
    /// The exclusive borrow holds its ledger between selecting and publishing
    /// credits; no competing response can publish the same sequence range.
    fn submitted(self) -> Result<()> { self.publish_with(|_| Ok(())) }
}
