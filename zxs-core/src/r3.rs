//! The currently deployed, frozen R3 wire subset. No session/upload/Range invention.
use crate::{Result, hex, invalid};
use http::{Method, Request, StatusCode};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::fmt;

pub(crate) const ROOT: &str = "/zxs/v1/containers";
pub(crate) const MAX_JSON: u64 = 1 << 20;

fn exact_hex(s: &str, width: usize) -> bool {
    s.len() == width
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

macro_rules! literal {
    ($name:ident, $validate:expr) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn parse(raw: &str) -> Result<Self> {
                Self::try_from(raw.to_owned()).map_err(invalid)
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = &'static str;
            fn try_from(raw: String) -> std::result::Result<Self, Self::Error> {
                if ($validate)(&raw) {
                    Ok(Self(raw))
                } else {
                    Err("invalid R3 literal")
                }
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

fn id(raw: &str) -> bool {
    exact_hex(raw, 32) && raw.bytes().any(|b| b != b'0')
}

literal!(Id, id);
literal!(Branch, |raw: &str| raw == "main" || id(raw));
literal!(Digest, |raw: &str| exact_hex(raw, 64));
literal!(DisplayName, |raw: &str| !raw.is_empty()
    && raw.len() <= 255
    && raw != "."
    && raw != ".."
    && !raw.ends_with([' ', '.'])
    && !raw
        .chars()
        .any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c)));

impl Id {
    pub(crate) fn fresh() -> Result<Self> {
        let mut bytes = [0; 16];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| invalid("could not obtain a new original ID"))?;
        Self::parse(&hex(&bytes))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub length: u64,
    pub sha256: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    FirstMain { name: DisplayName },
    Branch { branch: Branch, expected_head: Id },
    HistoricalMain { branch: Id, from_node: Id },
}

impl Target {
    pub fn expected_base(&self) -> Option<&Id> {
        match self {
            Self::FirstMain { .. } => None,
            Self::Branch { expected_head, .. } => Some(expected_head),
            Self::HistoricalMain { from_node, .. } => Some(from_node),
        }
    }

    pub fn branch(&self) -> &str {
        match self {
            Self::FirstMain { .. } => "main",
            Self::Branch { branch, .. } => branch.as_str(),
            Self::HistoricalMain { branch, .. } => branch.as_str(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub container_id: Id,
    pub operation_id: Id,
    pub node_id: Id,
    pub seen_sequence: u64,
    pub target: Target,
    pub body: Body,
}

impl Intent {
    pub fn validate(&self) -> Result<()> {
        if self.seen_sequence == u64::MAX
            || (self.seen_sequence == 0) != self.target.expected_base().is_none()
            || self.target.expected_base() == Some(&self.node_id)
        {
            return Err(invalid(
                "not an exact original first/head/historical SaveNode",
            ));
        }
        Ok(())
    }

    pub(crate) fn lookup(&self) -> Result<Request<()>> {
        self.validate()?;
        json_request(format!(
            "{ROOT}/{}/operations/{}",
            self.container_id, self.operation_id
        ))
    }

    pub(crate) fn save(&self) -> Result<Request<()>> {
        self.validate()?;
        let mut sent = Request::builder()
            .method(Method::POST)
            .uri(format!(
                "{ROOT}/{}/branches/{}/nodes",
                self.container_id,
                self.target.branch()
            ))
            .header("accept", "application/json")
            .header("accept-encoding", "identity")
            .header("content-type", "application/octet-stream")
            .header("content-length", self.body.length.to_string())
            .header("zxs-operation", self.operation_id.as_str())
            .header("zxs-node", self.node_id.as_str())
            .header("zxs-sha256", self.body.sha256.as_str());
        sent = match &self.target {
            Target::FirstMain { name } => sent
                .header("zxs-base-node", "0")
                .header("zxs-initial-name", hex(name.as_str().as_bytes())),
            Target::Branch { expected_head, .. } => {
                sent.header("zxs-base-node", expected_head.as_str())
            }
            Target::HistoricalMain { from_node, .. } => {
                sent.header("zxs-fork-from-main", from_node.as_str())
            }
        };
        Ok(sent.body(())?)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub node_id: Id,
    pub operation_id: Id,
    pub parent_node: Option<Id>,
    pub original_branch: Branch,
    pub sequence: u64,
    pub length: u64,
    pub sha256: Digest,
}

impl Node {
    pub fn body(&self) -> Body {
        Body {
            length: self.length,
            sha256: self.sha256.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Container {
    pub container_id: Id,
    pub name: DisplayName,
    pub sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub items: Vec<Container>,
    pub next_after: Option<Id>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Head {
    pub branch: Branch,
    pub head: Id,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Detail {
    pub container_id: Id,
    pub name: DisplayName,
    pub sequence: u64,
    pub tags: Vec<String>,
    pub heads: Vec<Head>,
    pub next_after: Option<Branch>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct History {
    pub head: Id,
    pub nodes: Vec<Node>,
    pub next: Option<Id>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lookup {
    Confirmed { sequence: u64 },
    Absent,
    Conflict,
    Unknown,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SaveKind {
    SaveNode,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    kind: SaveKind,
    container_id: Id,
    operation_id: Id,
    sequence: u64,
    node_id: Id,
    length: u64,
    sha256: Digest,
    target: Target,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Missing {
    state: MissingState,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MissingState {
    NotFound,
}

pub(crate) fn outcome(intent: &Intent, status: StatusCode, bytes: &[u8], readback: bool) -> Lookup {
    if intent.validate().is_err() || bytes.len() as u64 > MAX_JSON {
        return Lookup::Unknown;
    }
    match status {
        StatusCode::CONFLICT => Lookup::Conflict,
        StatusCode::NOT_FOUND if readback => {
            if matches!(
                serde_json::from_slice::<Missing>(bytes),
                Ok(Missing {
                    state: MissingState::NotFound
                })
            ) {
                Lookup::Absent
            } else {
                Lookup::Unknown
            }
        }
        StatusCode::OK => {
            let Ok(r) = serde_json::from_slice::<Receipt>(bytes) else {
                return Lookup::Unknown;
            };
            if r.kind == SaveKind::SaveNode
                && r.container_id == intent.container_id
                && r.operation_id == intent.operation_id
                && r.node_id == intent.node_id
                && r.target == intent.target
                && r.length == intent.body.length
                && r.sha256 == intent.body.sha256
                && r.sequence > intent.seen_sequence
                && r.sequence < u64::MAX
                && (!matches!(&intent.target, Target::FirstMain { .. }) || r.sequence == 1)
            {
                Lookup::Confirmed {
                    sequence: r.sequence,
                }
            } else {
                Lookup::Unknown
            }
        }
        _ => Lookup::Unknown,
    }
}

pub(crate) fn json_request(path: String) -> Result<Request<()>> {
    Ok(Request::builder()
        .uri(path)
        .header("accept", "application/json")
        .header("accept-encoding", "identity")
        .body(())?)
}

#[cfg(test)]
#[path = "../tests/unit/r3_cases.rs"]
mod tests;
