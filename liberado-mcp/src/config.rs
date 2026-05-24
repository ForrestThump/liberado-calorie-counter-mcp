/// Transport selection and full server configuration, loaded from environment variables.
/// All LIBERADO_* env vars override defaults; no config file is required.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum TransportConfig {
    #[default]
    Stdio,
    Http { host: String, port: u16 },
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub database_url: String,
    pub db_max_connections: u32,
    pub transport: TransportConfig,
    /// In stdio mode: API key for the single session user. In HTTP mode: unused
    /// (key is supplied per request as a tool parameter).
    pub default_api_key: String,
    pub usda_api_key: String,
    pub estimator_provider: String,
    pub estimator_model: String,
    pub estimator_api_key: String,
    pub estimator_base_url: String,
    pub search_strong_match_threshold: f64,
    pub search_max_weak_results: u32,
}

fn default_http_port() -> u16 {
    8080
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
            estimator_model: "claude-opus-4-7".to_string(),
            estimator_api_key: String::new(),
            estimator_base_url: "http://localhost:11434".to_string(),
            search_strong_match_threshold: 0.6,
            search_max_weak_results: 3,
        }
    }
}

impl ServerConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();

        if let Ok(v) = std::env::var("LIBERADO_DATABASE_URL") {
            cfg.database_url = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_DB_MAX_CONNECTIONS")
            && let Ok(n) = v.parse() {
            cfg.db_max_connections = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_DEFAULT_API_KEY") {
            cfg.default_api_key = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_USDA_API_KEY") {
            cfg.usda_api_key = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_ESTIMATOR_PROVIDER") {
            cfg.estimator_provider = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_ESTIMATOR_MODEL") {
            cfg.estimator_model = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_ESTIMATOR_API_KEY") {
            cfg.estimator_api_key = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_ESTIMATOR_BASE_URL") {
            cfg.estimator_base_url = v;
        }
        if let Ok(v) = std::env::var("LIBERADO_SEARCH_STRONG_MATCH_THRESHOLD")
            && let Ok(n) = v.parse() {
            cfg.search_strong_match_threshold = n;
        }
        if let Ok(v) = std::env::var("LIBERADO_SEARCH_MAX_WEAK_RESULTS")
            && let Ok(n) = v.parse() {
            cfg.search_max_weak_results = n;
        }

        if let Ok(v) = std::env::var("LIBERADO_TRANSPORT") {
            match v.to_lowercase().as_str() {
                "http" => {
                    let host = std::env::var("LIBERADO_HTTP_HOST")
                        .unwrap_or_else(|_| "0.0.0.0".to_string());
                    let port = std::env::var("LIBERADO_HTTP_PORT")
                        .ok()
                        .and_then(|p| p.parse().ok())
                        .unwrap_or_else(default_http_port);
                    cfg.transport = TransportConfig::Http { host, port };
                }
                _ => {
                    cfg.transport = TransportConfig::Stdio;
                }
            }
        }

        cfg
    }
}

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
}
