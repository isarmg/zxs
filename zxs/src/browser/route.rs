use std::fmt;

pub(crate) const ROOTS: &str = "#zxs-files";
const DIRECTORY: &str = "#zxs-d/";

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Folder {
    pub(crate) share: Id,
    pub(crate) parent: Option<Id>,
    pub(crate) page: Option<Id>,
}

impl Folder {
    pub(crate) fn destination(self, parent: Option<Id>) -> Self {
        Self { parent, page: None, ..self }
    }

    fn path(self, prefix: &str) -> String {
        let segment = |id: Option<Id>| id.map_or_else(|| "-".into(), |id| id.to_string());
        format!("{prefix}{}/{}/{}", self.share, segment(self.parent), segment(self.page))
    }

    pub(crate) fn fragment(self) -> String {
        self.path(DIRECTORY)
    }

    pub(crate) fn request_path(self) -> String {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
            segments.next().is_none().then_some(Folder { share, parent, page })
        })();
        folder.map_or(Self::InvalidDirectory, Self::Directory)
    }

    pub(crate) fn selected(self) -> Option<Panel> {
        match self {
            Self::Panel(panel) => Some(panel),
            Self::Directory(_) | Self::InvalidDirectory => Some(Panel::Folders),
            Self::Landing | Self::Other => None,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/browser/route.rs"]
mod tests;
