//! Byte/depth budget for the final and temporary names of *one* FILE. No
//! arbitrary client path, permission, capacity reservation or I/O is returned.
use super::{ContainerLeaf,FileName,TicketLeaf,TICKETS};
use crate::{FileId,OperationId,fs::{NameBudget,PathBudget}};
use std::io;

pub(in crate::ordinary) struct ResidentNames{terminal:ContainerLeaf,ticket:OperationId}
impl ResidentNames{
    pub(in crate::ordinary) fn for_file(display:FileName<'_>,file:FileId,ticket:OperationId)->io::Result<Self>{
        TicketLeaf::isolation(ticket)?;
        Ok(Self{terminal:display.container(file)?,ticket})
    }
    pub(in crate::ordinary) fn terminal(&self)->&ContainerLeaf{&self.terminal}
    /// The physical parent plus the exact same native names that a held
    /// writer will check. The returned budget is for the final container.
    pub(in crate::ordinary) fn at(&self,parent:NameBudget)->io::Result<PathBudget>{
        let terminal=parent.child(self.terminal.name())?;
        for i in 0..TICKETS{parent.child(&TicketLeaf::stage(self.ticket,i)?.name())?;}
        parent.child(&TicketLeaf::isolation(self.ticket)?.name())?;
        Ok(terminal)
    }
    /// New birth (and future business rename) must fit the original R's
    /// recorded geometry as well. No opening of R and no future delete
    /// operation/space is implied: the private grammar has fixed widths.
    pub(in crate::ordinary) fn new_spelling(&self,ordinary:NameBudget,original_recycle:NameBudget)->io::Result<PathBudget>{
        let container=self.at(ordinary)?;self.at(original_recycle)?;Ok(container)
    }
}

#[cfg(test)]
#[path="../../../tests/unit/storage_name_budget.rs"]
mod tests;
