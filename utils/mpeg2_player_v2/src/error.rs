use std::fmt;
use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    EndOfStream,
    InvalidData(String),
    Unsupported(String),
    NeedSequenceHeader,
    DecodeNotImplemented(String),
}

impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidData(message.into())
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }

    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::DecodeNotImplemented(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::EndOfStream => f.write_str("unexpected end of MPEG bitstream"),
            Self::InvalidData(msg) => write!(f, "invalid MPEG bitstream: {msg}"),
            Self::Unsupported(msg) => write!(f, "unsupported MPEG-2 feature: {msg}"),
            Self::NeedSequenceHeader => f.write_str("stream did not contain a sequence header"),
            Self::DecodeNotImplemented(msg) => {
                write!(f, "MPEG-2 decode milestone not implemented yet: {msg}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
