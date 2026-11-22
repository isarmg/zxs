impl<'r> SmbSession<'r> {
    pub(in crate::system::reader) fn fixture_authenticated(source:&'r InitialReader,login:&str)->io::Result<Self>{
        let original=source.at(|db,c|Credential::current(db,&c))?;
        if original.login!=login.as_bytes(){return Err(denied());}Ok(Self{source,original})
    }

    /// A SQL fixture may choose a typed ShareId; it is not a wire service name.
    pub(crate) fn connect(&self,share:ShareId)->io::Result<Tree<'_,'r>>{self.connect_id(share)}
}
