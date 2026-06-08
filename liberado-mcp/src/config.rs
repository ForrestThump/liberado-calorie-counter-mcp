use crate::constants::{
    DEFAULT_HTTP_CLIENT_TIMEOUT_SECS, DEFAULT_LOCAL_SEARCH_MIN_SCORE,
    DEFAULT_LOG_LIST_LIMIT, DEFAULT_LOG_LIST_MAX_LIMIT, DEFAULT_OFF_API_PAGE_SIZE,
    DEFAULT_OFF_MATCH_MIN_SCORE, DEFAULT_SEARCH_MAX_RESULTS_HARD_LIMIT,
    DEFAULT_USDA_API_PAGE_SIZE, DEFAULT_USDA_MATCH_MIN_SCORE,
};

// ─── Transport ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
pub enum TransportConfig {
    #[default]
    Stdio,
    Http { host: String, port: u16 },
}

// ─── ServerConfig ─────────────────────────────────────────────────────────────

/// Full runtime configuration. Built by `ServerConfig::load()`:
/// hardcoded defaults → YAML file → environment variables (highest priority).
#[derive(Debug, Clone)]
pub struct ServerConfig {
    // ── Database ──────────────────────────────────────────────────────────────
    pub database_url: String,
    pub db_max_connections: u32,

    // ── Transport ─────────────────────────────────────────────────────────────
    pub transport: TransportConfig,

    // ── Auth / API keys ───────────────────────────────────────────────────────
    /// API key for the default user. In stdio mode this seeds the DB on first
    /// boot. In HTTP mode it is used as a fallback when no api_key is supplied.
    pub default_api_key: String,
    pub usda_api_key: String,

    // ── Estimator ─────────────────────────────────────────────────────────────
    pub estimator_provider: String,
    pub estimator_model: String,
    pub estimator_api_key: String,
    pub estimator_base_url: String,

    // ── HTTP client ───────────────────────────────────────────────────────────
    pub http_client_timeout_secs: u64,

    // ── Search behaviour ──────────────────────────────────────────────────────
    pub search_strong_match_threshold: f64,
    pub search_max_weak_results: u32,
    pub search_max_results_hard_limit: u32,
    pub local_search_min_score: f32,
    pub usda_match_min_score: f32,
    pub off_match_min_score: f32,
    pub usda_api_page_size: u32,
    pub off_api_page_size: u32,

    // ── Log listing ───────────────────────────────────────────────────────────
    pub log_list_default_limit: u32,
    pub log_list_max_limit: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            database_url: "postgresql://liberado:liberado@localhost:5432/liberado".to_string(),
            db_max_connections: 5,
            transport: TransportConfig::Stdio,
            default_api_key: String::new(),
            usda_api_key: String::new(),
            estimator_provider: "none".to_string(),
            estimator_model: "claude-opus-4-8".to_string(),
            estimator_api_key: String::new(),
            estimator_base_url: "http://localhost:11434".to_string(),
            http_client_timeout_secs: DEFAULT_HTTP_CLIENT_TIMEOUT_SECS,
            search_strong_match_threshold: 0.6,
            search_max_weak_results: 3,
            search_max_results_hard_limit: DEFAULT_SEARCH_MAX_RESULTS_HARD_LIMIT,
            local_search_min_score: DEFAULT_LOCAL_SEARCH_MIN_SCORE,
            usda_match_min_score: DEFAULT_USDA_MATCH_MIN_SCORE,
            off_match_min_score: DEFAULT_OFF_MATCH_MIN_SCORE,
            usda_api_page_size: DEFAULT_USDA_API_PAGE_SIZE,
            off_api_page_size: DEFAULT_OFF_API_PAGE_SIZE,
            log_list_default_limit: DEFAULT_LOG_LIST_LIMIT,
            log_list_max_limit: DEFAULT_LOG_LIST_MAX_LIMIT,
        }
    }
}

// ─── YAML layer ───────────────────────────────────────────────────────────────

