#![allow(dead_code)]

use std::fmt;

#[derive(Debug, Clone)]
pub enum RhiError {
    UnsupportedBackend,
    BackendError(String),
    Shader(String),

    Unimplemented,
}

impl fmt::Display for RhiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RhiError::UnsupportedBackend => write!(f, "Unsupported backend"),
            RhiError::BackendError(s) => write!(f, "Backend error: {}", s),
            RhiError::Shader(msg) => write!(f, "Shader error: {}", msg),

            RhiError::Unimplemented => write!(f, "Unimplemented"),
        }
    }
}

pub type RhiResult<T> = Result<T, RhiError>;
