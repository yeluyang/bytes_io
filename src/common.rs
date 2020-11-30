use std::{
    convert::{TryFrom, TryInto},
    fmt::{self, Display, Formatter},
    io::Cursor,
    mem, result,
};

extern crate byteorder;
use byteorder::{ReadBytesExt, WriteBytesExt};

use crate::{
    error::{Error, Result},
    Endian,
};

// protocol version of BytesIO file
pub const VERSION_MAJOR: u128 = 1;
pub const VERSION_MINOR: u128 = 0;
pub const VERSION_PATCH: u128 = 0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Version {
    major: u128,
    minor: u128,
    patch: u128,
}
pub const VERSION_BYTES: usize = mem::size_of::<Version>();
pub const CURRENT_VERSION: Version = Version::new();

impl Version {
    pub const fn new() -> Version {
        return Version {
            major: VERSION_MAJOR,
            minor: VERSION_MINOR,
            patch: VERSION_PATCH,
        };
    }

    pub fn is_compatible(&self) -> bool {
        self.major == VERSION_MAJOR
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        bytes.write_u128::<Endian>(self.major)?;
        bytes.write_u128::<Endian>(self.minor)?;
        bytes.write_u128::<Endian>(self.patch)?;

        assert_eq!(bytes.len(), VERSION_BYTES);

        Ok(bytes)
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl TryFrom<&[u8]> for Version {
    type Error = Error;
    fn try_from(bytes: &[u8]) -> result::Result<Self, Self::Error> {
        assert_eq!(bytes.len(), VERSION_BYTES);

        let mut rdr = Cursor::new(Vec::from(bytes));
        let major = rdr.read_u128::<Endian>()?;
        let minor = rdr.read_u128::<Endian>()?;
        let patch = rdr.read_u128::<Endian>()?;

        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl TryFrom<Vec<u8>> for Version {
    type Error = Error;
    fn try_from(bytes: Vec<u8>) -> result::Result<Self, Self::Error> {
        Self::try_from(bytes.as_slice())
    }
}

impl TryInto<Vec<u8>> for Version {
    type Error = Error;
    fn try_into(self) -> result::Result<Vec<u8>, Self::Error> {
        self.to_bytes()
    }
}

#[derive(Debug)]
pub struct EntryID {
    pub file_id: u128,
    pub entry_seq: u128,
}

#[derive(Debug)]
pub struct EntryOffset {
    pub entry_id: EntryID,
    pub first_frame: usize,
}

impl EntryOffset {
    pub fn new(file_id: u128, entry_seq: u128, first_frame: usize) -> EntryOffset {
        return EntryOffset {
            entry_id: EntryID { file_id, entry_seq },
            first_frame,
        };
    }
}
