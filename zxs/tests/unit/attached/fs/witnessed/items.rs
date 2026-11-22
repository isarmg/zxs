impl HeldBytes {
    pub(crate) fn separate_format_fixture(file:File)->io::Result<Self>{
        Ok(Self{original:LiveStamp::read(&file)?,file,extent:Extent::Whole})
    }
}
