impl PendingOriginalMove {
    fn prepare(input:CapturedRecord,owner:&mut Connection)->Result<Self>{
        let input=OriginalMoveInput::new(input);
        let held={
            let frame=input.frame_on(owner)?;
            let [raw]=frame.messages()else{return Err(Fault::WrongReply);};
            if !raw.is_basic_rename_request()||owner.in_flight!=0{return Err(Fault::WrongReply);}
            let capture=MoveCapture{frame:&frame,owned:&input};
            let paid=owner.receive(&frame)?;
            let admitted=paid.preflight(owner);
            match admitted{
                Admission::Ready(batch)=>batch.into_move_escrow(capture)?,
                Admission::TransportGone=>return Err(Fault::Closed),
                _=>return Err(Fault::SessionState),
            }
        };
        let pending=Self{held,input,control:move_control::MoveControl::default()};
        pending.require_original(owner)?;
        Ok(pending)
    }
}

impl Peer {
    /// Private direct receiver seam for negative ownership fixtures.
    /// The live dispatcher keeps its already paid original instead.
    fn park_original_move(&mut self,input:CapturedRecord)->Result<()>{
        if let Err(e)=input.require_owner(&self.connection){
            if e!=Fault::WrongBatch{self.disconnect_original();}
            return Err(e);
        }
        let prepare=if self.original_move.is_none(){PendingOriginalMove::prepare(input,&mut self.connection)}
            else{Err(Fault::WrongBatch)};
        match prepare.and_then(|pending|self.install_pending_move(pending)){
            Ok(())=>Ok(()),
            Err(e)=>{self.disconnect_original();Err(e)},
        }
    }

    fn poll_with(&mut self,issuer:Issuer,other:impl for<'c,'r> FnOnce(Outstanding<'c,'r>)->Result<()>)->Result<Turn> {
        self.poll_selected(issuer,false,other)
    }
}
