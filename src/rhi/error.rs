#![allow(dead_code)]

use super::device::SkippedDevice;
use std::error::Error;

pub type RhiResult<T> = Result<T, RhiError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct ShaderDiagnostic {
    pub severity: DiagnosticSeverity,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ValidationMessage {
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub object_name: Option<String>,
}

/// Error data only; Display, Error, and conversion implementations are deferred.
#[derive(Debug)]
pub enum RhiError {
    UnsupportedBackend,
    DeviceSelection {
        skipped: Vec<SkippedDevice>,
    },
    Backend {
        context: &'static str,
        source: Option<Box<dyn Error + Send + Sync>>,
    },
    InvalidCommand {
        context: &'static str,
        detail: String,
    },
    InvalidSyncPoint {
        context: &'static str,
    },
    ResourceState {
        context: &'static str,
        detail: String,
    },
    ResourceBusy {
        resource: String,
    },
    Shader {
        diagnostics: Vec<ShaderDiagnostic>,
    },
    Validation(String),
    DeviceLost,
    OutOfMemory,
    Unsupported {
        feature: &'static str,
    },
}
