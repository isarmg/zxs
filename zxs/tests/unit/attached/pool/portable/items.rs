impl Genesis {
    /// This reader understands exactly an empty register, including EOF.
    /// No speculative/unknown suffix can masquerade as the confirmed cut.
    fn only(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_GENESIS { return Err(invalid("portable GENESIS exceeds its format limit")); }
        let (genesis,tail)=Self::prefix(bytes)?;
        if !tail.is_empty(){return Err(invalid("not exactly the empty portable registration"));}
        Ok(genesis)
    }
}
