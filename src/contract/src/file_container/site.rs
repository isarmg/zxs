//! One registered location for this FILE. These values are descriptive. A
//! valid transition additionally needs the original native publication and
//! independently held FILE/DISK/SYSTEM roles; parsing is never a move permit.
use super::retained::{RetainedAllocation,RetainedKind};
use crate::working_tree::MaterialCommitment;
use crate::{FileId,Generation,OperationId};
use data_serialization::{Deserialize,Serialize};

#[derive(Clone,Debug,Eq,PartialEq,Serialize,Deserialize)]
#[serialization(tag="state", rename_all="snake_case", deny_unknown_fields)]
pub enum FileSite {
    /// At its *original registration*. For an instance this may already be
    /// the Worktree's birth address, not an ordinary common-data occurrence.
    Created {},
    Retained { generation:Generation, hide:OperationId, allocation:RetainedAllocation },
    Restored { generation:Generation, restore:OperationId, hide:OperationId },
}

#[derive(Clone,Copy,Debug,Eq,PartialEq,Serialize,Deserialize)]
#[serialization(rename_all="snake_case")]
pub enum SitePurpose { Created, Retained, Restored }

/// FILE-private row evidence contains no old path, retained spelling, disk
/// address or management access. Its source registration is verified separately.
#[derive(Clone,Copy,Debug,Eq,PartialEq,Serialize,Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct FileSiteWitness {
    pub purpose:SitePurpose,
    pub generation:Generation,
    pub registration:MaterialCommitment,
}

#[derive(Debug,error_definition::Error)]
pub enum FileSiteError {
    #[error("FILE lifecycle prior position or original operation differs")]
    Source,
    #[error("FILE retained allocation is not the original file name and disk number")]
    Allocation,
    #[error("FILE lifecycle generation exhausted: {0}")]
    Exhausted(#[source] crate::GenerationExhausted),
}

impl FileSite {
    pub const fn generation(&self)->Generation {
        match self {Self::Created{}=>Generation::new(1),Self::Retained{generation,..}
            | Self::Restored{generation,..}=>*generation}
    }
    pub const fn purpose(&self)->SitePurpose {
        match self {Self::Created{}=>SitePurpose::Created,Self::Retained{..}=>SitePurpose::Retained,
            Self::Restored{..}=>SitePurpose::Restored}
    }
    /// Check shape only. No "positive" read/write capability is returned.
    pub fn valid_shape(&self)->bool {
        match self {
            Self::Created{}=>true,
            Self::Retained{generation,allocation,..}=>generation.get()>=2 && generation.get()%2==0
                && allocation.name.kind()==RetainedKind::File && allocation.name.number()==allocation.registration.number,
            Self::Restored{generation,hide,restore}=>generation.get()>=3 && generation.get()%2==1 && hide!=restore,
        }
    }
    pub fn hide(&self, operation:OperationId, allocation:RetainedAllocation, original:&str)
        ->std::result::Result<Self,FileSiteError> {
        if !self.valid_shape() || matches!(self,Self::Retained{..})
            || matches!(self,Self::Restored{hide,restore,..} if *hide==operation || *restore==operation)
        {return Err(FileSiteError::Source)}
        if allocation.name.original_name()!=original || allocation.name.kind()!=RetainedKind::File
            || allocation.registration.number!=allocation.name.number() {return Err(FileSiteError::Allocation)}
        Ok(Self::Retained{generation:self.generation().next().map_err(FileSiteError::Exhausted)?,hide:operation,allocation})
    }
    pub fn restore(&self, expected_hide:OperationId, restore:OperationId)->std::result::Result<Self,FileSiteError> {
        let Self::Retained{hide,..}=self else{return Err(FileSiteError::Source)};
        if !self.valid_shape() || *hide!=expected_hide || *hide==restore {return Err(FileSiteError::Source)}
        Ok(Self::Restored{generation:self.generation().next().map_err(FileSiteError::Exhausted)?,restore,hide:*hide})
    }
    pub fn witness(&self,file:FileId)->crate::Result<FileSiteWitness> {
        if !self.valid_shape() {
            return Err(crate::Error::new(crate::ErrorCode::ContainerLayoutInvalid,"invalid registered FILE site"));
        }
        let bytes=crate::canonical_json(&("registered-file-site",file,self))?;
        Ok(FileSiteWitness{purpose:self.purpose(),generation:self.generation(),
            registration:MaterialCommitment::from_digest(blake3::hash(&bytes))})
    }
}

#[cfg(test)]
#[path="../../../../test/contract/unit/file_site_tests.rs"]
mod tests;
