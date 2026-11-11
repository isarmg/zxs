//! Identity domains and their existing wire prefixes.
use crate::IdKind;

macro_rules! kinds {
    ($($kind:ident => $prefix:literal),+ $(,)?) => {
        $(
            pub enum $kind {}
            impl IdKind for $kind {
                const PREFIX: &'static str = $prefix;
            }
        )+
    };
}

kinds! {
    Operation => "op",
    Rule => "rule",
    Repository => "repo",
    HistoryTree => "history",
    Branch => "br",
    Commit => "commit",
    Tree => "tree",
    DirectoryTree => "dtree",
    DirectoryBranch => "dbr",
    Entry => "entry",
    File => "file",
    Version => "ver",
    Pool => "pool",
    Disk => "disk",
    FileAnchor => "fnode",
    FileMetadata => "fmeta",
    FileDeletion => "fdel",
    FileMerge => "fmerge",
}
