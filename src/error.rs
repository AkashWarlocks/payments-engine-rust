use thiserror::Error;

/// Unified error type wrapping the two external failure sources.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
}

/// Convenience `Result` alias used throughout the crate.
pub type Result<T> = std::result::Result<T, AppError>;
