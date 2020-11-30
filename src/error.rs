use std::{
    error,
    fmt::{self, Formatter},
    io,
    path::PathBuf,
    result,
};

use crate::common::{self, Version};

pub type Result<T> = result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    // TODO: use struct when enum have multi same type
    MetaMissing(PathBuf),
    Incompatible(Version),
    FileExisted(PathBuf),
    EntryMismatch(u128, u128),
    MeetIncompleteEntry(u128, u128),
    WriteOnReadOnlyFile(PathBuf),
    External(ExternalError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MetaMissing(path) => write!(f, "header of file missing: {:?}", path),
            Self::Incompatible(file_version) => write!(
                f,
                "incompatible version, major version should be equal: current={}, got={}",
                common::CURRENT_VERSION,
                file_version
            ),
            Self::EntryMismatch(expect, actual) => write!(
                f,
                "sequence of entry mismatch: expect={}, actual={}",
                expect, actual
            ),
            Self::FileExisted(path) => write!(f, "file already existed: {:?}", path),
            Self::MeetIncompleteEntry(expect, actual) => write!(
                f,
                "meet incomplete entry: expect {} frames, got {}",
                expect, actual
            ),
            Self::WriteOnReadOnlyFile(path) => {
                write!(f, "write entry in read-only file: {:?}", path)
            }
            Self::External(err) => err.fmt(f),
        }
    }
}

impl error::Error for Error {}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::External(ExternalError::from(err))
    }
}

#[derive(Debug, Clone)]
pub enum ExternalError {
    Os(i32),
    IO(io::ErrorKind, String),
}

impl error::Error for ExternalError {}

impl From<io::Error> for ExternalError {
    fn from(err: io::Error) -> Self {
        match err.raw_os_error() {
            Some(code) => Self::Os(code),
            None => Self::IO(err.kind(), err.to_string()),
        }
    }
}

impl fmt::Display for ExternalError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Os(code) => io::Error::from_raw_os_error(code.to_owned()).fmt(f),
            Self::IO(kind, error) => io::Error::new(kind.to_owned(), error.to_owned()).fmt(f),
        }
    }
}

impl PartialEq for ExternalError {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}
