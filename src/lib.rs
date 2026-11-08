//! Offline exFAT volume reader and fragmentation analysis.
//!
//! On-disk layouts follow Microsoft’s *exFAT File System Specification*
//! revision 1.00.

mod analysis;
mod defrag;
mod structs;
mod volume;

pub use analysis::{FragmentedEntry, analyze, analyze_filtered};
pub use defrag::{DefragOptions, DefragStats, defrag};
pub use structs::*;
pub use volume::Volume;

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Invalid(_) => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Error::Io(value)
    }
}
