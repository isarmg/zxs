//! One bounded root-to-leaf path, authenticated by the aggregate map reference.
use super::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct InclusionProof {
    nodes: Vec<Value>,
}
impl InclusionProof {
    pub(crate) fn verify(&self, root: &Reference, key: &str) -> Result<Option<Value>> {
        if self.nodes.is_empty() || self.nodes.len() > RADIX_DEPTH + 1 {
            return Err(invalid("invalid metadata inclusion proof length"));
        }
        let mut nodes = self.nodes.iter();
        let value = resolve_path(
            root,
            key,
            |kind, hash| {
                let value = nodes
                    .next()
                    .ok_or_else(|| invalid("metadata inclusion proof is truncated"))?;
                AuthenticatedMetadataObject::authenticate(kind, hash, value.clone())
            },
            |_| {},
        )?;
        if nodes.next().is_some() {
            return Err(invalid("metadata inclusion proof has unused nodes"));
        }
        Ok(value)
    }
}
pub(crate) fn read_proof(
    root: &Reference,
    key: &str,
    load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<(Option<Value>, InclusionProof)> {
    let mut nodes = Vec::new();
    let value = resolve_path(root, key, load, |object| nodes.push(object.value().clone()))?;
    Ok((value, InclusionProof { nodes }))
}
pub(crate) fn lookup(
    root: &Reference,
    key: &str,
    load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<Option<Value>> {
    resolve_path(root, key, load, |_| {})
}
fn resolve_path(
    root: &Reference,
    key: &str,
    mut load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
    mut observe: impl FnMut(&AuthenticatedMetadataObject),
) -> Result<Option<Value>> {
    root.validate()?;
    let route = key_route(key);
    let mut reference = root.clone();
    let mut prefix = String::new();
    loop {
        let object = load(reference.kind.domain(), reference.hash())?;
        observe(&object);
        match reference.kind {
            ReferenceKind::Page => {
                PageShape::read(&object)?.validate_at(&prefix)?;
                return Ok(object
                    .value()
                    .as_object()
                    .expect("invariant: PageShape admits map")
                    .get(key)
                    .cloned());
            }
            ReferenceKind::Index => {
                let index = decode_index(&object)?;
                if index.prefix != prefix {
                    return Err(invalid("metadata inclusion proof routing prefix differs"));
                }
                let digit = &route[prefix.len()..prefix.len() + 1];
                let Some(child) = index.children.get(digit) else {
                    return Ok(None);
                };
                reference = child.clone();
                prefix.push_str(digit);
            }
        }
    }
}
