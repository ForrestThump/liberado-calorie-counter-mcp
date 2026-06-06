/// Initial delay before the first DB connection retry, in seconds.
pub const DB_RETRY_INITIAL_DELAY_SECS: u64 = 2;

/// Maximum delay between DB connection retries, in seconds.
pub const DB_RETRY_MAX_DELAY_SECS: u64 = 30;

/// Exponential backoff multiplier applied to the delay after each failed attempt.
pub const DB_RETRY_BACKOFF_FACTOR: u32 = 2;

/// How long to keep retrying DB connection before giving up, in seconds.
/// Overridable at runtime via LIBERADO_DB_CONNECT_TIMEOUT_SECS.
pub const DB_CONNECT_TIMEOUT_SECS: u64 = 120;
