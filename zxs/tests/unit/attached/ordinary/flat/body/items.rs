use super::MaterialRef;

use std::fs::File;

impl Contents{
    fn open(descriptor:Descriptor,table:File,mut get:impl FnMut(Material)->io::Result<MaterialRef>)->io::Result<Self>{
        Self::bind(descriptor,HeldBytes::separate_format_fixture(table)?,|m|get(m).map(Located::change))
    }
    fn range_to(&self,at:u64,n:u64,budget:ReadBudget,mut get:impl FnMut(MaterialRef)->io::Result<(File,File)>,out:impl Write)->io::Result<()>{
        self.read_with(at,n,budget,0,|v|{
            let(a,b)=get(v.reference())?;Ok((HeldBytes::separate_format_fixture(a)?,HeldBytes::separate_format_fixture(b)?))
        },||Ok(()),out)
    }
}
