fn create_inner(system:&Path,op:OperationId,id:VolumeId,name:&str,cut:impl FnMut(Cut)->io::Result<()>)->io::Result<FileInfo>{
    create_at_inner(system,op,DirectoryParent::Root(id),name,cut)
}
