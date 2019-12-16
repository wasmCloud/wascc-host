use std::error::Error as StdError;
use std::fmt;

#[derive(Debug)]
pub struct Error(Box<ErrorKind>);

pub fn new(kind: ErrorKind) -> Error {
    Error(Box::new(kind))
}

#[derive(Debug)]
pub enum ErrorKind {
    Wapc(wapc::errors::Error),
    HostCallFailure(Box<dyn StdError>),
    Encoding(prost::EncodeError),
    Decoding(prost::DecodeError),
    Wascap(wascap::Error),
    Authorization(String),
    IO(std::io::Error),
    CapabilityProvider(String),
    MiscHost(String),
}

impl Error {
    pub fn kind(&self) -> &ErrorKind {
        &self.0
    }

    pub fn into_kind(self) -> ErrorKind {
        *self.0
    }
}

impl StdError for Error {
    fn description(&self) -> &str {
        match *self.0 {
            ErrorKind::Wapc(_) => "waPC error",
            ErrorKind::IO(_) => "I/O error",
            ErrorKind::HostCallFailure(_) => "Error occurred during host call",
            ErrorKind::Encoding(_) => "Encoding failure",
            ErrorKind::Decoding(_) => "Decoding failure",
            ErrorKind::Wascap(_) => "Embedded JWT Failure",
            ErrorKind::Authorization(_) => "Module authorization failure",
            ErrorKind::CapabilityProvider(_) => "Capability provider failure",
            ErrorKind::MiscHost(_) => "waSCC Host error",
        }
    }

    fn cause(&self) -> Option<&dyn StdError> {
        match *self.0 {
            ErrorKind::Wapc(ref err) => Some(err),
            ErrorKind::HostCallFailure(_) => None,
            ErrorKind::Encoding(ref err) => Some(err),
            ErrorKind::Decoding(ref err) => Some(err),
            ErrorKind::Wascap(ref err) => Some(err),
            ErrorKind::Authorization(_) => None,
            ErrorKind::IO(ref err) => Some(err),
            ErrorKind::CapabilityProvider(_) => None,
            ErrorKind::MiscHost(_) => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self.0 {
            ErrorKind::Wapc(ref err) => write!(f, "waPC failure: {}", err),
            ErrorKind::HostCallFailure(ref err) => {
                write!(f, "Error occurred during host call: {}", err)
            }
            ErrorKind::Encoding(ref err) => write!(f, "Encoding failure: {}", err),
            ErrorKind::Decoding(ref err) => write!(f, "Decoding failure: {}", err),
            ErrorKind::Wascap(ref err) => write!(f, "Embedded JWT failure: {}", err),
            ErrorKind::Authorization(ref err) => {
                write!(f, "WebAssembly module authorization failure: {}", err)
            }
            ErrorKind::IO(ref err) => write!(f, "I/O error: {}", err),
            ErrorKind::CapabilityProvider(ref err) => {
                write!(f, "Capability provider error: {}", err)
            }
            ErrorKind::MiscHost(ref err) => write!(f, "waSCC Host Error: {}", err),
        }
    }
}

impl From<wascap::Error> for Error {
    fn from(source: wascap::Error) -> Error {
        Error(Box::new(ErrorKind::Wascap(source)))
    }
}

impl From<wapc::errors::Error> for Error {
    fn from(source: wapc::errors::Error) -> Error {
        Error(Box::new(ErrorKind::Wapc(source)))
    }
}

impl From<prost::EncodeError> for Error {
    fn from(source: prost::EncodeError) -> Error {
        Error(Box::new(ErrorKind::Encoding(source)))
    }
}

impl From<prost::DecodeError> for Error {
    fn from(source: prost::DecodeError) -> Error {
        Error(Box::new(ErrorKind::Decoding(source)))
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Error {
        Error(Box::new(ErrorKind::IO(source)))
    }
}
