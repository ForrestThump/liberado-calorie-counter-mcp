use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("hashing error: {0}")]
    Hashing(String),

    #[error("external API error: {0}")]
    ExternalApi(String),

    #[error("estimation unavailable: {0}")]
    EstimationUnavailable(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
