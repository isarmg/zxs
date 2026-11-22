pub(super) fn serve(ls:&TcpListener,app:&mut App<'_>,stopping:impl Fn()->bool+Sync)->io::Result<()>{
    let accepting=AtomicBool::new(true); // Only a fixture's private listener.
    serve_after(ls,app,stopping,&accepting,||Ok(()))
}
