impl<'v> PreparedIncoming<'v> {
    pub(crate) fn inspection(&self)->io::Result<Vec<(Entity,Birth,u64,[u8;32])>>{
        self.files.iter().map(|(_,f,st)|Ok((st.stat.entity(),st.birth.clone(),st.stat.size(),stream::hash_unnamed(f,st)?))).collect()
    }
}
