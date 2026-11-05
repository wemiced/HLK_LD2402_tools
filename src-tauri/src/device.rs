//! Device-level errors shared by the transport and the session layer.

#[derive(thiserror::Error, Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DeviceError {
    #[error("permission denied opening {path}")]
    PermissionDenied { path: String },
    #[error("serial port {path} not found")]
    NotFound { path: String },
    #[error("no answer to command {word:#06x}")]
    Timeout { word: u16 },
    #[error("command {word:#06x} rejected with status {status}")]
    Nack { word: u16, status: u16 },
    #[error("{feature} needs firmware {required} or newer")]
    Unsupported { feature: String, required: String },
    #[error("device disconnected")]
    Disconnected,
    #[error("device is busy")]
    Busy,
    #[error("I/O error: {message}")]
    Io { message: String },
}
