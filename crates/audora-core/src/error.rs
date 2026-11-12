//! Stable application error taxonomy (`AV-*`) and redacted diagnostic payloads.
//!
//! Maps internal persistence, filesystem, validation, and playback failures into user-safe
//! error payloads defined in `docs/ERROR_CATALOG.md` without leaking note bodies or raw paths.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable user-facing error codes defined in `docs/ERROR_CATALOG.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// `AV-101`: Media file is missing or on a disconnected volume.
    #[serde(rename = "AV-101")]
    MediaUnavailable,
    /// `AV-102`: File cannot be read due to permissions or I/O failure.
    #[serde(rename = "AV-102")]
    MediaUnreadable,
    /// `AV-103`: Media stream is corrupt or truncated.
    #[serde(rename = "AV-103")]
    MediaCorrupt,
    /// `AV-104`: Container or codec is unsupported by available decoders.
    #[serde(rename = "AV-104")]
    MediaUnsupported,
    /// `AV-201`: Selected relink path already belongs to another library record.
    #[serde(rename = "AV-201")]
    RelinkConflict,
    /// `AV-202`: Selected relink file differs in duration or media kind and requires confirmation.
    #[serde(rename = "AV-202")]
    RelinkMismatch,
    /// `AV-301`: Input payload failed domain or boundary validation.
    #[serde(rename = "AV-301")]
    ValidationFailed,
    /// `AV-401`: SQLite connection, transaction, or schema migration failure.
    #[serde(rename = "AV-401")]
    PersistenceFailure,
    /// `AV-501`: Unexpected internal error.
    #[serde(rename = "AV-501")]
    InternalError,
}

impl ErrorCode {
    /// Returns the canonical string representation (for example `"AV-104"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MediaUnavailable => "AV-101",
            Self::MediaUnreadable => "AV-102",
            Self::MediaCorrupt => "AV-103",
            Self::MediaUnsupported => "AV-104",
            Self::RelinkConflict => "AV-201",
            Self::RelinkMismatch => "AV-202",
            Self::ValidationFailed => "AV-301",
            Self::PersistenceFailure => "AV-401",
            Self::InternalError => "AV-501",
        }
    }

    /// Returns the error category name.
    #[must_use]
    pub const fn category(self) -> &'static str {
        match self {
            Self::MediaUnavailable => "MediaUnavailable",
            Self::MediaUnreadable => "MediaUnreadable",
            Self::MediaCorrupt => "MediaCorrupt",
            Self::MediaUnsupported => "MediaUnsupported",
            Self::RelinkConflict => "RelinkConflict",
            Self::RelinkMismatch => "RelinkMismatch",
            Self::ValidationFailed => "ValidationFailed",
            Self::PersistenceFailure => "PersistenceFailure",
            Self::InternalError => "InternalError",
        }
    }
}

/// Serializable error payload returned across the IPC boundary to the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppErrorPayload {
    pub code: ErrorCode,
    pub category: String,
    pub title: String,
    pub message: String,
    pub recovery_steps: Vec<String>,
    pub recoverable: bool,
}

/// Application error type used across services, adapters, and repositories.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("Media unavailable ({code:?}): {message}")]
    MediaUnavailable { code: ErrorCode, message: String },

    #[error("Media unreadable ({code:?}): {message}")]
    MediaUnreadable { code: ErrorCode, message: String },

    #[error("Media corrupt ({code:?}): {message}")]
    MediaCorrupt { code: ErrorCode, message: String },

    #[error("Media format unsupported ({code:?}): {file_name} ({detail})")]
    MediaUnsupported {
        code: ErrorCode,
        file_name: String,
        detail: String,
    },

    #[error("Relink conflict ({code:?}): {message}")]
    RelinkConflict { code: ErrorCode, message: String },

    #[error("Relink mismatch ({code:?}): {message}")]
    RelinkMismatch { code: ErrorCode, message: String },

    #[error("Validation failed ({code:?}): {message}")]
    Validation { code: ErrorCode, message: String },

    #[error("Persistence failure ({code:?}): {message}")]
    Persistence { code: ErrorCode, message: String },

    #[error("Internal error ({code:?}): {message}")]
    Internal { code: ErrorCode, message: String },
}

