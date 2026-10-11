//! Errors.

use thiserror::Error;

/// Everything that can go wrong reading, building or evaluating an ODD.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum Error {
    /// An OpenODD document (or a binding file) this reader cannot make an ODD of.
    #[error("{0}")]
    OpenOdd(String),
    /// An ODD, module, attribute or cover item built with arguments that do not make one.
    #[error("{0}")]
    Value(String),
    /// A unit that is unknown, or that cannot be converted into another.
    #[error("{0}")]
    Unit(String),
    /// A git source could not be fetched, or names a file it does not have.
    #[error("{0}")]
    GitSource(String),
    /// Reading or writing a file.
    #[error("{0}")]
    Io(String),
}

impl Error {
    pub fn open_odd(message: impl Into<String>) -> Self {
        Error::OpenOdd(message.into())
    }
    pub fn value(message: impl Into<String>) -> Self {
        Error::Value(message.into())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
