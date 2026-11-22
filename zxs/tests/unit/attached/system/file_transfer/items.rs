impl Tree<'_,'_> {
    fn transfer_after(&self,ask:&Handoff,supplied:OperationId,live:impl Fn()->io::Result<()>,
        point:impl FnMut(Cut)->io::Result<()>)->io::Result<Moved>{
        self.with_transfer(None,|at|at.transfer_inner(ask,supplied,None,live,false,point))
    }

    fn ordinary_after(&self,target:&Tree<'_,'_>,opened:&OrdinaryMoveSource,name:String,op:OperationId,
        point:impl FnMut(Cut)->io::Result<()>,select:impl Fn()->io::Result<()>)->io::Result<Moved>{
        if self.setting!=opened.grant{return Err(io::ErrorKind::PermissionDenied.into());}
        let original=OpenedOrdinaryMove{original:&opened.physical,select:&select};
        self.with_transfer(Some(target),|at|at.transfer_inner(
            &Handoff::MoveOrdinary{source:opened.place.clone(),target:name},op,Some(original),||Ok(()),false,point))
    }
}

#[test]
fn stored_transfer_directions_bind_the_exact_source_and_target_roles(){
    use crate::Role::{File,Recycle};
    let expected=[
        (1,Direction::Delete,File,Recycle),
        (2,Direction::Restore,Recycle,File),
        (3,Direction::Ordinary,File,File),
    ];
    for (tag,direction,source,target) in expected{
        assert_eq!(Direction::from_stored(tag),Some(direction));
        assert_eq!(direction.stored(),tag);
        assert_eq!((direction.source_role(),direction.target_role()),(source,target));
        assert_eq!(direction.ordinary_destination(),target==File);
    }
    for tag in (u8::MIN..=u8::MAX).filter(|tag|!(1..=3).contains(tag)){
        assert_eq!(Direction::from_stored(tag),None);
    }
}

impl Tree<'_,'_>{
    pub(super) fn fixture_confirmed_move(&self,target:&Tree<'_,'_>,db:&Connection,c:&Catalog,
        original:&Place,op:OperationId)->io::Result<Moved>{
        self.with_transfer(Some(target),|at|
            Route::receipt(&at,db,c,&Handoff::MoveOrdinary{source:original.clone(),target:original.name.clone()},op)?
                .ok_or_else(||invalid("no confirmed original fixture move")))
    }
}
