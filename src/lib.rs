extern crate byteorder;
use byteorder::LittleEndian;
type Endian = LittleEndian;

#[macro_use]
extern crate log;

mod bytes_io;
mod common;
mod error;
mod frame;

#[cfg(test)]
mod tests;

// public lists for user
pub use self::bytes_io::BytesIO;
pub use self::common::{EntryID, EntryOffset};
pub use self::error::{Error, Result};
