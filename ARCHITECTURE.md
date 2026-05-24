# Architecture

## Crate layout

```
liberado-calorie-counter-mcp/
├── liberado-core/        shared library
│   ├── src/db.rs         connection pool factory
│   ├── src/error.rs      CoreError type
│   ├── src/estimator.rs  NutritionEstimator trait + NoopEstimator
│   └── src/models.rs     sqlx row types (User, Recipe, MealLog, …)
│
├── liberado-mcp/         MCP server binary
│   ├── src/main.rs       CLI (serve / user add / user list)
│   ├── src/config.rs     ServerConfig from env vars
│   ├── src/server.rs     all 16 MCP tool handlers
│   ├── src/food.rs       food search, external API calls, caching
│   ├── src/units.rs      unit parsing and nutrient scaling
│   └── src/error.rs      Error → McpError bridging
│
└── liberado-import/      bulk USDA import CLI (placeholder)
```

`liberado-core` has no dependency on TurboMCP. The MCP-specific layer (`liberado-mcp`) depends on `liberado-core`. This keeps the domain logic independent of the protocol.

## Request lifecycle

```
LLM client
    │
    │  JSON-RPC 2.0  (stdio pipe or HTTP POST)
    ▼
TurboMCP transport layer
    │  deserialises params, validates JSON Schema
    ▼
LiberadoServer::tool_handler(&self, params…)   [server.rs]
    │  1. resolve_user(api_key)   — Argon2 verification against DB
    │  2. business logic
    │     ├── food::search(…)     — local → USDA → OFF fallback
    │     ├── units::parse_amount / resolve_named_portion
    │     └── sqlx queries against PostgreSQL
    │  3. return String / McpResult<String>
    ▼
TurboMCP: serialises response, sends to client
```

Every tool call is stateless: no session state is kept in the server process. All state lives in PostgreSQL.

## Transport

Selected at startup via `LIBERADO_TRANSPORT`:

- **stdio** — one process per user session; used with Claude Desktop. `LIBERADO_DEFAULT_API_KEY` seeds a user on first boot.
- **http** — multi-user; API key passed as a tool parameter on every call. Uses a sqlx connection pool (`LIBERADO_DB_MAX_CONNECTIONS`, default 5).

Tool handlers are identical between transports.

## Authentication

`resolve_user(api_key)` runs on every tool call:

1. Fetches all rows from `users`.
2. Iterates and runs `Argon2::verify_password` against each stored hash.
3. Returns the matching `User` or an `unauthorized` error.

The plaintext key never touches the database. There is no session token or bearer auth — each call is independently verified.

> **Note**: The full-table scan + Argon2 per-call is acceptable for small user counts (single-user to tens of users). At larger scale, add a fast pre-filter (e.g. store a non-secret key prefix for lookup, then verify only the matching row).

## Food lookup fallback chain

```
search(query)
  │
  ├─ Step 1: PostgreSQL local cache
  │    pg_trgm similarity on canonical_name + food_aliases
  │    ├─ score ≥ threshold  →  auto_selected = true, return match
  │    └─ score < threshold  →  return top-N candidates (fallback_required = false)
  │
  ├─ Step 2: External APIs (in parallel via tokio::join!)
  │    USDA FoodData Central  +  Open Food Facts
  │    │
  │    ├─ trigram re-ranking of returned candidates (best_usda_match / best_off_match)
  │    ├─ cache winning result in food_items + food_nutrient_values
  │    ├─ (USDA) fetch detail endpoint for named portions → food_portions
  │    └─ return auto_selected = true
  │
  └─ Step 3: Nothing found
       fallback_required = true
       LLM should ask user for nutrition data, then call confirm_food
```

The trigram re-ranking in Step 2 uses Jaccard similarity over character trigrams (same algorithm as `pg_trgm`) computed in Rust, ensuring the best name match from the API results is selected rather than the first.

## Unit handling

`units::parse_amount(amount, unit)` returns a `ParsedAmount` variant:

| Input | Variant |
|---|---|
| `(200, "g")`, `(7, "oz")`, `(1, "lb")` | `Grams(f32)` after conversion |
| `(250, "ml")`, `(1, "l")` | `Milliliters(f32)` after conversion |
| `(1, "cup")`, `(2, "tbsp")` | `Named { label, count }` |

Named variants are resolved to `Grams` or `Milliliters` by `resolve_named_portion`, which looks up the `food_portions` table for the specific `food_id`.

