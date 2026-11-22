use crate::label_text::{Label, MAX};
use std::fmt;

pub(crate) const ROOTS: &str = "#zxs-files";
const DIRECTORY: &str = "#zxs-d/";
// Two ids and a continuation leave room for one hex label in the fragment,
// not in the HTTP request target.
pub(crate) const MAX_FRAGMENT: usize = DIRECTORY.len() + 3 * 32 + 3 + 2 * MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Id([u8; 32]);

impl Id {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let bytes: [u8; 32] = value.as_bytes().try_into().ok()?;
        (bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            && bytes.iter().any(|b| *b != b'0'))
        .then_some(Self(bytes))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(std::str::from_utf8(&self.0).expect("ASCII identifier"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Folder {
    pub(crate) share: Id,
    pub(crate) parent: Option<Id>,
    pub(crate) page: Option<Id>,
    pub(crate) filter: Option<Label>,
}

impl Folder {
    pub(crate) fn destination(&self, parent: Option<Id>) -> Self {
        Self { parent, page: None, ..self.clone() }
    }

    fn path(&self, prefix: &str) -> String {
        let segment = |id: Option<Id>| id.map_or_else(|| "-".into(), |id| id.to_string());
        format!("{prefix}{}/{}/{}", self.share, segment(self.parent), segment(self.page))
    }

    pub(crate) fn fragment(&self) -> String {
        let mut path = self.path(DIRECTORY);
        if let Some(name) = &self.filter {
            path.push('/');
            path.push_str(&name.encoded());
        }
        path
    }

    pub(crate) fn form(&self, csrf: &str) -> Option<String> {
        let label = self.filter.as_ref()?.encoded();
        let parent = self.parent.map_or_else(|| "-".into(), |id| id.to_string());
        let after = self.page.map_or_else(|| "-".into(), |id| id.to_string());
        Some(format!("csrf={csrf}&share={}&parent={parent}&after={after}&label={label}", self.share))
    }

    pub(crate) fn filtered(&self, filter: Option<Label>) -> Self {
        Self { filter, page: None, ..self.clone() }
    }

    pub(crate) fn request_path(&self) -> String {
        self.path("/ui/v1/d/")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Panel {
    Folders,
    Shares,
    Versions,
    Account,
}

impl Panel {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Folders => "zxs-files",
            Self::Shares => "zxs-shares",
            Self::Versions => "zxs-versions",
            Self::Account => "zxs-credentials",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Location {
    Landing,
    Panel(Panel),
    Directory(Folder),
    Other,
    InvalidDirectory,
}

impl Location {
    pub(crate) fn parse(hash: &str) -> Self {
        let at = match hash {
            "" => return Self::Landing,
            ROOTS => Some(Panel::Folders),
            "#zxs-shares" => Some(Panel::Shares),
            "#zxs-versions" => Some(Panel::Versions),
            "#zxs-credentials" => Some(Panel::Account),
            _ => None,
        };
        if let Some(panel) = at {
            return Self::Panel(panel);
        }
        let Some(raw) = hash.strip_prefix(DIRECTORY) else {
            return if hash == "#zxs-d" { Self::InvalidDirectory } else { Self::Other };
        };
        let mut segments = raw.split('/');
        let folder = (|| {
            let share = Id::parse(segments.next()?)?;
            let segment = |part: &str| if part == "-" { Some(None) } else { Id::parse(part).map(Some) };
            let parent = segment(segments.next()?)?;
            let page = segment(segments.next()?)?;
            let filter = match segments.next() {
                None => None,
                Some(label) => Some(Label::from_hex(label)?),
            };
            segments.next().is_none().then_some(Folder { share, parent, page, filter })
        })();
        folder.map_or(Self::InvalidDirectory, Self::Directory)
    }

    pub(crate) fn selected(&self) -> Option<Panel> {
        match self {
            Self::Panel(panel) => Some(*panel),
            Self::Directory(_) | Self::InvalidDirectory => Some(Panel::Folders),
            Self::Landing | Self::Other => None,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/browser/route.rs"]
mod tests;
