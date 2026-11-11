//! Repository labels use the full PortableName policy, not the shorter branch limit.
#[derive(Debug, PartialEq, Eq)]
pub struct RepositoryName {
    display: name_policy::Name,
    key: String,
}
impl RepositoryName {
    pub fn parse(name: &str) -> crate::Result<Self> {
        let display = name_policy::Name::parse(name)?;
        let key = display.key();
        Ok(Self { display, key })
    }
    pub fn as_str(&self) -> &str {
        self.display.as_str()
    }
    pub fn key(&self) -> &str {
        &self.key
    }
}
#[cfg(test)]
#[path = "../../../test/contract/unit/repository_name.rs"]
mod tests;
