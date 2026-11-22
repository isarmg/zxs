/// Only a real original namespace OPEN may implement this in production.
/// Return the complete authorized projection from one current Core cut,
/// in exact UTF-8 byte order. Do not page it in advance, probe host names
/// or media, or report an unavailable/denied source as an empty projection.
/// The actual opened root, active subject and installed plan are all checked
/// by the source; this is not a permission check over caller-provided rows.
pub(in crate::smb::backend) trait OriginalNamespace {
    fn root_projection(&self) -> io::Result<Vec<Child>>;
}

impl RootPage {
    pub(in crate::smb::backend) fn written_for_fixture<H>(self,open:&mut Held<H>)->bool{
        self.into_effect().apply(open)
    }
}

impl<H:OriginalNamespace> Held<H>{
    pub(in crate::smb::backend) fn prepare_root_query(
        &mut self,query:&DirectoryQuery<'_>,room:usize,
    )->Result<RootPage,Fault>{
        self.prepare_root_query_with(query,room,|h|h.root_projection().map(|v|(v,()))).map(|(p,())|p)
    }
}
