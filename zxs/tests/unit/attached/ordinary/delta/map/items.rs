#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum Source{Full,Pack(usize),Zero}

impl Map {
    fn resolve<'a>(full: u64, layers: impl IntoIterator<Item=&'a Map>, at: u64, len: u64,
        mut emit: impl FnMut(Source, u64, u64)->io::Result<()>) -> io::Result<()> {
        let file=crate::FileId([1;16]); let op=crate::OperationId([2;16]);
        let mut view=Index::full(Material::declared(file,op,0,full)?)?;
        for (i,m) in layers.into_iter().enumerate() {
            let slot=u32::try_from(i+1).map_err(|_|invalid("fixture ordinal overflow"))?;
            view=m.flatten(&view,Material::declared(file,op,slot,m.changed)?)?;
        }
        view.visit(at,len,|leaf,off,n|emit(match leaf {
            Leaf::Zero=>Source::Zero, Leaf::Data(x) if x.slot()==0=>Source::Full,
            Leaf::Data(x)=>Source::Pack((x.slot()-1) as usize),
        },off,n))
    }
}
