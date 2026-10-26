use std::fmt;

/// Failure to decode or print a core WebAssembly binary.
#[derive(Debug)]
pub enum Error {
    /// The binary reader rejected malformed input.
    Parse(wasmparser::BinaryReaderError),
    /// A code section's declared size does not match its contents.
    InvalidCodeSectionSize,
    /// A memory page size exponent cannot fit in a 64-bit size.
    InvalidPageSize(u32),
    /// Operators ended without the required terminator.
    UnexpectedEndOfOperators,
    /// Component encoding is outside this core printer's scope.
    UnsupportedComponent,
    /// The section identifier is not supported.
    UnknownSection(u8),
    /// The parser supplied a payload the core printer does not support.
    UnknownPayload,
    /// The dylink.0 subsection identifier is not supported.
    UnknownDylinkSubsection(u8),
    /// The function count exceeds the printer's allocation limit.
    TooManyFunctions(u32),
    /// A function exceeds the printer's local variable limit.
    TooManyLocals,
    /// Branch hints describe more functions than the printer permits.
    TooManyBranchHintFunctions,
    /// A function has more branch hints than the printer permits.
    TooManyBranchHints,
    /// A type index exceeds the representation used for printed heap types.
    TypeIndexTooLarge,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(error) => error.fmt(f),
            Self::InvalidCodeSectionSize => f.write_str("invalid code section size"),
            Self::InvalidPageSize(_) => f.write_str("invalid page size"),
            Self::UnexpectedEndOfOperators => f.write_str("unexpected end of operators"),
            Self::UnsupportedComponent => {
                f.write_str("component binaries are outside the core text printer scope")
            }
            Self::UnknownSection(id) => write!(f, "found unknown section `{id}`"),
            Self::UnknownPayload => f.write_str("found unknown payload"),
            Self::UnknownDylinkSubsection(id) => {
                write!(f, "don't know how to print dylink.0 subsection id {id}")
            }
            Self::TooManyFunctions(count) => write!(
                f,
                "module contains {count} functions which exceeds the limit of {}",
                crate::MAX_WASM_FUNCTIONS
            ),
            Self::TooManyLocals => {
                f.write_str("function exceeds the maximum number of locals that can be printed")
            }
            Self::TooManyBranchHintFunctions | Self::TooManyBranchHints => {
                f.write_str("found too many hints")
            }
            Self::TypeIndexTooLarge => f.write_str("implementation limit: type index too large"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(error) => Some(error),
            Self::InvalidCodeSectionSize
            | Self::InvalidPageSize(_)
            | Self::UnexpectedEndOfOperators
            | Self::UnsupportedComponent
            | Self::UnknownSection(_)
            | Self::UnknownPayload
            | Self::UnknownDylinkSubsection(_)
            | Self::TooManyFunctions(_)
            | Self::TooManyLocals
            | Self::TooManyBranchHintFunctions
            | Self::TooManyBranchHints
            | Self::TypeIndexTooLarge => None,
        }
    }
}

impl From<wasmparser::BinaryReaderError> for Error {
    fn from(error: wasmparser::BinaryReaderError) -> Self {
        Self::Parse(error)
    }
}
