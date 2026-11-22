impl LocalPool {
    pub(crate) fn open(path:&Path,access:Access)->io::Result<Self>{
        Self::with_snapshot(path,access).map(|(pool,_,_)|pool)
    }
}
