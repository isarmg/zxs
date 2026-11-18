use super::physical::HistoryBody;
use crate::ordinary::indexed::Indexed;
use std::io::{self,Write};
impl HistoryBody{
    pub(in crate::ordinary) fn write_range(&self,offset:u64,length:u64,out:impl Write)->io::Result<()>{
        let o=&self.original;let header=o.node_header();let link_start=header.len() as u64;
        Indexed{raw:&self.raw.file,index:&self.node.file,header,raw_start:0,link_start,
            claim:o.content,head:o.chain,digest:self.index_digest}.write_range(offset,length,out,||self.unchanged())
    }
}
