impl Peer {
    fn open_fixture(server:&RunningServerGuid,stream:DirectStream)->Result<Self> {
        if !matches!(&stream,DirectStream::Fixture(_)) {return Err(Fault::Transport);}
        Self::open(server,Incoming{stream,endpoints:None})
    }
}

impl OpeningSend {
    /// Existing pure wire/state fixtures never open sockets. No production
    /// method can manually acknowledge a NEGOTIATE without doing the I/O.
    pub(in crate::smb) fn submitted(self)->Result<Connection> {
        let seat=AcceptSeat::reserve(&self.run)?;self.after_write(Completion::WithoutSocket,seat)
    }
}
