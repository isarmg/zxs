//! Private bijection over the already captured object domains. It never rewrites
//! arbitrary strings, source operations, actors, paths or external provenance.
use super::super::catalog::Domain;
use super::*;

pub(in crate::foreign_import::identity_closure) struct Remap<'a> {
    objects: &'a BTreeMap<Domain, BTreeMap<String, String>>,
}
impl<'a> Remap<'a> {
    pub(in crate::foreign_import::identity_closure) fn new(
        objects: &'a BTreeMap<Domain, BTreeMap<String, String>>,
    ) -> Self {
        Self { objects }
    }
    pub(super) fn id(&self, domain: Domain, id: &str) -> Result<String> {
        self.objects.get(&domain).and_then(|map| map.get(id)).cloned()
            .ok_or_else(|| invalid("foreign typed graph references an identity absent from its exact naming closure"))
    }
    pub(super) fn replace(&self, domain: Domain, id: &mut String) -> Result<()> {
        *id = self.id(domain, id)?;
        Ok(())
    }
    pub(super) fn optional(&self, domain: Domain, id: &mut Option<String>) -> Result<()> {
        if let Some(id) = id {
            self.replace(domain, id)?;
        }
        Ok(())
    }
    pub(super) fn ids(&self, domain: Domain, ids: &mut [String]) -> Result<()> {
        for id in ids {
            self.replace(domain, id)?;
        }
        Ok(())
    }
    pub(super) fn directory(&self, id: &str) -> Result<String> {
        if id.starts_with("repo_") {
            self.id(Domain::Repository, id)
        } else {
            self.id(Domain::Entry, id)
        }
    }
    pub(super) fn node(&self, id: &str) -> Result<String> {
        let domain = if id.starts_with("ver_") {
            Domain::Version
        } else if id.starts_with("fnode_") {
            Domain::PathNode
        } else if id.starts_with("fmeta_") {
            Domain::MetadataNode
        } else if id.starts_with("fdel_") {
            Domain::DeletionNode
        } else {
            return Err(invalid(
                "foreign typed graph has an unsupported file node domain",
            ));
        };
        self.id(domain, id)
    }
    pub(super) fn nodes(&self, values: &mut [String]) -> Result<()> {
        for id in values {
            *id = self.node(id)?;
        }
        Ok(())
    }
    pub(super) fn pairs(
        &self,
        values: &mut BTreeMap<String, String>,
        key: Domain,
        value: Domain,
    ) -> Result<()> {
        *values = values
            .iter()
            .map(|(k, v)| Ok((self.id(key, k)?, self.id(value, v)?)))
            .collect::<Result<_>>()?;
        Ok(())
    }
    pub(super) fn keys<T>(
        &self,
        values: BTreeMap<String, T>,
        domain: Domain,
    ) -> Result<BTreeMap<String, T>> {
        values
            .into_iter()
            .map(|(k, v)| Ok((self.id(domain, &k)?, v)))
            .collect()
    }
    pub(super) fn directories<T>(
        &self,
        values: BTreeMap<String, T>,
    ) -> Result<BTreeMap<String, T>> {
        values
            .into_iter()
            .map(|(k, v)| Ok((self.directory(&k)?, v)))
            .collect()
    }
    pub(super) fn contexts(
        &self,
        contexts: &mut BTreeMap<String, storage_contract::FileContext>,
    ) -> Result<()> {
        for context in contexts.values_mut() {
            self.context(context)?;
        }
        Ok(())
    }
    pub(super) fn references(
        &self,
        values: &mut BTreeMap<String, storage_contract::DirectoryReference>,
    ) -> Result<()> {
        for value in values.values_mut() {
            self.reference(value)?;
        }
        *values = self.directories(std::mem::take(values))?;
        Ok(())
    }
    pub(super) fn entries(&self, entries: &mut [storage_contract::Entry]) -> Result<()> {
        for entry in entries {
            self.entry(entry)?;
        }
        Ok(())
    }
}
