# Liberado Calorie Counter MCP

A self-hosted [Model Context Protocol](https://modelcontextprotocol.io/) server written in Rust for accurate, private nutrition tracking. Connect any MCP-compatible LLM client and say "I had a bowl of oatmeal and a glass of milk" — Liberado handles lookup, unit conversion, calorie math, and logging without the LLM needing to know nutrition facts.

## Features

- **Automatic food lookup** — searches a local PostgreSQL cache first, then falls back to [USDA FoodData Central](https://fdc.nal.usda.gov/) and [Open Food Facts](https://world.openfoodfacts.org/). Results are cached on first retrieval.
- **Trigram re-ranking** — external API results are re-ranked by trigram similarity (matching PostgreSQL's `pg_trgm`) so the best name match is always selected, not just the first result returned.
- **Liquids and named portions** — nutrients can be expressed per 100 ml (`basis = per_100ml`) for accurate logging of liquids (milk, juice, oil). Named serving sizes (cup, tbsp, slice) are stored per food and resolved automatically.
- **Multi-user** — every user has their own log, goals, recipes, and aliases. Food data is shared across users. Auth uses Argon2-hashed API keys.
- **Idempotent logging** — each log call requires a caller-supplied `idempotency_key`. Retries are ignored automatically, preventing double-counting from LLM retries.
- **Two transports** — `stdio` for Claude Desktop / single-user, `http` for shared or remote deployments.
- **16 MCP tools** with full JSON Schema parameter descriptions so LLMs can use the server from a cold start without additional instructions.

## Quick Start

### 1. Prerequisites

- Rust (edition 2024, `rust-version = "1.89"`)
- Docker (for the bundled PostgreSQL)

### 2. Clone and start PostgreSQL

```bash
git clone https://github.com/youruser/liberado-calorie-counter-mcp
cd liberado-calorie-counter-mcp
docker compose up -d
```

### 3. Create a user

```bash
cargo build --release -p liberado-mcp

LIBERADO_DATABASE_URL=postgresql://liberado:liberado@localhost:5432/liberado \
  ./target/release/liberado-mcp user add \
    --username alice \
    --api-key your-secret-key \
    --timezone America/New_York
```

### 4. Start the server

**stdio** (Claude Desktop / single-user):

```bash
LIBERADO_DATABASE_URL=postgresql://liberado:liberado@localhost:5432/liberado \
LIBERADO_TRANSPORT=stdio \
./target/release/liberado-mcp
```

**HTTP** (multi-user / remote):

```bash
LIBERADO_DATABASE_URL=postgresql://liberado:liberado@localhost:5432/liberado \
LIBERADO_TRANSPORT=http \
LIBERADO_HTTP_HOST=127.0.0.1 \
LIBERADO_HTTP_PORT=8765 \
./target/release/liberado-mcp
```

### 5. Connect an LLM client

Point your MCP client at the server. For Claude Desktop (stdio mode), add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "liberado": {
      "command": "/path/to/liberado-mcp",
      "env": {
        "LIBERADO_DATABASE_URL": "postgresql://liberado:liberado@localhost:5432/liberado",
        "LIBERADO_DEFAULT_API_KEY": "your-secret-key"
      }
    }
  }
}
```

## Configuration

All configuration is via environment variables. Copy `.env.example` to `.env` as a reference:

| Variable | Default | Description |
|---|---|---|
| `LIBERADO_DATABASE_URL` | `postgresql://liberado:liberado@localhost:5432/liberado` | PostgreSQL connection string |
| `LIBERADO_DB_MAX_CONNECTIONS` | `5` | Connection pool size |
| `LIBERADO_TRANSPORT` | `stdio` | Transport: `stdio` or `http` |
| `LIBERADO_HTTP_HOST` | `0.0.0.0` | HTTP bind address (http mode only) |
| `LIBERADO_HTTP_PORT` | `8080` | HTTP port (http mode only) |
| `LIBERADO_DEFAULT_API_KEY` | _(empty)_ | stdio mode: seeds a `default` user on first boot |
| `LIBERADO_USDA_API_KEY` | _(empty)_ | [USDA FDC API key](https://fdc.nal.usda.gov/api-guide.html) (free, recommended) |
| `LIBERADO_ESTIMATOR_PROVIDER` | `none` | LLM estimator: `none`, `claude`, or `ollama` |
| `LIBERADO_ESTIMATOR_MODEL` | `claude-opus-4-7` | Model name for the estimator |
| `LIBERADO_ESTIMATOR_API_KEY` | _(empty)_ | API key for the estimator (Claude only) |
| `LIBERADO_ESTIMATOR_BASE_URL` | `http://localhost:11434` | Base URL for Ollama |
| `LIBERADO_SEARCH_STRONG_MATCH_THRESHOLD` | `0.6` | pg_trgm score above which a match is auto-selected |
| `LIBERADO_SEARCH_MAX_WEAK_RESULTS` | `3` | Max candidates returned on a weak match |

## MCP Tools

All tools require an `api_key` parameter. Full parameter descriptions are returned by `tools/list`.

| Tool | Description |
|---|---|
| `search_food` | Search by name; checks local cache, then USDA and Open Food Facts |
| `confirm_food` | Manually add a food with known nutrition data |
| `refresh_food` | Re-fetch a USDA-sourced food from the API |
| `add_food_alias` | Register a personal nickname for a food |
| `tag_food` | Attach a label to a food item for later filtering |
| `log_food` | Log a food item by name with amount and unit |
| `log_recipe` | Log a saved recipe |
| `log_exercise` | Log an exercise session with calories burned |
| `log_weight` | Log a body weight measurement |
| `create_recipe` | Define a named composite meal from food ingredients |
| `list_recent_logs` | List log entries with optional filters |
| `get_daily_summary` | Full nutrient totals vs. goals for a given date |
| `get_net_calories` | Calories consumed minus calories burned for a date |
| `get_meal_summary` | Per-item breakdown for a specific meal |
| `get_weight_history` | Body weight entries over a date range |
| `set_goals` | Set daily calorie and macro targets |

### Logging workflow

1. **`log_food "oatmeal" 1 cup`** — searches automatically. If ambiguous, call `search_food` first to get the exact canonical name. If nothing is found, call `confirm_food` to add it.
2. **Named portions** — cups, tbsp, etc. are supported when registered for a food. USDA items have portions populated automatically; use `confirm_food` with the `portions` parameter for manually-added items.
3. **Liquids** — pass `basis: "per_100ml"` to `confirm_food` for items whose nutrients are expressed per 100 ml (milk, juice, oil). Logging in ml then produces correct calorie totals.

## User Management

```bash
# Add a user
liberado-mcp user add --username bob --api-key secret --timezone Europe/London

# List users
liberado-mcp user list
```

## Development

```bash
# Run all tests (DB integration tests are ignored without a live PG)
cargo test --workspace

# Lint
cargo clippy --workspace --all-targets

# Build release binary
cargo build --release -p liberado-mcp
```

Database migrations are managed with sqlx and run automatically at server startup. Migration files are in `migrations/`.

## Workspace Layout

```
liberado-calorie-counter-mcp/
  Cargo.toml           workspace manifest
  docker-compose.yml   PostgreSQL (pgvector/pg17)
  migrations/          sqlx migration files
  liberado-core/       shared library: types, DB pool, estimator trait
  liberado-mcp/        MCP server binary (stdio + HTTP)
  liberado-import/     bulk USDA import CLI (placeholder, not yet implemented)
```

## License

MIT
