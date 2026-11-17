//! The overlay never substitutes a sparse file for its immutable historical base.
use crate::file_draft::manifest::{ManifestError, WorkingManifest};
use data_serialization::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serialization(deny_unknown_fields)]
pub struct IncrementalManifest {
    material: WorkingManifest,
    #[serialization(with = "crate::u64_string")]
    retained_base_bytes: u64,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct WireIncremental {
    material: WorkingManifest,
    #[serialization(with = "crate::u64_string")]
    retained_base_bytes: u64,
}

impl<'de> Deserialize<'de> for IncrementalManifest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireIncremental::deserialize(deserializer)?;
        Self::new(wire.material, wire.retained_base_bytes)
            .map_err(data_serialization::de::Error::custom)
    }
}

impl IncrementalManifest {
    pub fn new(material: WorkingManifest, retained_base_bytes: u64) -> Result<Self, ManifestError> {
        if retained_base_bytes > material.logical_size() {
            return Err(ManifestError::Predecessor);
        }
        Ok(Self {
            material,
            retained_base_bytes,
        })
    }

    pub fn material(&self) -> &WorkingManifest {
        &self.material
    }
    pub const fn retained_base_bytes(&self) -> u64 {
        self.retained_base_bytes
    }

    pub fn validate(&self, base_bytes: u64, maximum: u64) -> Result<(), ManifestError> {
        self.material.validate(maximum)?;
        if self.retained_base_bytes > base_bytes
            || self.retained_base_bytes > self.material.logical_size()
        {
            return Err(ManifestError::Predecessor);
        }
        Ok(())
    }

    pub fn require_previous(
        &self,
        previous: Option<&Self>,
        base_bytes: u64,
        maximum: u64,
    ) -> Result<(), ManifestError> {
        self.validate(base_bytes, maximum)?;
        let retained = match previous {
            Some(previous) => {
                previous.validate(base_bytes, maximum)?;
                self.material
                    .require_successor_of(&previous.material, maximum)?;
                previous.retained_base_bytes
            }
            None => {
                if self.material.generation()
                    != crate::file_draft::manifest::SaveGeneration::first()
                    || self.material.previous()
                        != (crate::file_draft::manifest::ManifestPrevious::Birth {})
                {
                    return Err(ManifestError::Predecessor);
                }
                base_bytes
            }
        };
        let expected = retained.min(self.material.retained_prefix().unwrap_or(retained));
        if self.retained_base_bytes != expected {
            return Err(ManifestError::Predecessor);
        }
        Ok(())
    }
}