/// Mirrors `ServerConfig` with all fields optional.
/// Deserialised from the YAML config file; absent keys leave the default unchanged.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct YamlConfig {
    database_url: Option<String>,
    db_max_connections: Option<u32>,
    transport: Option<String>,
    http_host: Option<String>,
    http_port: Option<u16>,
    default_api_key: Option<String>,
    usda_api_key: Option<String>,
    estimator_provider: Option<String>,
    estimator_model: Option<String>,
    estimator_api_key: Option<String>,
    estimator_base_url: Option<String>,
    http_client_timeout_secs: Option<u64>,
    search_strong_match_threshold: Option<f64>,
    search_max_weak_results: Option<u32>,
    search_max_results_hard_limit: Option<u32>,
    local_search_min_score: Option<f32>,
    usda_match_min_score: Option<f32>,
    off_match_min_score: Option<f32>,
    usda_api_page_size: Option<u32>,
    off_api_page_size: Option<u32>,
    log_list_default_limit: Option<u32>,
    log_list_max_limit: Option<u32>,
}

// ─── Loading ──────────────────────────────────────────────────────────────────

impl ServerConfig {
    /// Builds configuration using a three-layer merge:
    /// 1. Hardcoded defaults (`ServerConfig::default()`)
    /// 2. YAML file at `$LIBERADO_CONFIG` (or `./config.yaml` if the var is absent)
    /// 3. `LIBERADO_*` environment variables (highest priority)
    ///
    /// The YAML file is silently skipped if it does not exist.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        cfg.apply_yaml();
        cfg.apply_env();
        cfg
    }

    fn apply_yaml(&mut self) {
        let path = std::env::var("LIBERADO_CONFIG")
            .unwrap_or_else(|_| "./config.yaml".to_string());

        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!("could not read config file '{path}': {e}");
                return;
            }
        };

        let yaml: YamlConfig = match serde_yaml::from_str(&text) {
            Ok(y) => y,
            Err(e) => {
                tracing::warn!("could not parse config file '{path}': {e}");
                return;
            }
        };

        tracing::info!("loaded config from '{path}'");

        if let Some(v) = yaml.database_url               { self.database_url = v; }
        if let Some(v) = yaml.db_max_connections          { self.db_max_connections = v; }
        if let Some(v) = yaml.default_api_key             { self.default_api_key = v; }
        if let Some(v) = yaml.usda_api_key                { self.usda_api_key = v; }
        if let Some(v) = yaml.estimator_provider          { self.estimator_provider = v; }
        if let Some(v) = yaml.estimator_model             { self.estimator_model = v; }
        if let Some(v) = yaml.estimator_api_key           { self.estimator_api_key = v; }
        if let Some(v) = yaml.estimator_base_url          { self.estimator_base_url = v; }
        if let Some(v) = yaml.http_client_timeout_secs    { self.http_client_timeout_secs = v; }
        if let Some(v) = yaml.search_strong_match_threshold { self.search_strong_match_threshold = v; }
        if let Some(v) = yaml.search_max_weak_results     { self.search_max_weak_results = v; }
        if let Some(v) = yaml.search_max_results_hard_limit { self.search_max_results_hard_limit = v; }
        if let Some(v) = yaml.local_search_min_score      { self.local_search_min_score = v; }
        if let Some(v) = yaml.usda_match_min_score        { self.usda_match_min_score = v; }
        if let Some(v) = yaml.off_match_min_score         { self.off_match_min_score = v; }
        if let Some(v) = yaml.usda_api_page_size          { self.usda_api_page_size = v; }
        if let Some(v) = yaml.off_api_page_size           { self.off_api_page_size = v; }
        if let Some(v) = yaml.log_list_default_limit      { self.log_list_default_limit = v; }
        if let Some(v) = yaml.log_list_max_limit          { self.log_list_max_limit = v; }

        if let Some(t) = yaml.transport {
            self.apply_transport_str(
                &t,
                yaml.http_host.as_deref(),
                yaml.http_port,
            );
        }
    }

    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("LIBERADO_DATABASE_URL") {
            self.database_url = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_DB_MAX_CONNECTIONS")
            && let Ok(n) = v.parse() {
            self.db_max_connections = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_DEFAULT_API_KEY") {
            self.default_api_key = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_USDA_API_KEY") {
            self.usda_api_key = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_ESTIMATOR_PROVIDER") {
            self.estimator_provider = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_ESTIMATOR_MODEL") {
            self.estimator_model = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_ESTIMATOR_API_KEY") {
            self.estimator_api_key = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_ESTIMATOR_BASE_URL") {
            self.estimator_base_url = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_HTTP_CLIENT_TIMEOUT_SECS")
            && let Ok(n) = v.parse() {
            self.http_client_timeout_secs = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_SEARCH_STRONG_MATCH_THRESHOLD")
            && let Ok(n) = v.parse() {
            self.search_strong_match_threshold = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_SEARCH_MAX_WEAK_RESULTS")
            && let Ok(n) = v.parse() {
            self.search_max_weak_results = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_SEARCH_MAX_RESULTS_HARD_LIMIT")
            && let Ok(n) = v.parse() {
            self.search_max_results_hard_limit = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_LOCAL_SEARCH_MIN_SCORE")
            && let Ok(n) = v.parse() {
            self.local_search_min_score = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_USDA_MATCH_MIN_SCORE")
            && let Ok(n) = v.parse() {
            self.usda_match_min_score = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_OFF_MATCH_MIN_SCORE")
            && let Ok(n) = v.parse() {
            self.off_match_min_score = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_USDA_API_PAGE_SIZE")
            && let Ok(n) = v.parse() {
            self.usda_api_page_size = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_OFF_API_PAGE_SIZE")
            && let Ok(n) = v.parse() {
            self.off_api_page_size = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_LOG_LIST_DEFAULT_LIMIT")
            && let Ok(n) = v.parse() {
            self.log_list_default_limit = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_LOG_LIST_MAX_LIMIT")
            && let Ok(n) = v.parse() {
            self.log_list_max_limit = n;
        }

        if let Ok(v) = std::env::var("LIBERADO_TRANSPORT") {
            let host = std::env::var("LIBERADO_HTTP_HOST").ok();
            let port = std::env::var("LIBERADO_HTTP_PORT")
                .ok()
                .and_then(|p| p.parse().ok());
            self.apply_transport_str(&v, host.as_deref(), port);
        }
    }

    fn apply_transport_str(&mut self, transport: &str, host: Option<&str>, port: Option<u16>) {
        match transport.to_lowercase().as_str() {
            "http" => {
                let host = host.unwrap_or("0.0.0.0").to_string();
                let port = port.unwrap_or(8080);
                self.transport = TransportConfig::Http { host, port };
            }
            _ => {
                self.transport = TransportConfig::Stdio;
            }
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn default_transport_is_stdio() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.transport, TransportConfig::Stdio);
    }

    #[test]
    #[serial]
    fn from_env_defaults_when_vars_absent() {
        unsafe { std::env::remove_var("LIBERADO_TRANSPORT") };
        unsafe { std::env::remove_var("LIBERADO_DATABASE_URL") };
        unsafe { std::env::remove_var("LIBERADO_CONFIG") };
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.transport, TransportConfig::Stdio);
        assert!(cfg.database_url.contains("liberado"));
    }

    #[test]
    #[serial]
    fn from_env_http_transport() {
        unsafe { std::env::set_var("LIBERADO_TRANSPORT", "http") };
        unsafe { std::env::set_var("LIBERADO_HTTP_HOST", "127.0.0.1") };
        unsafe { std::env::set_var("LIBERADO_HTTP_PORT", "9090") };
        let cfg = ServerConfig::from_env();
        assert_eq!(
            cfg.transport,
            TransportConfig::Http {
                host: "127.0.0.1".to_string(),
                port: 9090,
            }
        );
        unsafe { std::env::remove_var("LIBERADO_TRANSPORT") };
        unsafe { std::env::remove_var("LIBERADO_HTTP_HOST") };
        unsafe { std::env::remove_var("LIBERADO_HTTP_PORT") };
    }

    #[test]
    #[serial]
    fn from_env_invalid_transport_falls_back_to_stdio() {
        unsafe { std::env::set_var("LIBERADO_TRANSPORT", "websocket") };
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.transport, TransportConfig::Stdio);
        unsafe { std::env::remove_var("LIBERADO_TRANSPORT") };
    }

    #[test]
    #[serial]
    fn from_env_http_transport_case_insensitive() {
        unsafe { std::env::set_var("LIBERADO_TRANSPORT", "HTTP") };
        unsafe { std::env::remove_var("LIBERADO_HTTP_HOST") };
        unsafe { std::env::remove_var("LIBERADO_HTTP_PORT") };
        let cfg = ServerConfig::from_env();
        assert_eq!(
            cfg.transport,
            TransportConfig::Http {
                host: "0.0.0.0".to_string(),
                port: 8080,
            }
        );
        unsafe { std::env::remove_var("LIBERADO_TRANSPORT") };
    }

    #[test]
    #[serial]
    fn from_env_invalid_port_uses_default() {
        unsafe { std::env::set_var("LIBERADO_TRANSPORT", "http") };
        unsafe { std::env::set_var("LIBERADO_HTTP_PORT", "not-a-number") };
        unsafe { std::env::remove_var("LIBERADO_HTTP_HOST") };
        let cfg = ServerConfig::from_env();
        assert_eq!(
            cfg.transport,
            TransportConfig::Http {
                host: "0.0.0.0".to_string(),
                port: 8080,
            }
        );
        unsafe { std::env::remove_var("LIBERADO_TRANSPORT") };
        unsafe { std::env::remove_var("LIBERADO_HTTP_PORT") };
    }

    #[test]
    #[serial]
    fn from_env_sets_database_url() {
        unsafe { std::env::set_var("LIBERADO_DATABASE_URL", "postgres://custom:pass@db/mydb") };
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.database_url, "postgres://custom:pass@db/mydb");
        unsafe { std::env::remove_var("LIBERADO_DATABASE_URL") };
    }

    #[test]
    #[serial]
    fn from_env_sets_db_max_connections() {
        unsafe { std::env::set_var("LIBERADO_DB_MAX_CONNECTIONS", "20") };
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.db_max_connections, 20);
        unsafe { std::env::remove_var("LIBERADO_DB_MAX_CONNECTIONS") };
    }

    #[test]
    #[serial]
    fn from_env_invalid_db_max_connections_uses_default() {
        unsafe { std::env::set_var("LIBERADO_DB_MAX_CONNECTIONS", "not-a-number") };
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.db_max_connections, 5);
        unsafe { std::env::remove_var("LIBERADO_DB_MAX_CONNECTIONS") };
    }

    #[test]
    #[serial]
    fn from_env_sets_api_keys() {
        unsafe { std::env::set_var("LIBERADO_DEFAULT_API_KEY", "my-default-key") };
        unsafe { std::env::set_var("LIBERADO_USDA_API_KEY", "my-usda-key") };
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.default_api_key, "my-default-key");
        assert_eq!(cfg.usda_api_key, "my-usda-key");
        unsafe { std::env::remove_var("LIBERADO_DEFAULT_API_KEY") };
        unsafe { std::env::remove_var("LIBERADO_USDA_API_KEY") };
    }

    #[test]
    #[serial]
    fn from_env_sets_estimator_fields() {
        unsafe { std::env::set_var("LIBERADO_ESTIMATOR_PROVIDER", "ollama") };
        unsafe { std::env::set_var("LIBERADO_ESTIMATOR_MODEL", "llama3") };
        unsafe { std::env::set_var("LIBERADO_ESTIMATOR_API_KEY", "sk-test") };
        unsafe { std::env::set_var("LIBERADO_ESTIMATOR_BASE_URL", "http://localhost:11434") };
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.estimator_provider, "ollama");
        assert_eq!(cfg.estimator_model, "llama3");
        assert_eq!(cfg.estimator_api_key, "sk-test");
        assert_eq!(cfg.estimator_base_url, "http://localhost:11434");
        unsafe { std::env::remove_var("LIBERADO_ESTIMATOR_PROVIDER") };
        unsafe { std::env::remove_var("LIBERADO_ESTIMATOR_MODEL") };
        unsafe { std::env::remove_var("LIBERADO_ESTIMATOR_API_KEY") };
        unsafe { std::env::remove_var("LIBERADO_ESTIMATOR_BASE_URL") };
    }

    #[test]
    #[serial]
    fn from_env_sets_search_thresholds() {
        unsafe { std::env::set_var("LIBERADO_SEARCH_STRONG_MATCH_THRESHOLD", "0.75") };
        unsafe { std::env::set_var("LIBERADO_SEARCH_MAX_WEAK_RESULTS", "7") };
        let cfg = ServerConfig::from_env();
        assert!((cfg.search_strong_match_threshold - 0.75).abs() < 0.001);
        assert_eq!(cfg.search_max_weak_results, 7);
        unsafe { std::env::remove_var("LIBERADO_SEARCH_STRONG_MATCH_THRESHOLD") };
        unsafe { std::env::remove_var("LIBERADO_SEARCH_MAX_WEAK_RESULTS") };
    }

    #[test]
    #[serial]
    fn from_env_invalid_search_fields_use_defaults() {
        unsafe { std::env::set_var("LIBERADO_SEARCH_STRONG_MATCH_THRESHOLD", "bad") };
        unsafe { std::env::set_var("LIBERADO_SEARCH_MAX_WEAK_RESULTS", "bad") };
        let cfg = ServerConfig::from_env();
        assert!((cfg.search_strong_match_threshold - 0.6).abs() < 0.001);
        assert_eq!(cfg.search_max_weak_results, 3);
        unsafe { std::env::remove_var("LIBERADO_SEARCH_STRONG_MATCH_THRESHOLD") };
        unsafe { std::env::remove_var("LIBERADO_SEARCH_MAX_WEAK_RESULTS") };
    }

    #[test]
    #[serial]
    fn from_env_sets_new_search_limits() {
        unsafe { std::env::set_var("LIBERADO_SEARCH_MAX_RESULTS_HARD_LIMIT", "15") };
        unsafe { std::env::set_var("LIBERADO_LOCAL_SEARCH_MIN_SCORE", "0.2") };
        unsafe { std::env::set_var("LIBERADO_USDA_MATCH_MIN_SCORE", "0.05") };
        unsafe { std::env::set_var("LIBERADO_OFF_MATCH_MIN_SCORE", "0.05") };
        unsafe { std::env::set_var("LIBERADO_USDA_API_PAGE_SIZE", "10") };
        unsafe { std::env::set_var("LIBERADO_OFF_API_PAGE_SIZE", "10") };
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.search_max_results_hard_limit, 15);
        assert!((cfg.local_search_min_score - 0.2).abs() < 0.001);
        assert!((cfg.usda_match_min_score - 0.05).abs() < 0.001);
        assert!((cfg.off_match_min_score - 0.05).abs() < 0.001);
        assert_eq!(cfg.usda_api_page_size, 10);
        assert_eq!(cfg.off_api_page_size, 10);
        unsafe { std::env::remove_var("LIBERADO_SEARCH_MAX_RESULTS_HARD_LIMIT") };
        unsafe { std::env::remove_var("LIBERADO_LOCAL_SEARCH_MIN_SCORE") };
        unsafe { std::env::remove_var("LIBERADO_USDA_MATCH_MIN_SCORE") };
        unsafe { std::env::remove_var("LIBERADO_OFF_MATCH_MIN_SCORE") };
        unsafe { std::env::remove_var("LIBERADO_USDA_API_PAGE_SIZE") };
        unsafe { std::env::remove_var("LIBERADO_OFF_API_PAGE_SIZE") };
    }

    #[test]
    #[serial]
    fn from_env_sets_log_list_limits() {
        unsafe { std::env::set_var("LIBERADO_LOG_LIST_DEFAULT_LIMIT", "50") };
        unsafe { std::env::set_var("LIBERADO_LOG_LIST_MAX_LIMIT", "200") };
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.log_list_default_limit, 50);
        assert_eq!(cfg.log_list_max_limit, 200);
        unsafe { std::env::remove_var("LIBERADO_LOG_LIST_DEFAULT_LIMIT") };
        unsafe { std::env::remove_var("LIBERADO_LOG_LIST_MAX_LIMIT") };
    }

    #[test]
    #[serial]
    fn yaml_overrides_defaults() {
        use std::io::Write;

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            tmp,
            "db_max_connections: 20\nhttp_client_timeout_secs: 60\nlocal_search_min_score: 0.25"
        )
        .unwrap();

        unsafe { std::env::set_var("LIBERADO_CONFIG", tmp.path().to_str().unwrap()) };
        // Ensure no env override that would shadow the yaml value
        unsafe { std::env::remove_var("LIBERADO_DB_MAX_CONNECTIONS") };
        unsafe { std::env::remove_var("LIBERADO_HTTP_CLIENT_TIMEOUT_SECS") };
        unsafe { std::env::remove_var("LIBERADO_LOCAL_SEARCH_MIN_SCORE") };

        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.db_max_connections, 20);
        assert_eq!(cfg.http_client_timeout_secs, 60);
        assert!((cfg.local_search_min_score - 0.25).abs() < 0.001);

        unsafe { std::env::remove_var("LIBERADO_CONFIG") };
    }

    #[test]
    #[serial]
    fn env_overrides_yaml() {
        use std::io::Write;

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "db_max_connections: 20").unwrap();

        unsafe { std::env::set_var("LIBERADO_CONFIG", tmp.path().to_str().unwrap()) };
        unsafe { std::env::set_var("LIBERADO_DB_MAX_CONNECTIONS", "99") };

        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.db_max_connections, 99);

        unsafe { std::env::remove_var("LIBERADO_CONFIG") };
        unsafe { std::env::remove_var("LIBERADO_DB_MAX_CONNECTIONS") };
    }
}
