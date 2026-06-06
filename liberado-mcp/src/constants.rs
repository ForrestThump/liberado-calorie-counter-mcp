// ─── DB connection retry ──────────────────────────────────────────────────────

/// Initial delay before the first DB connection retry, in seconds.
pub const DB_RETRY_INITIAL_DELAY_SECS: u64 = 2;

/// Maximum delay between DB connection retries, in seconds.
pub const DB_RETRY_MAX_DELAY_SECS: u64 = 30;

/// Exponential backoff multiplier applied to the delay after each failed attempt.
pub const DB_RETRY_BACKOFF_FACTOR: u32 = 2;

/// How long to keep retrying DB connection before giving up, in seconds.
/// Overridable at runtime via LIBERADO_DB_CONNECT_TIMEOUT_SECS.
pub const DB_CONNECT_TIMEOUT_SECS: u64 = 120;

// ─── HTTP client ──────────────────────────────────────────────────────────────

/// Timeout for outbound HTTP requests to USDA and Open Food Facts, in seconds.
pub const DEFAULT_HTTP_CLIENT_TIMEOUT_SECS: u64 = 30;

// ─── Search behaviour ─────────────────────────────────────────────────────────

/// Hard cap on the number of results returned by search_food.
pub const DEFAULT_SEARCH_MAX_RESULTS_HARD_LIMIT: u32 = 10;

/// Minimum pg_trgm similarity score for a result to be returned from the local cache.
pub const DEFAULT_LOCAL_SEARCH_MIN_SCORE: f32 = 0.15;

/// Minimum trigram similarity score to accept a result from the USDA API.
pub const DEFAULT_USDA_MATCH_MIN_SCORE: f32 = 0.10;

/// Minimum trigram similarity score to accept a result from Open Food Facts.
pub const DEFAULT_OFF_MATCH_MIN_SCORE: f32 = 0.10;

/// Number of results to request per page from the USDA FoodData Central API.
pub const DEFAULT_USDA_API_PAGE_SIZE: u32 = 5;

/// Number of results to request per page from the Open Food Facts API.
pub const DEFAULT_OFF_API_PAGE_SIZE: u32 = 5;

// ─── Log listing ─────────────────────────────────────────────────────────────

/// Default number of entries returned by list_recent_logs when no limit is supplied.
pub const DEFAULT_LOG_LIST_LIMIT: u32 = 20;

/// Hard cap on the number of entries returned by list_recent_logs.
pub const DEFAULT_LOG_LIST_MAX_LIMIT: u32 = 100;
