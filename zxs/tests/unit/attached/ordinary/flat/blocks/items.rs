impl Reader {
    pub(super) fn from_test_files(original:Ref,raw:File,table:File)->io::Result<Self>{
        Self::open(original,HeldBytes::separate_format_fixture(raw)?,HeldBytes::separate_format_fixture(table)?)
    }
}
