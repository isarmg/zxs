pub(in crate::system::reader) fn memory_navigation(db:&Connection,c:&Catalog,share:ShareId,parent:Option<DirectoryId>,
    after:Option<&Continuation>)->io::Result<(Vec<Directory>,Vec<Entry>,Option<Continuation>)>{
    navigation_cut(db,VOTES,||{
        let epoch=WebCurrent::select(db,c)?.ok_or_else(super::denied)?;
        select_page(db,c,&epoch,share,parent,after)
    })
}
