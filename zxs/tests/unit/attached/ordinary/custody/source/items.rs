impl Clean {
    /// Read the actual current source of a new/pending paired handoff. No
    /// source fallback when R is unavailable, and no opening A just because
    /// a later B-native FILE/INCR was first created there. A completed
    /// decision must use target observation and exact retirement instead.
    pub(super) fn capture_lifecycle(system:&Path,r:Request)->io::Result<Self>{Self::capture_checked(system,r,|_|Ok(()))}
}
