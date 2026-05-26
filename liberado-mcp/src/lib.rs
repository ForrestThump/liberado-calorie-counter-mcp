pub mod config;
pub mod error;
pub mod food;
pub mod server;
pub mod units;

use turbomcp::prelude::{McpError, McpResult};

/// Extension trait that converts any `Display` error into an `McpError::internal`.
/// Import this in modules that call fallible operations and need `McpResult`.
pub(crate) trait McpResultExt<T> {
    fn mcp_err(self) -> McpResult<T>;
}

impl<T, E: std::fmt::Display> McpResultExt<T> for Result<T, E> {
    fn mcp_err(self) -> McpResult<T> {
        self.map_err(|e| McpError::internal(e.to_string()))
    }
}
