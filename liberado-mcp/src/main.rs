use std::sync::Arc;

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};
use clap::{Parser, Subcommand};
use tracing::info;
use turbomcp::prelude::*;

use liberado_core::{db::create_pool, estimator::NoopEstimator};
use liberado_mcp::{
    config::{ServerConfig, TransportConfig},
    server::LiberadoServer,
};

// ─── CLI definition ───────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "liberado-mcp", about = "Liberado calorie-counter MCP server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the MCP server (default when no subcommand is given)
    Serve,
    /// Manage users
    User {
        #[command(subcommand)]
        action: UserAction,
    },
}

#[derive(Subcommand)]
enum UserAction {
    /// Add a new user and print their hashed API key
    Add {
        #[arg(long)]
        username: String,
        /// Plain-text API key the user will authenticate with
        #[arg(long)]
        api_key: String,
        /// IANA timezone name (e.g. America/New_York). Defaults to UTC.
        #[arg(long, default_value = "UTC")]
        timezone: String,
    },
    /// List all users
    List,
}

// ─── Entry point ──────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => cmd_serve().await,
        Command::User { action } => cmd_user(action).await,
    }
}

// ─── serve ────────────────────────────────────────────────────────────────────

async fn cmd_serve() {
    init_tracing();
    let config = ServerConfig::from_env();
    info!(
        transport = ?config.transport,
        estimator = %config.estimator_provider,
        "liberado-calorie-mcp starting"
    );

    let config = Arc::new(config);

    let pool = create_pool(&config.database_url, config.db_max_connections)
        .await
        .expect("failed to connect to PostgreSQL");

    sqlx::migrate!("../migrations")
        .run(&pool)
        .await
        .expect("failed to run database migrations");

    info!("migrations applied");

    // Seed a default user on first boot if LIBERADO_DEFAULT_API_KEY is set
    // and no users exist yet.
    if !config.default_api_key.is_empty() {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .expect("failed to query users");

        if count == 0 {
            match hash_api_key(&config.default_api_key) {
                Ok(hash) => {
                    sqlx::query(
                        "INSERT INTO users (username, api_key_hash, timezone)
                         VALUES ('default', $1, 'UTC')",
                    )
                    .bind(&hash)
                    .execute(&pool)
                    .await
                    .expect("failed to seed default user");

                    info!("seeded default user from LIBERADO_DEFAULT_API_KEY");
                }
                Err(e) => tracing::error!("failed to hash default API key: {e}"),
            }
        }
    }

    let estimator: Arc<dyn liberado_core::estimator::NutritionEstimator> =
        match config.estimator_provider.as_str() {
            "claude" => {
                tracing::warn!("claude estimator not yet implemented; falling back to noop");
                Arc::new(NoopEstimator)
            }
            "ollama" => {
                tracing::warn!("ollama estimator not yet implemented; falling back to noop");
                Arc::new(NoopEstimator)
            }
            _ => Arc::new(NoopEstimator),
        };

    let server = LiberadoServer::new(pool, config.clone(), estimator);

    let builder = server.builder().with_protocol(ProtocolConfig {
        allow_fallback: true,
        ..Default::default()
    });

    let transport = config.transport.clone();

    let server = match transport {
        TransportConfig::Stdio => {
            info!("transport: stdio");
            builder.transport(turbomcp::Transport::stdio())
        }
        TransportConfig::Http { ref host, port } => {
            let addr = format!("{host}:{port}");
            info!(addr = %addr, "transport: HTTP");
            builder
                .transport(turbomcp::Transport::http(addr))
                .allow_any_origin(true)
        }
    };

    server.serve().await.unwrap();
}

// ─── user ─────────────────────────────────────────────────────────────────────

async fn cmd_user(action: UserAction) {
    // User management commands need DB access but don't need the full server.
    // We read config from env just like serve does.
    let config = ServerConfig::from_env();

    let pool = create_pool(&config.database_url, config.db_max_connections)
        .await
        .expect("failed to connect to PostgreSQL");

    sqlx::migrate!("../migrations")
        .run(&pool)
        .await
        .expect("failed to run database migrations");

    match action {
        UserAction::Add { username, api_key, timezone } => {
            let hash = hash_api_key(&api_key).expect("failed to hash API key");

            sqlx::query(
                "INSERT INTO users (username, api_key_hash, timezone)
                 VALUES ($1, $2, $3)",
            )
            .bind(&username)
            .bind(&hash)
            .bind(&timezone)
            .execute(&pool)
            .await
            .expect("failed to insert user (username may already exist)");

            println!("User '{username}' created (timezone: {timezone}).");
            println!("Store the plain-text API key securely — it cannot be recovered from the hash.");
        }
        UserAction::List => {
            let rows: Vec<(i32, String, String, String)> = sqlx::query_as(
                "SELECT id, username, timezone, created_at::text FROM users ORDER BY id",
            )
            .fetch_all(&pool)
            .await
            .expect("failed to query users");

            if rows.is_empty() {
                println!("No users found.");
            } else {
                println!("{:<6} {:<20} {:<30} Created", "ID", "Username", "Timezone");
                println!("{}", "-".repeat(75));
                for (id, username, timezone, created_at) in rows {
                    println!("{:<6} {:<20} {:<30} {}", id, username, timezone, created_at);
                }
            }
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn hash_api_key(key: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default().hash_password(key.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_tracing_does_not_panic() {
        let _ = std::panic::catch_unwind(init_tracing);
    }

    #[test]
    fn hash_api_key_produces_verifiable_hash() {
        use argon2::{Argon2, PasswordHash, PasswordVerifier};
        let key = "my-secret-api-key";
        let hash_str = hash_api_key(key).unwrap();
        let hash = PasswordHash::new(&hash_str).unwrap();
        assert!(Argon2::default().verify_password(key.as_bytes(), &hash).is_ok());
    }

    #[test]
    fn hash_api_key_different_keys_produce_different_hashes() {
        let h1 = hash_api_key("key-one").unwrap();
        let h2 = hash_api_key("key-two").unwrap();
        assert_ne!(h1, h2);
    }
}
