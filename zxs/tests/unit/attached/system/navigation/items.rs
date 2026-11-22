pub(in crate::system::reader) fn memory_navigation(db:&Connection,c:&Catalog,share:ShareId,parent:Option<DirectoryId>,
    after:Option<&Continuation>)->io::Result<(Vec<Directory>,Vec<Entry>,Option<Continuation>)>{
    let (dirs,files,next,_,_)=memory_label_navigation(db,c,share,parent,after,None)?;
    Ok((dirs,files,next))
}

pub(in crate::system::reader) fn memory_label_navigation(db:&Connection,c:&Catalog,share:ShareId,parent:Option<DirectoryId>,
    after:Option<&Continuation>,filter:Option<&Label>)->io::Result<SelectedPage>{
    navigation_cut(db,VOTES,||{
        let epoch=WebCurrent::select(db,c)?.ok_or_else(super::denied)?;
        select_page(db,c,&epoch,share,parent,after,filter)
    })
}

pub(in crate::system::reader) fn memory_label_check(db:&Connection,c:&Catalog,selected:Selected,write:bool)->io::Result<()>{
    let epoch=WebCurrent::select(db,c)?.ok_or_else(super::denied)?;
    require_label_file(db,c,&epoch,selected,write)
}
