//! Repository labels use the full PortableName policy, not the shorter branch limit.
#[derive(Debug, PartialEq, Eq)]
pub struct RepositoryName {
    display: String,
    key: String,
}
impl RepositoryName {
    pub fn parse(name: &str) -> crate::Result<Self> {
        name_policy::validate_name(name)?;
        Ok(Self {
            display: name.into(),
            key: name_policy::lookup_key(name),
        })
    }
    pub fn as_str(&self) -> &str {
        &self.display
    }
    pub fn key(&self) -> &str {
        &self.key
    }
}
#[cfg(test)]
#[path = "../../../test/contract/unit/repository_name.rs"]
mod tests;
