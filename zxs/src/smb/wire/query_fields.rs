//! Request views for MS-SMB2 §§2.2.33, 2.2.37. In particular, neither a
//! related FileId nor a directory search index is an open or a storage key.
use super::{bytes, field, io_fields::{IoFieldError,OpenReference},message::Message, u16le, u32le};

const DIRECTORY_FIXED:usize=96;
const INFO_FIXED:usize=104;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum QueryError {
    WrongCommand,
    Malformed,
    ExceedsMaxTransact,
    InvalidInfoClass,
    NotSupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum DirectoryClass {
    Directory,
    Full,
    Both,
    Names,
    IdBoth,
    IdFull,
}

impl DirectoryClass {
    fn select(value: u8) -> Result<Self, QueryError> {
        use DirectoryClass::{Directory, Full, Both, Names, IdBoth, IdFull};
        match value {
            1 => Ok(Directory),
            2 => Ok(Full),
            3 => Ok(Both),
            12 => Ok(Names),
            37 => Ok(IdBoth),
            38 => Ok(IdFull),
            // Other directory formats listed in MS-SMB2 §3.3.5.18.
            60 | 78..=81 => Err(QueryError::NotSupported),
            _ => Err(QueryError::InvalidInfoClass),
        }
    }

    /// Offset of FileName in the selected MS-FSCC record, not sizeof a
    /// native C structure (which could include alignment or a name slot).
    pub(in crate::smb) fn fixed_bytes(self) -> usize {
        match self {
            Self::Directory => 64,
            Self::Full => 68,
            Self::Both => 94,
            Self::Names => 12,
            Self::IdBoth => 104,
            Self::IdFull => 80,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum Scan {
    Continue,
    Restart,
    Reopen,
}

#[derive(Debug, Eq, PartialEq)]
pub(in crate::smb) struct DirectoryQuery<'a> {
    pub(in crate::smb) class: DirectoryClass,
    pub(in crate::smb) scan: Scan,
    pub(in crate::smb) single: bool,
    pub(in crate::smb) wire_id: [u8; 16],
    /// The original OutputBufferLength, also the chargeable size (§3.3.5.18).
    pub(in crate::smb) output_bytes: u32,
    pattern: &'a [u8],
}

impl<'a> DirectoryQuery<'a> {
    /// Even-length within this command, possibly empty. Name decoding and
    /// scan state belong to the directory open; these bytes create neither.
    pub(in crate::smb) fn pattern(&self) -> &'a [u8] {
        self.pattern
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum FileClass {
    Basic,
    Standard,
    Internal,
    Access,
    All,
    NetworkOpen,
    AttributeTag,
    NormalizedName,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum FsClass {
    Volume,
    Size,
    Device,
    Attribute,
    FullSize,
    SectorSize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum InfoClass {
    File(FileClass),
    Filesystem(FsClass),
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum CapacityFormat{Size,FullSize}
impl CapacityFormat{
    pub(in crate::smb) fn bytes(self)->u32{match self{Self::Size=>24,Self::FullSize=>32}}
}

impl InfoClass {
    pub(in crate::smb) fn capacity_format(self)->Option<CapacityFormat>{
        match self{
            Self::Filesystem(FsClass::Size)=>Some(CapacityFormat::Size),
            Self::Filesystem(FsClass::FullSize)=>Some(CapacityFormat::FullSize),
            _=>None,
        }
    }

    fn select(kind: u8, value: u8) -> Result<Self, QueryError> {
        use QueryError::{InvalidInfoClass, Malformed, NotSupported};
        match kind {
            1 => Ok(Self::File(match value {
                4 => FileClass::Basic,
                5 => FileClass::Standard,
                6 => FileClass::Internal,
                8 => FileClass::Access,
                18 => FileClass::All,
                34 => FileClass::NetworkOpen,
                35 => FileClass::AttributeTag,
                48 => FileClass::NormalizedName,
                // Documented MS-FSCC §2.4 classes outside this parser's
                // profile; 0x64 is the reserved MS-SMB2 FILE class.
                1..=3 | 7 | 9..=17 | 19..=29 | 31..=33 | 36..=40 | 44..=46 |
                50 | 54 | 59 | 60 | 64 | 65 | 78..=81 | 0x64 => return Err(NotSupported),
                _ => return Err(InvalidInfoClass),
            })),
            2 => Ok(Self::Filesystem(match value {
                1 => FsClass::Volume,
                3 => FsClass::Size,
                4 => FsClass::Device,
                5 => FsClass::Attribute,
                7 => FsClass::FullSize,
                11 => FsClass::SectorSize,
                2 | 6 | 8..=10 => return Err(NotSupported),
                _ => return Err(InvalidInfoClass),
            })),
            3 | 4 => Err(NotSupported), // Security and quota are not read-mount metadata.
            _ => Err(Malformed),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(in crate::smb) struct InfoQuery {
    pub(in crate::smb) class: InfoClass,
    pub(in crate::smb) wire_id: [u8; 16],
    pub(in crate::smb) output_bytes: u32,
    /// Max(InputBufferLength, OutputBufferLength) as sent, before any server
    /// response budgeting. The connection validates credits independently.
    pub(in crate::smb) credit_bytes: u32,
}

fn check_output(request: u32, max_transact: u32) -> Result<u32, QueryError> {
    (request <= max_transact).then_some(request).ok_or(QueryError::ExceedsMaxTransact)
}

impl<'a> Message<'a> {
    /// A minimally bounded ID for Session→Tree→OPEN lookup before any
    /// query clauses are interpreted or sent to an object.
    pub(in crate::smb) fn directory_target(&self)->Result<OpenReference,IoFieldError>{
        self.directory_fixed().map_err(|e|match e{
            QueryError::WrongCommand=>IoFieldError::WrongCommand,_=>IoFieldError::Malformed,
        })?;
        self.io_open_reference(72)
    }
    fn directory_fixed(&self)->Result<(),QueryError>{
        use QueryError::{Malformed,WrongCommand};
        if self.header.command!=0x000e{return Err(WrongCommand);}
        if self.raw.len()<DIRECTORY_FIXED||u16le(self.raw,64).map_err(|_|Malformed)?!=33{return Err(Malformed);}
        Ok(())
    }
    pub(in crate::smb) fn directory_output(&self,max_transact:u32)->Result<u32,QueryError>{
        self.directory_fixed()?;
        check_output(u32le(self.raw,92).map_err(|_|QueryError::Malformed)?,max_transact)
    }
    /// The maximum is the connection's actual negotiated MaxTransactSize.
    pub(in crate::smb) fn query_directory(&self, max_transact: u32) -> Result<DirectoryQuery<'a>, QueryError> {
        use QueryError::Malformed;
        const FIXED:usize=DIRECTORY_FIXED;
        const RESTART: u8 = 0x01;
        const SINGLE: u8 = 0x02;
        const INDEX: u8 = 0x04;
        const REOPEN: u8 = 0x10;
        let output_bytes=self.directory_output(max_transact)?;
        let p = self.raw;
        let flags = p[67];
        if flags & !(RESTART | SINGLE | INDEX | REOPEN) != 0 {
            return Err(Malformed);
        }
        let at = usize::from(u16le(p, 88).map_err(|_| Malformed)?);
        let n = usize::from(u16le(p, 90).map_err(|_| Malformed)?);
        let pattern = if n == 0 {
            // MS-SMB2 sends offset zero for an absent pattern. Apple's native
            // client also sends offset 96 and a two-byte zero placeholder
            // (source linked in ARCHITECTURE.md); use neither as an offset.
            if at != 0 && !(at == FIXED && bytes(p, FIXED, 2).ok() == Some(&[0, 0][..])) {
                return Err(Malformed);
            }
            &p[0..0]
        } else {
            if at < FIXED || n % 2 != 0 {
                return Err(Malformed);
            }
            bytes(p, at, n).map_err(|_| Malformed)?
        };
        // Index resumption is optional in §3.3.5.18. This profile does not
        // use the 32-bit wire position, even when the request sets INDEX.
        let scan = if flags & REOPEN != 0 {
            Scan::Reopen
        } else if flags & RESTART != 0 {
            Scan::Restart
        } else {
            Scan::Continue
        };
        Ok(DirectoryQuery {
            class: DirectoryClass::select(p[66])?,
            scan,
            single: flags & SINGLE != 0,
            wire_id: field(p, 72).map_err(|_| Malformed)?,
            output_bytes,
            pattern,
        })
    }

    /// Original bounded lookup coordinate only. The queried info class
    /// cannot bypass the original Session -> Tree -> OPEN route.
    pub(in crate::smb) fn query_info_target(&self)->Result<OpenReference,IoFieldError>{
        self.query_info_fixed().map_err(|e|match e{
            QueryError::WrongCommand=>IoFieldError::WrongCommand,_=>IoFieldError::Malformed,
        })?;
        self.io_open_reference(88)
    }
    fn query_info_fixed(&self)->Result<(),QueryError>{
        use QueryError::{Malformed, WrongCommand};
        if self.header.command != 0x0010 {
            return Err(WrongCommand);
        }
        if self.raw.len() < INFO_FIXED || u16le(self.raw, 64).map_err(|_| Malformed)? != 41 {
            return Err(Malformed);
        }
        Ok(())
    }
    pub(in crate::smb) fn query_info(&self, max_transact: u32) -> Result<InfoQuery, QueryError> {
        use QueryError::Malformed;
        self.query_info_fixed()?;
        let p = self.raw;
        let class = InfoClass::select(p[66], p[67])?;
        let output_bytes = check_output(u32le(p, 68).map_err(|_| Malformed)?, max_transact)?;
        // For these classes there is no meaningful input window, flags or
        // AdditionalInformation. Do not dereference or copy any guessed
        // input. §3.3.5.20 still uses the raw length in multi-credit checks.
        let input_bytes = u32le(p, 76).map_err(|_| Malformed)?;
        Ok(InfoQuery {
            class,
            wire_id: field(p, 88).map_err(|_| Malformed)?,
            output_bytes,
            credit_bytes: input_bytes.max(output_bytes),
        })
    }
}