impl AppError {
    #[must_use]
    pub fn media_unavailable(message: impl Into<String>) -> Self {
        Self::MediaUnavailable {
            code: ErrorCode::MediaUnavailable,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn media_unreadable(message: impl Into<String>) -> Self {
        Self::MediaUnreadable {
            code: ErrorCode::MediaUnreadable,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn media_corrupt(message: impl Into<String>) -> Self {
        Self::MediaCorrupt {
            code: ErrorCode::MediaCorrupt,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn media_unsupported(file_name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::MediaUnsupported {
            code: ErrorCode::MediaUnsupported,
            file_name: file_name.into(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn relink_conflict(message: impl Into<String>) -> Self {
        Self::RelinkConflict {
            code: ErrorCode::RelinkConflict,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn relink_mismatch(message: impl Into<String>) -> Self {
        Self::RelinkMismatch {
            code: ErrorCode::RelinkMismatch,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            code: ErrorCode::ValidationFailed,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn persistence(message: impl Into<String>) -> Self {
        Self::Persistence {
            code: ErrorCode::PersistenceFailure,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            code: ErrorCode::InternalError,
            message: message.into(),
        }
    }

    /// Returns the stable `ErrorCode` associated with this error.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::MediaUnavailable { code, .. }
            | Self::MediaUnreadable { code, .. }
            | Self::MediaCorrupt { code, .. }
            | Self::MediaUnsupported { code, .. }
            | Self::RelinkConflict { code, .. }
            | Self::RelinkMismatch { code, .. }
            | Self::Validation { code, .. }
            | Self::Persistence { code, .. }
            | Self::Internal { code, .. } => *code,
        }
    }

    /// Converts this error into a user-safe `AppErrorPayload` for the frontend UI.
    #[must_use]
    pub fn to_payload(&self) -> AppErrorPayload {
        let code = self.code();
        match self {
            Self::MediaUnavailable { message, .. } => AppErrorPayload {
                code,
                category: code.category().to_owned(),
                title: "We can’t find this file".to_owned(),
                message: message.clone(),
                recovery_steps: vec![
                    "Use Find file… to relink the moved or renamed file.".to_owned(),
                    "Your saved notes remain intact while the file is offline.".to_owned(),
                ],
                recoverable: true,
            },
            Self::MediaUnreadable { message, .. } => AppErrorPayload {
                code,
                category: code.category().to_owned(),
                title: "We can’t read this file".to_owned(),
                message: message.clone(),
                recovery_steps: vec![
                    "Check that the file exists and you have read permission.".to_owned(),
                    "Choose another local audio or video file.".to_owned(),
                ],
                recoverable: true,
            },
            Self::MediaCorrupt { message, .. } => AppErrorPayload {
                code,
                category: code.category().to_owned(),
                title: "This file appears damaged".to_owned(),
                message: message.clone(),
                recovery_steps: vec![
                    "Try opening another copy of the recording.".to_owned(),
                    "Verify the file plays in your system’s default media player.".to_owned(),
                ],
                recoverable: true,
            },
            Self::MediaUnsupported { file_name, .. } => AppErrorPayload {
                code,
                category: code.category().to_owned(),
                title: "This format isn’t supported".to_owned(),
                message: format!(
                    "Audora couldn’t decode {file_name}. The file uses a codec that isn’t available on this system."
                ),
                recovery_steps: vec![
                    "Convert the file to MP4 (H.264) or WebM.".to_owned(),
                    "Try opening it with your system’s default player.".to_owned(),
                ],
                recoverable: true,
            },
            Self::RelinkConflict { message, .. } => AppErrorPayload {
                code,
                category: code.category().to_owned(),
                title: "File already in library".to_owned(),
                message: message.clone(),
                recovery_steps: vec![
                    "Select the relocated copy that corresponds to this library record.".to_owned(),
                ],
                recoverable: true,
            },
            Self::RelinkMismatch { message, .. } => AppErrorPayload {
                code,
                category: code.category().to_owned(),
                title: "Confirm file relink".to_owned(),
                message: message.clone(),
                recovery_steps: vec![
                    "Confirm if you intentionally replaced this file with an edited version."
                        .to_owned(),
                ],
                recoverable: true,
            },
            Self::Validation { message, .. } => AppErrorPayload {
                code,
                category: code.category().to_owned(),
                title: "Invalid input".to_owned(),
                message: message.clone(),
                recovery_steps: vec![
                    "Review the highlighted field and try again.".to_owned(),
                ],
                recoverable: true,
            },
            Self::Persistence { message, .. } => AppErrorPayload {
                code,
                category: code.category().to_owned(),
                title: "Database recovery needed".to_owned(),
                message: message.clone(),
                recovery_steps: vec![
                    "Audora preserves a pre-migration backup in your application data directory."
                        .to_owned(),
                ],
                recoverable: false,
            },
            Self::Internal { message, .. } => AppErrorPayload {
                code,
                category: code.category().to_owned(),
                title: "Something went wrong".to_owned(),
                message: message.clone(),
                recovery_steps: vec!["Try the action again.".to_owned()],
                recoverable: true,
            },
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        tracing::error!(
            code = ErrorCode::PersistenceFailure.as_str(),
            "SQLite error: {err}"
        );
        Self::persistence(
            "Local database operation failed; existing committed notes are preserved.",
        )
    }
}

/// Redacts a filesystem path for safe diagnostic logging (retaining only the extension).
#[must_use]
pub fn redact_path(path: &Path) -> String {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if !ext.is_empty() => format!("<redacted-path>.{ext}"),
        _ => "<redacted-path>".to_owned(),
    }
}

pub type AppResult<T> = Result<T, AppError>;
