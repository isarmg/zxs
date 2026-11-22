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
mod tests {
    use super::*;

    const SHARE: &str = "11111111111111111111111111111111";
    const DIR: &str = "22222222222222222222222222222222";
    const CURSOR: &str = "33333333333333333333333333333333";

    #[test]
    fn typed_route_round_trip_and_wire_size() {
        let hash = format!("{DIRECTORY}{SHARE}/{DIR}/{CURSOR}");
        let Location::Directory(folder) = Location::parse(&hash) else { panic!("directory") };
        assert_eq!(folder.fragment(), hash);
        assert_eq!(Location::Directory(folder).selected(), Some(Panel::Folders));
        assert_eq!(Panel::Folders.id(), "zxs-files");
        assert_eq!(folder.request_path(), format!("/ui/v1/d/{SHARE}/{DIR}/{CURSOR}"));
        assert_eq!(folder.request_path().len(), 107);
        assert_eq!(folder.destination(None).fragment(), format!("{DIRECTORY}{SHARE}/-/-"));
    }

    #[test]
    fn no_guessed_ids_paths_or_percent_decoding() {
        assert_eq!(Location::parse("#zxs-versions"), Location::Panel(Panel::Versions));
        assert_eq!(Location::parse("#not-a-panel"), Location::Other);
        assert_eq!(Location::parse(""), Location::Landing);
        for bad in [
            format!("{DIRECTORY}{SHARE}/../-"),
            format!("{DIRECTORY}{SHARE}/%2d/-"),
            format!("{DIRECTORY}{SHARE}/{DIR}/-/-"),
            format!("{DIRECTORY}{SHARE}/{DIR}/?after={CURSOR}"),
            format!("{DIRECTORY}00000000000000000000000000000000/-/-"),
            format!("{DIRECTORY}Aa111111111111111111111111111111/-/-"),
            format!("{DIRECTORY}/{DIR}/-"),
        ] {
            assert_eq!(Location::parse(&bad), Location::InvalidDirectory, "{bad}");
        }
    }
}