`scale_nutrient(value_per_100, amount, basis)` converts the stored per-100-unit value to the actual logged amount:

| basis | amount variant | result |
|---|---|---|
| `per_100g` | `Grams(g)` | `value × g / 100` |
| `per_100ml` | `Milliliters(ml)` | `value × ml / 100` |
| mismatch | any | `0.0` (logged) |

The `basis` field on `food_items` is the key to correct liquid calorie math. Foods confirmed with `basis = "per_100ml"` produce correct totals when logged in ml.

## Nutrient snapshots

At log time, `kcal_snapshot` and `nutrient_snapshot` (JSONB) are computed from current food data and frozen on the `log_entries` row. They are never recomputed. This means:

- Historical log entries are unaffected by later edits to food items or recipes.
- `get_daily_summary` reads snapshots directly without re-joining nutrient tables.
- Recipe ingredient changes do not retroactively alter logged meals.

## Idempotency

`log_food`, `log_recipe`, and `log_exercise` each require an `idempotency_key` from the caller. This is stored with a `UNIQUE` constraint:

```sql
ON CONFLICT (idempotency_key) DO UPDATE SET kcal_snapshot = log_entries.kcal_snapshot
```

The no-op update ensures `RETURNING id` always fires, so the caller gets the same entry ID on a retry. Duplicate log calls are safe.

## Database schema summary

```
users                   api_key_hash, timezone
user_goals              kcal/macro targets versioned by effective_from date
nutrients               lookup table: name, display_name, unit, usda_nutrient_id
food_items              canonical_name, basis (per_100g|per_100ml), source, confidence
food_nutrient_values    food_id × nutrient_id → value per 100 base units
food_portions           food_id × unit_label → gram_equivalent / ml_equivalent
food_aliases            personal nicknames + global aliases (trigram-indexed)
food_item_tags          per-food labels ("organic", "seed oils")
recipes                 user-owned named composite meals
recipe_ingredients      recipe_id × food_id → amount_g / amount_ml
meal_logs               user_id, logged_at (UTC), meal_type
log_entries             meal_log_id, food_id|recipe_id, amounts, kcal_snapshot, nutrient_snapshot, idempotency_key
log_entry_tags          per-entry labels ("cheat meal", "post-workout")
exercise_logs           calories_burned, source, idempotency_key
weight_logs             weight_kg per timestamp
```

`food_items`, `nutrients`, and global aliases/tags are shared across users. Everything else is per-user via `user_id` foreign keys.

All timestamps stored as `TIMESTAMPTZ` (UTC). Date queries use `AT TIME ZONE users.timezone` so per-user "today" is always correct regardless of the server's local timezone.

## Estimator trait

The `NutritionEstimator` trait in `liberado-core` is the escape hatch for foods that cannot be found in any database:

```rust
#[async_trait]
pub trait NutritionEstimator: Send + Sync {
    async fn estimate(&self, description: &str, amount_g: f32) -> Result<EstimatedNutrition>;
}
```

Currently only `NoopEstimator` is implemented (returns `EstimationUnavailable`). Future implementations:

- `ClaudeEstimator` — Anthropic API
- `OllamaEstimator` — local model (no API cost)

Configured via `LIBERADO_ESTIMATOR_PROVIDER`. The quality of the serving LLM does not affect estimation quality.

## Key dependencies

| Crate | Purpose |
|---|---|
| `turbomcp` | MCP protocol framework (macros, transport, schema generation) |
| `sqlx` | Async PostgreSQL with compile-time query checking |
| `argon2` | API key hashing |
| `reqwest` | USDA + Open Food Facts HTTP calls |
| `chrono` | Timestamp parsing and timezone-aware date math |
| `serde` / `serde_json` | Serialisation throughout |
| `clap` | CLI for user management subcommands |
| `tokio` | Async runtime |
| `tracing` | Structured logging |
| `serial_test` | Serialises env-var tests to prevent races |

## Out of scope (v1)

- Barcode scanning (Open Food Facts supports it; straightforward addition)
- Photo-based food recognition
- Recursive recipes (recipe containing a recipe)
- Semantic / vector search (`pgvector` extension is installed and ready, column not yet added)
- Data export (CSV, JSON)
- Web dashboard (planned as a separate `liberado-dashboard` binary using `rustview`)
- Push notifications or wearable integrations
