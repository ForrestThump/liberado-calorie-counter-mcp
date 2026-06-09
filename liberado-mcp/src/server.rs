use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value as JsonValue;
use turbomcp::prelude::*;

use liberado_core::estimator::NutritionEstimator;

trait IntoMcpResult<T> {
    fn mcp_err(self) -> McpResult<T>;
}

impl<T, E: std::fmt::Display> IntoMcpResult<T> for Result<T, E> {
    fn mcp_err(self) -> McpResult<T> {
        self.mcp_err()
    }
}

use crate::config::ServerConfig;
use crate::food::{self, FoodSearchOptions};
use crate::units::{self, ParsedAmount};

pub struct AppState {
    pub db: sqlx::PgPool,
    pub http_client: reqwest::Client,
    pub config: Arc<ServerConfig>,
    pub estimator: Arc<dyn NutritionEstimator>,
}

#[derive(Clone)]
pub struct LiberadoServer {
    state: Arc<AppState>,
}

#[turbomcp::server(name = "liberado-calorie-mcp", version = "0.1.0")]
#[allow(clippy::too_many_arguments)]
impl LiberadoServer {
    pub fn new(
        db: sqlx::PgPool,
        config: Arc<ServerConfig>,
        estimator: Arc<dyn NutritionEstimator>,
    ) -> Self {
        Self {
            state: Arc::new(AppState {
                db,
                http_client: reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(config.http_client_timeout_secs))
                    .build()
                    .expect("failed to build HTTP client"),
                config,
                estimator,
            }),
        }
    }

    // ─── Food search & management ─────────────────────────────────────────────

    /// Search for a food by name. Checks the local cache first, then falls back
    /// to USDA FoodData Central and Open Food Facts. A strong match is
    /// auto-selected and ready to use in log_food; weak matches are returned as
    /// candidates \u{2014} pick the best name and pass it to log_food, or call
    /// confirm_food if none match.
    #[tool("Search for a food by name; checks local cache then USDA and Open Food Facts. Returns food_id, canonical name, kcal, and macros. Strong matches are auto-selected; weak matches list candidates for confirm_food.")]
    async fn search_food(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("Food name to look up (e.g. 'brown rice', 'whole milk', 'banana')")]
        query: String,
        #[description("Maximum number of results to return (default 3, max 10)")]
        limit: Option<u32>,
    ) -> McpResult<String> {
        let _ = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;
        let max = limit
            .unwrap_or(self.state.config.search_max_weak_results)
            .min(self.state.config.search_max_results_hard_limit) as usize;

        let opts = FoodSearchOptions::from(&*self.state.config);
        let resp = food::search(
            &self.state.db,
            &self.state.http_client,
            &self.state.config.usda_api_key,
            self.state.config.search_strong_match_threshold as f32,
            max,
            &query,
            &opts,
        )
        .await
        .mcp_err()?;

        serde_json::to_string_pretty(&resp).mcp_err()
    }

    /// Add a personal nickname so future lookups resolve immediately.
    #[tool("Add a personal alias for a food item so future lookups by that nickname resolve immediately. food_id comes from search_food results.")]
    async fn add_food_alias(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("ID of the food item to alias; from search_food results")]
        food_id: i32,
        #[description("Nickname to register (e.g. 'my protein shake', 'office coffee')")]
        alias: String,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;

        sqlx::query(
            "INSERT INTO food_aliases (food_id, alias, user_id)
             VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(food_id)
        .bind(&alias)
        .bind(user.id)
        .execute(&self.state.db)
        .await
        .mcp_err()?;

        Ok(format!("Alias '{alias}' added for food_id {food_id}."))
    }

    /// Add a descriptive tag to a food item ("contains seed oils", "organic").
    #[tool("Attach a descriptive label to a food item for later filtering in list_recent_logs. food_id comes from search_food results.")]
    async fn tag_food(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("ID of the food item to tag; from search_food results")]
        food_id: i32,
        #[description("Label to attach (e.g. 'organic', 'contains gluten', 'seed oils')")]
        tag: String,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;

        sqlx::query(
            "INSERT INTO food_item_tags (food_id, tag, user_id)
             VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(food_id)
        .bind(&tag)
        .bind(user.id)
        .execute(&self.state.db)
        .await
        .mcp_err()?;

        Ok(format!("Tag '{tag}' added to food_id {food_id}."))
    }

    /// Manually add a food item with known nutrition data. Use after search_food
    /// returns no match, or to add any food with precise figures. The food is
    /// immediately available in log_food. Supply portions to enable logging by
    /// cup, tbsp, or other named units.
    #[tool("Add a food item with user-supplied nutrition data. Use when search_food finds no match. basis='per_100ml' for liquids (milk, juice, oil); default is per_100g. portions enables named-unit logging (e.g. cup, tbsp).")]
    async fn confirm_food(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("Canonical name for this food (e.g. 'Whole Milk', 'Rolled Oats')")]
        food_name: String,
        #[description("Kilocalories per 100 base units (100g or 100ml depending on basis)")]
        kcal_per_100: f32,
        #[description("Protein in grams per 100 base units")]
        protein_per_100: Option<f32>,
        #[description("Fat in grams per 100 base units")]
        fat_per_100: Option<f32>,
        #[description("Carbohydrates in grams per 100 base units")]
        carbs_per_100: Option<f32>,
        #[description("Nutrient basis: 'per_100g' for solids (default) or 'per_100ml' for liquids")]
        basis: Option<String>,
        #[description("Named serving sizes as a JSON array, e.g. [{\"unit\":\"cup\",\"grams\":90},{\"unit\":\"tbsp\",\"ml\":15}]. Use 'grams' for solid portions and 'ml' for liquid portions.")]
        portions: Option<String>,
    ) -> McpResult<String> {
        let _ = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;

        let basis_str = match basis.as_deref().unwrap_or("per_100g") {
            v @ ("per_100g" | "per_100ml") => v,
            other => return Err(McpError::internal(format!(
                "invalid basis '{other}': must be 'per_100g' or 'per_100ml'"
            ))),
        };

        let food_id = sqlx::query_scalar::<_, i32>(
            "INSERT INTO food_items (canonical_name, basis, source, confidence)
             VALUES ($1, $2, 'user', 'user_confirmed')
             RETURNING id",
        )
        .bind(&food_name)
        .bind(basis_str)
        .fetch_one(&self.state.db)
        .await
        .mcp_err()?;

        let nutrients: &[(&str, f32)] = &[
            ("energy",        kcal_per_100),
            ("protein",       protein_per_100.unwrap_or(0.0)),
            ("fat_total",     fat_per_100.unwrap_or(0.0)),
            ("carbohydrates", carbs_per_100.unwrap_or(0.0)),
        ];

        for (name, value) in nutrients {
            if *value == 0.0 { continue; }
            sqlx::query(
                "INSERT INTO food_nutrient_values (food_id, nutrient_id, value)
                 SELECT $1, id, $3 FROM nutrients WHERE name = $2
                 ON CONFLICT (food_id, nutrient_id) DO UPDATE SET value = EXCLUDED.value",
            )
            .bind(food_id)
            .bind(*name)
            .bind(*value)
            .execute(&self.state.db)
            .await
            .mcp_err()?;
        }

        if let Some(portions_json) = &portions {
            let portion_inputs: Vec<PortionInput> = serde_json::from_str(portions_json)
                .map_err(|e| McpError::internal(format!(
                    "portions must be a JSON array like \
                     [{{\"unit\":\"cup\",\"grams\":90}}]: {e}"
                )))?;

            for p in &portion_inputs {
                if p.grams.is_none() && p.ml.is_none() {
                    return Err(McpError::internal(format!(
                        "portion '{}' must specify either 'grams' or 'ml'", p.unit
                    )));
                }
                sqlx::query(
                    "INSERT INTO food_portions (food_id, unit_label, gram_equivalent, ml_equivalent)
                     VALUES ($1, $2, $3, $4)
                     ON CONFLICT (food_id, unit_label) DO UPDATE
                         SET gram_equivalent = EXCLUDED.gram_equivalent,
                             ml_equivalent   = EXCLUDED.ml_equivalent",
                )
                .bind(food_id)
                .bind(p.unit.trim().to_lowercase())
                .bind(p.grams)
                .bind(p.ml)
                .execute(&self.state.db)
                .await
                .mcp_err()?;
            }
        }

        let unit_label = if basis_str == "per_100ml" { "100ml" } else { "100g" };
        Ok(format!(
            "Cached '{food_name}' as food_id {food_id} with {kcal_per_100} kcal/{unit_label} (user-confirmed)."
        ))
    }

    /// Re-fetch a food item's data from external APIs, replacing the cached values.
    #[tool("Re-fetch a USDA-sourced food item's nutrition data and portions from the API, replacing cached values. Only works for USDA-sourced foods.")]
    async fn refresh_food(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("ID of the food item to refresh; from search_food results (must be USDA-sourced)")]
        food_id: i32,
    ) -> McpResult<String> {
        let _ = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;

        let row = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT source, source_id FROM food_items WHERE id = $1",
        )
        .bind(food_id)
        .fetch_optional(&self.state.db)
        .await
        .mcp_err()?
        .ok_or_else(|| McpError::internal(format!("food_id {food_id} not found")))?;

        let (source, source_id) = row;

        match source.as_str() {
            "usda" => {
                let sid = source_id
                    .ok_or_else(|| McpError::internal("food has no source_id"))?;
                let fdc_id: i32 = sid
                    .parse()
                    .map_err(|_| McpError::internal(format!("invalid source_id '{sid}'")))?
;

                let detail = food::fetch_usda_detail(
                    &self.state.http_client,
                    &self.state.config.usda_api_key,
                    fdc_id,
                )
                .await
                .mcp_err()?;

                sqlx::query("DELETE FROM food_nutrient_values WHERE food_id = $1")
                    .bind(food_id)
                    .execute(&self.state.db)
                    .await
                    .mcp_err()?;

                sqlx::query("DELETE FROM food_portions WHERE food_id = $1")
                    .bind(food_id)
                    .execute(&self.state.db)
                    .await
                    .mcp_err()?;

                sqlx::query(
                    "UPDATE food_items SET canonical_name = $1, updated_at = now() WHERE id = $2",
                )
                .bind(&detail.description)
                .bind(food_id)
                .execute(&self.state.db)
                .await
                .mcp_err()?;

                let nutrients = detail.to_usda_nutrients();
                food::insert_usda_nutrients(&self.state.db, food_id, &nutrients)
                    .await
                    .mcp_err()?;

                food::insert_usda_portions(&self.state.db, food_id, &detail.food_portions)
                    .await
                    .mcp_err()?;

                Ok(format!("Refreshed food_id {food_id} from USDA (fdc_id {sid})."))
            }
            _ => Err(McpError::internal(format!(
                "Refresh not supported for source '{source}'. Only USDA items can be refreshed."
            ))),
        }
    }

    // ─── Food logging ─────────────────────────────────────────────────────────

    /// Log a food item. Searches by name (local cache \u{2192} USDA \u{2192} Open Food Facts),
    /// converts units, and snapshots kcal + nutrients at write time. If the
    /// name is ambiguous the tool returns candidates \u{2014} call search_food to find
    /// the exact canonical name, or call confirm_food if no match exists.
    #[tool("Log a food item by name. Searches automatically; call search_food first if the name is ambiguous. Supports g, oz, lb, ml, l, and named portions (cup, tbsp, etc.) registered for that food.")]
    async fn log_food(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("Name of the food to log; searched automatically. Use the exact canonical name from search_food if there was ambiguity.")]
        food_name: String,
        #[description("Numeric quantity to log (e.g. 250 for 250 ml, 1.5 for 1.5 cups)")]
        amount: f32,
        #[description("Unit of measurement: g, oz, lb, kg for mass; ml, l for volume; or a named portion (cup, tbsp, tsp, slice) if registered for this food via confirm_food")]
        unit: String,
        #[description("Meal name (e.g. breakfast, lunch, dinner, snack)")]
        meal_type: String,
        #[description("When food was eaten: RFC 3339 ('2024-01-15T08:30:00Z') or date only ('2024-01-15'). Defaults to now.")]
        logged_at: Option<String>,
        #[description("Unique string for this logging intent (e.g. 'breakfast-milk-2024-01-15'). Safe to retry \u{2014} duplicate keys are ignored.")]
        idempotency_key: String,
        #[description("Optional labels to attach to this log entry (e.g. [\"cheat meal\", \"post-workout\"])")]
        tags: Option<Vec<String>>,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;
        let ts = parse_logged_at(logged_at.as_deref())?;

        // Find the food (full fallback chain: local \u{2192} USDA \u{2192} OFF)
        let opts = FoodSearchOptions::from(&*self.state.config);
        let search_resp = food::search(
            &self.state.db,
            &self.state.http_client,
            &self.state.config.usda_api_key,
            self.state.config.search_strong_match_threshold as f32,
            self.state.config.search_max_weak_results as usize,
            &food_name,
            &opts,
        )
        .await
        .mcp_err()?;

        if search_resp.fallback_required {
            return Err(McpError::internal(format!(
                "Food '{}' not found in any database. Call confirm_food to add it, then retry.",
                food_name
            )));
        }

        if !search_resp.auto_selected {
            let names: Vec<&str> = search_resp.matches.iter().map(|m| m.name.as_str()).collect();
            return Err(McpError::internal(format!(
                "Ambiguous food name '{}'. Did you mean: {}? \
                 Call search_food to see food_ids, then retry with the exact canonical name.",
                food_name,
                names.join(", ")
            )));
        }

        let matched = &search_resp.matches[0];
        let food_id = matched.food_id;

        // Resolve units
        let parsed = units::parse_amount(amount, &unit);
        let resolved = match parsed {
            ParsedAmount::Named { ref label, count } => {
                units::resolve_named_portion(&self.state.db, food_id, label, count)
                    .await
                    .mcp_err()?
                    .ok_or_else(|| McpError::internal(format!(
                        "Unit '{unit}' not recognized for '{}'. \
                         Use g, oz, lb, ml, l, or a portion from food_portions.",
                        matched.name
                    )))?
            }
            other => other,
        };

        // Compute snapshots
        let kcal_snapshot = units::scale_nutrient(matched.kcal_per_100, &resolved, &matched.basis);
        let nutrient_snapshot = build_nutrient_snapshot(
            &self.state.db,
            food_id,
            &resolved,
            &matched.basis,
        )
        .await?;

        // Find or create meal_log for this user/date/meal_type
        let meal_log_id = find_or_create_meal_log(
            &self.state.db,
            user.id,
            &ts,
            &meal_type,
            &user.timezone,
        )
        .await?;

        let (amount_g, amount_ml) = resolved_to_db_amounts(&resolved);

        // Idempotent insert: DO UPDATE with a no-op ensures RETURNING always fires
        let log_entry_id = sqlx::query_scalar::<_, i32>(
            "INSERT INTO log_entries
                 (meal_log_id, food_id, amount_g, amount_ml, kcal_snapshot, nutrient_snapshot, idempotency_key)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (idempotency_key) DO UPDATE
                 SET kcal_snapshot = log_entries.kcal_snapshot
             RETURNING id",
        )
        .bind(meal_log_id)
        .bind(food_id)
        .bind(amount_g)
        .bind(amount_ml)
        .bind(kcal_snapshot)
        .bind(&nutrient_snapshot)
        .bind(&idempotency_key)
        .fetch_one(&self.state.db)
        .await
        .mcp_err()?;

        if let Some(tag_list) = &tags {
            for tag in tag_list {
                sqlx::query(
                    "INSERT INTO log_entry_tags (log_entry_id, tag) VALUES ($1, $2)
                     ON CONFLICT DO NOTHING",
                )
                .bind(log_entry_id)
                .bind(tag)
                .execute(&self.state.db)
                .await
                .mcp_err()?;
            }
        }

        Ok(format!(
            "Logged {amount} {unit} of '{}' ({:.1} kcal) to {meal_type} on {}.",
            matched.name,
            kcal_snapshot,
            ts.format("%Y-%m-%d")
        ))
    }

    /// Log a saved recipe as a single meal entry.
    #[tool("Log a saved recipe as a meal entry. recipe_id comes from create_recipe. Nutrients are snapshotted from current ingredient definitions scaled by servings.")]
    async fn log_recipe(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("ID of the recipe to log; returned by create_recipe")]
        recipe_id: i32,
        #[description("Number of servings to log (default 1.0)")]
        servings: Option<f32>,
        #[description("Meal name (e.g. breakfast, lunch, dinner, snack)")]
        meal_type: String,
        #[description("When the meal was eaten: RFC 3339 ('2024-01-15T08:30:00Z') or date only ('2024-01-15'). Defaults to now.")]
        logged_at: Option<String>,
        #[description("Unique string for this logging intent. Safe to retry \u{2014} duplicate keys are ignored.")]
        idempotency_key: String,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;
        let ts = parse_logged_at(logged_at.as_deref())?;
        let scale = servings.unwrap_or(1.0);

        // Verify recipe ownership
        let recipe = sqlx::query_as::<_, liberado_core::models::Recipe>(
            "SELECT id, user_id, name, created_at FROM recipes WHERE id = $1",
        )
        .bind(recipe_id)
        .fetch_optional(&self.state.db)
        .await
        .mcp_err()?
        .ok_or_else(|| McpError::internal(format!("recipe_id {recipe_id} not found")))?;

        if recipe.user_id != user.id {
            return Err(McpError::internal("unauthorized: recipe belongs to another user"));
        }

        // Fetch ingredients
        let ingredients = sqlx::query_as::<_, liberado_core::models::RecipeIngredient>(
            "SELECT recipe_id, food_id, amount_g, amount_ml
             FROM recipe_ingredients WHERE recipe_id = $1",
        )
        .bind(recipe_id)
        .fetch_all(&self.state.db)
        .await
        .mcp_err()?;

        // Aggregate kcal and nutrients across all ingredients, scaled by servings
        let mut total_kcal: f32 = 0.0;
        let mut total_nutrients: std::collections::HashMap<String, f32> = Default::default();
        let mut total_g: f32 = 0.0;
        let mut total_ml: f32 = 0.0;

        for ing in &ingredients {
            let basis: String = sqlx::query_scalar(
                "SELECT basis FROM food_items WHERE id = $1",
            )
            .bind(ing.food_id)
            .fetch_optional(&self.state.db)
            .await
            .mcp_err()?
            .unwrap_or_else(|| "per_100g".to_string());

            let resolved = match (ing.amount_g, ing.amount_ml) {
                (Some(g), _) => ParsedAmount::Grams(g * scale),
                (_, Some(ml)) => ParsedAmount::Milliliters(ml * scale),
                _ => continue,
            };

            match &resolved {
                ParsedAmount::Grams(g) => total_g += g,
                ParsedAmount::Milliliters(ml) => total_ml += ml,
                ParsedAmount::Named { .. } => {}
            }

            let nutrient_rows: Vec<(String, f32)> = sqlx::query_as::<_, (String, f32)>(
                "SELECT n.name, fnv.value
                 FROM food_nutrient_values fnv
                 JOIN nutrients n ON n.id = fnv.nutrient_id
                 WHERE fnv.food_id = $1",
            )
            .bind(ing.food_id)
            .fetch_all(&self.state.db)
            .await
            .mcp_err()?;

            for (name, value_per_100) in nutrient_rows {
                let scaled = units::scale_nutrient(value_per_100, &resolved, &basis);
                *total_nutrients.entry(name.clone()).or_default() += scaled;
                if name == "energy" {
                    total_kcal += scaled;
                }
            }
        }

        let nutrient_snapshot = serde_json::to_value(&total_nutrients)
            .mcp_err()?;

        let meal_log_id = find_or_create_meal_log(
            &self.state.db,
            user.id,
            &ts,
            &meal_type,
            &user.timezone,
        )
        .await?;

        let (amount_g_db, amount_ml_db) = if total_g > 0.0 {
            (Some(total_g), None::<f32>)
        } else if total_ml > 0.0 {
            (None::<f32>, Some(total_ml))
        } else {
            (Some(scale * 100.0), None::<f32>)
        };

        sqlx::query_scalar::<_, i32>(
            "INSERT INTO log_entries
                 (meal_log_id, recipe_id, amount_g, amount_ml, kcal_snapshot, nutrient_snapshot, idempotency_key)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (idempotency_key) DO UPDATE
                 SET kcal_snapshot = log_entries.kcal_snapshot
             RETURNING id",
        )
        .bind(meal_log_id)
        .bind(recipe_id)
        .bind(amount_g_db)
        .bind(amount_ml_db)
        .bind(total_kcal)
        .bind(&nutrient_snapshot)
        .bind(&idempotency_key)
        .fetch_one(&self.state.db)
        .await
        .mcp_err()?;

        Ok(format!(
            "Logged recipe '{}' ({:.1} kcal, {scale} serving(s)) to {meal_type} on {}.",
            recipe.name,
            total_kcal,
            ts.format("%Y-%m-%d")
        ))
    }

    /// List recent log entries, optionally filtered by date, meal type, or tags.
    #[tool("List recent food log entries. Returns meal_log_id values (for get_meal_summary), food names, amounts, and kcal. Filter by date, meal type, or tags.")]
    async fn list_recent_logs(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("Filter to a specific date (YYYY-MM-DD). Omit for all recent entries.")]
        date: Option<String>,
        #[description("Filter by meal name (e.g. breakfast, lunch, dinner, snack)")]
        meal_type: Option<String>,
        #[description("Filter by log entry tag (attached via log_food tags parameter)")]
        tag: Option<String>,
        #[description("Filter by food item tag (attached via tag_food)")]
        food_tag: Option<String>,
        #[description("Maximum number of entries to return (default 20, max 100)")]
        limit: Option<u32>,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;
        let max: i64 = limit
            .unwrap_or(self.state.config.log_list_default_limit)
            .min(self.state.config.log_list_max_limit) as i64;

        let date_filter: Option<NaiveDate> = date
            .as_deref()
            .map(|s| parse_date(s, "date"))
            .transpose()?;

        let rows = sqlx::query_as::<_, RecentLogRow>(
            "SELECT
                 le.id          AS entry_id,
                 ml.id          AS meal_log_id,
                 ml.meal_type,
                 ml.logged_at,
                 fi.canonical_name AS food_name,
                 r.name         AS recipe_name,
                 le.amount_g,
                 le.amount_ml,
                 le.kcal_snapshot
             FROM log_entries le
             JOIN meal_logs ml ON ml.id = le.meal_log_id
             LEFT JOIN food_items fi ON fi.id = le.food_id
             LEFT JOIN recipes r ON r.id = le.recipe_id
             WHERE ml.user_id = $1
               AND ($2::date IS NULL
                    OR (ml.logged_at AT TIME ZONE $3)::date = $2::date)
               AND ($4::text IS NULL OR ml.meal_type = $4)
               AND ($5::text IS NULL
                    OR EXISTS (
                        SELECT 1 FROM log_entry_tags et
                        WHERE et.log_entry_id = le.id AND et.tag = $5))
               AND ($6::text IS NULL
                    OR EXISTS (
                        SELECT 1 FROM food_item_tags ft
                        WHERE ft.food_id = le.food_id AND ft.tag = $6
                          AND (ft.user_id = $1 OR ft.user_id IS NULL)))
             ORDER BY ml.logged_at DESC, le.id DESC
             LIMIT $7",
        )
        .bind(user.id)
        .bind(date_filter)
        .bind(&user.timezone)
        .bind(meal_type.as_deref())
        .bind(tag.as_deref())
        .bind(food_tag.as_deref())
        .bind(max)
        .fetch_all(&self.state.db)
        .await
        .mcp_err()?;

        let entries: Vec<JsonValue> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "log_entry_id": r.entry_id,
                    "meal_log_id":  r.meal_log_id,
                    "meal_type":    r.meal_type,
                    "logged_at":    r.logged_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                    "food_name":    r.food_name,
                    "recipe_name":  r.recipe_name,
                    "amount_g":     r.amount_g,
                    "amount_ml":    r.amount_ml,
                    "kcal":         r.kcal_snapshot,
                })
            })
            .collect();

        let result = serde_json::json!({
            "user":    user.username,
            "entries": entries,
            "count":   entries.len(),
        });

        serde_json::to_string_pretty(&result).mcp_err()
    }

    // ─── Recipes ──────────────────────────────────────────────────────────────

    /// Create a named recipe from a list of food ingredients with amounts.
    #[tool("Create a named recipe from food ingredients for repeated composite meals. Returns a recipe_id for use with log_recipe. food_id values come from search_food.")]
    async fn create_recipe(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("Name for the recipe (e.g. 'Morning Oatmeal Bowl')")]
        name: String,
        #[description("Ingredients as a JSON array: [{\"food_id\":123,\"amount\":200,\"unit\":\"g\"},{\"food_id\":456,\"amount\":1,\"unit\":\"cup\"}]. food_id values come from search_food.")]
        ingredients: String,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;

        let inputs: Vec<IngredientInput> = serde_json::from_str(&ingredients)
            .map_err(|e| McpError::internal(format!(
                "ingredients must be a JSON array like \
                 [{{\"food_id\":123,\"amount\":200,\"unit\":\"g\"}}]: {e}"
            )))?;

        if inputs.is_empty() {
            return Err(McpError::internal("ingredients array must not be empty"));
        }

        let recipe_id: i32 = sqlx::query_scalar::<_, i32>(
            "INSERT INTO recipes (user_id, name) VALUES ($1, $2) RETURNING id",
        )
        .bind(user.id)
        .bind(&name)
        .fetch_one(&self.state.db)
        .await
        .mcp_err()?;

        for ing in &inputs {
            let food_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM food_items WHERE id = $1)",
            )
            .bind(ing.food_id)
            .fetch_one(&self.state.db)
            .await
            .mcp_err()?;

            if !food_exists {
                return Err(McpError::internal(format!(
                    "food_id {} not found; use search_food or confirm_food first",
                    ing.food_id
                )));
            }

            let parsed = units::parse_amount(ing.amount, &ing.unit);
            let resolved = match parsed {
                ParsedAmount::Named { ref label, count } => {
                    units::resolve_named_portion(&self.state.db, ing.food_id, label, count)
                        .await
                        .mcp_err()?
                        .ok_or_else(|| McpError::internal(format!(
                            "Unit '{}' not found for food_id {}",
                            ing.unit, ing.food_id
                        )))?
                }
                other => other,
            };

            let (amount_g, amount_ml) = resolved_to_db_amounts(&resolved);

            sqlx::query(
                "INSERT INTO recipe_ingredients (recipe_id, food_id, amount_g, amount_ml)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (recipe_id, food_id) DO UPDATE
                     SET amount_g = EXCLUDED.amount_g, amount_ml = EXCLUDED.amount_ml",
            )
            .bind(recipe_id)
            .bind(ing.food_id)
            .bind(amount_g)
            .bind(amount_ml)
            .execute(&self.state.db)
            .await
            .mcp_err()?;
        }

        Ok(format!(
            "Recipe '{name}' created (id {recipe_id}) with {} ingredient(s).",
            inputs.len()
        ))
    }

    // ─── Exercise ─────────────────────────────────────────────────────────────

    /// Log an exercise session with calories burned.
    #[tool("Log an exercise session with estimated or confirmed calories burned. Burned calories are subtracted from net_kcal in get_daily_summary.")]
    async fn log_exercise(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("Activity description (e.g. '30 min running', '45 min weight training')")]
        description: String,
        #[description("Estimated kilocalories burned")]
        calories_burned: f32,
        #[description("How the calorie estimate was obtained: 'user' (manually entered, default), 'llm_estimated', or 'device' (wearable/app)")]
        source: Option<String>,
        #[description("When exercise occurred: RFC 3339 ('2024-01-15T08:30:00Z') or date only ('2024-01-15'). Defaults to now.")]
        logged_at: Option<String>,
        #[description("Optional free-text note")]
        note: Option<String>,
        #[description("Unique string for this logging intent. Safe to retry \u{2014} duplicate keys are ignored.")]
        idempotency_key: String,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;
        let ts = parse_logged_at(logged_at.as_deref())?;
        let source_str = source.as_deref().unwrap_or("user");

        if !["user", "llm_estimated", "device"].contains(&source_str) {
            return Err(McpError::internal(format!(
                "Invalid source '{source_str}'. Must be one of: user, llm_estimated, device."
            )));
        }

        sqlx::query(
            "INSERT INTO exercise_logs
                 (user_id, logged_at, description, calories_burned, source, idempotency_key, note)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (idempotency_key) DO NOTHING",
        )
        .bind(user.id)
        .bind(ts)
        .bind(&description)
        .bind(calories_burned)
        .bind(source_str)
        .bind(&idempotency_key)
        .bind(note.as_deref())
        .execute(&self.state.db)
        .await
        .mcp_err()?;

        Ok(format!(
            "Exercise '{description}' logged ({calories_burned} kcal burned) on {}.",
            ts.format("%Y-%m-%d")
        ))
    }

    // ─── Weight ───────────────────────────────────────────────────────────────

    /// Log a body weight measurement.
    #[tool("Log a body weight measurement in kilograms. History is retrievable with get_weight_history.")]
    async fn log_weight(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("Body weight in kilograms (e.g. 72.5)")]
        weight_kg: f32,
        #[description("When weight was measured: RFC 3339 or date only (YYYY-MM-DD). Defaults to now.")]
        logged_at: Option<String>,
        #[description("Optional note (e.g. 'morning, fasted', 'after gym')")]
        note: Option<String>,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;
        let ts = parse_logged_at(logged_at.as_deref())?;

        sqlx::query(
            "INSERT INTO weight_logs (user_id, logged_at, weight_kg, note)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(user.id)
        .bind(ts)
        .bind(weight_kg)
        .bind(note.as_deref())
        .execute(&self.state.db)
        .await
        .mcp_err()?;

        Ok(format!("Weight {weight_kg:.1} kg logged on {}.", ts.format("%Y-%m-%d")))
    }

    /// Retrieve weight entries over a date range.
    #[tool("Retrieve body weight history for a date range. Returns chronological list of weight_kg entries with timestamps.")]
    async fn get_weight_history(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("Start of date range (YYYY-MM-DD, inclusive)")]
        start_date: String,
        #[description("End of date range (YYYY-MM-DD, inclusive). Defaults to today.")]
        end_date: Option<String>,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;

        let start = parse_date(&start_date, "start_date")?;
        let end = end_date
            .as_deref()
            .map(|s| parse_date(s, "end_date"))
            .transpose()?
            .unwrap_or_else(|| Utc::now().date_naive());

        let rows = sqlx::query_as::<_, liberado_core::models::WeightLog>(
            "SELECT id, user_id, logged_at, weight_kg, note
             FROM weight_logs
             WHERE user_id = $1
               AND (logged_at AT TIME ZONE $2)::date BETWEEN $3 AND $4
             ORDER BY logged_at ASC",
        )
        .bind(user.id)
        .bind(&user.timezone)
        .bind(start)
        .bind(end)
        .fetch_all(&self.state.db)
        .await
        .mcp_err()?;

        let entries: Vec<JsonValue> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "date":      r.logged_at.format("%Y-%m-%d").to_string(),
                    "time":      r.logged_at.format("%H:%M:%SZ").to_string(),
                    "weight_kg": r.weight_kg,
                    "note":      r.note,
                })
            })
            .collect();

        let result = serde_json::json!({
            "user":       user.username,
            "start_date": start.to_string(),
            "end_date":   end.to_string(),
            "count":      entries.len(),
            "entries":    entries,
        });

        serde_json::to_string_pretty(&result).mcp_err()
    }

    // ─── Summaries ────────────────────────────────────────────────────────────

    /// All nutrient totals for a user on a given date, compared against their goals.
    #[tool("Return total kcal, macros, exercise burned, net kcal, and per-meal breakdown for a given date. Includes goal comparison if set_goals has been called.")]
    async fn get_daily_summary(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("Date to summarize (YYYY-MM-DD). Defaults to today.")]
        date: Option<String>,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;
        let target_date = parse_date_param(date.as_deref())?;

        // All log entries for the day
        let entries: Vec<(f32, Option<JsonValue>, String)> = sqlx::query_as::<
            _,
            (f32, Option<JsonValue>, String),
        >(
            "SELECT le.kcal_snapshot, le.nutrient_snapshot, ml.meal_type
             FROM log_entries le
             JOIN meal_logs ml ON ml.id = le.meal_log_id
             WHERE ml.user_id = $1
               AND (ml.logged_at AT TIME ZONE $2)::date = $3",
        )
        .bind(user.id)
        .bind(&user.timezone)
        .bind(target_date)
        .fetch_all(&self.state.db)
        .await
        .mcp_err()?;

        let total_kcal: f32 = entries.iter().map(|(k, _, _)| k).sum();

        let mut nutrients: std::collections::HashMap<String, f32> = Default::default();
        let mut meal_kcal: std::collections::HashMap<String, f32> = Default::default();

        for (kcal, snapshot, meal_type) in &entries {
            *meal_kcal.entry(meal_type.clone()).or_default() += kcal;
            if let Some(JsonValue::Object(map)) = snapshot {
                for (k, v) in map {
                    if let Some(f) = v.as_f64() {
                        *nutrients.entry(k.clone()).or_default() += f as f32;
                    }
                }
            }
        }

        // Exercise burned for the day
        let burned: Option<f32> = sqlx::query_scalar(
            "SELECT SUM(calories_burned)
             FROM exercise_logs
             WHERE user_id = $1
               AND (logged_at AT TIME ZONE $2)::date = $3",
        )
        .bind(user.id)
        .bind(&user.timezone)
        .bind(target_date)
        .fetch_one(&self.state.db)
        .await
        .mcp_err()?;
        let burned = burned.unwrap_or(0.0);

        // Most recent applicable goal
        let goal = sqlx::query_as::<_, liberado_core::models::UserGoal>(
            "SELECT id, user_id, effective_from, kcal_target, protein_g, fat_g, carbs_g, fiber_g, water_ml
             FROM user_goals
             WHERE user_id = $1 AND effective_from <= $2
             ORDER BY effective_from DESC
             LIMIT 1",
        )
        .bind(user.id)
        .bind(target_date)
        .fetch_optional(&self.state.db)
        .await
        .mcp_err()?;

        let result = serde_json::json!({
            "date":  target_date.to_string(),
            "user":  user.username,
            "consumed": {
                "kcal":       total_kcal,
                "protein_g":  nutrients.get("protein").copied().unwrap_or(0.0),
                "fat_g":      nutrients.get("fat_total").copied().unwrap_or(0.0),
                "carbs_g":    nutrients.get("carbohydrates").copied().unwrap_or(0.0),
                "fiber_g":    nutrients.get("fiber").copied().unwrap_or(0.0),
                "water_ml":   nutrients.get("water").copied().unwrap_or(0.0),
                "nutrients":  nutrients,
            },
            "exercise_burned": burned,
            "net_kcal": total_kcal - burned,
            "meals":    meal_kcal,
            "goals": goal.map(|g| serde_json::json!({
                "kcal_target":    g.kcal_target,
                "protein_g":      g.protein_g,
                "fat_g":          g.fat_g,
                "carbs_g":        g.carbs_g,
                "fiber_g":        g.fiber_g,
                "water_ml":       g.water_ml,
                "effective_from": g.effective_from.to_string(),
            })),
        });

        serde_json::to_string_pretty(&result).mcp_err()
    }

    /// Net calories for a user on a given date: calories consumed minus calories burned.
    #[tool("Return kcal_consumed, kcal_burned, and kcal_net for a given date. Lightweight alternative to get_daily_summary when only the calorie balance is needed.")]
    async fn get_net_calories(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("Date to calculate (YYYY-MM-DD). Defaults to today.")]
        date: Option<String>,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;
        let target_date = parse_date_param(date.as_deref())?;

        let consumed: Option<f32> = sqlx::query_scalar(
            "SELECT SUM(le.kcal_snapshot)
             FROM log_entries le
             JOIN meal_logs ml ON ml.id = le.meal_log_id
             WHERE ml.user_id = $1
               AND (ml.logged_at AT TIME ZONE $2)::date = $3",
        )
        .bind(user.id)
        .bind(&user.timezone)
        .bind(target_date)
        .fetch_one(&self.state.db)
        .await
        .mcp_err()?;
        let consumed = consumed.unwrap_or(0.0);

        let burned: Option<f32> = sqlx::query_scalar(
            "SELECT SUM(calories_burned)
             FROM exercise_logs
             WHERE user_id = $1
               AND (logged_at AT TIME ZONE $2)::date = $3",
        )
        .bind(user.id)
        .bind(&user.timezone)
        .bind(target_date)
        .fetch_one(&self.state.db)
        .await
        .mcp_err()?;
        let burned = burned.unwrap_or(0.0);

        let result = serde_json::json!({
            "date":          target_date.to_string(),
            "kcal_consumed": consumed,
            "kcal_burned":   burned,
            "kcal_net":      consumed - burned,
        });

        serde_json::to_string_pretty(&result).mcp_err()
    }

    /// Nutrient totals for a single meal log.
    #[tool("Return per-item breakdown and nutrient totals for one meal. meal_log_id comes from list_recent_logs results.")]
    async fn get_meal_summary(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("ID of the meal log to detail; from list_recent_logs results")]
        meal_log_id: i32,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;

        // Verify ownership
        let meal = sqlx::query_as::<_, liberado_core::models::MealLog>(
            "SELECT id, user_id, logged_at, meal_type, note
             FROM meal_logs WHERE id = $1",
        )
        .bind(meal_log_id)
        .fetch_optional(&self.state.db)
        .await
        .mcp_err()?
        .ok_or_else(|| McpError::internal(format!("meal_log {meal_log_id} not found")))?;

        if meal.user_id != user.id {
            return Err(McpError::internal("unauthorized: meal log belongs to another user"));
        }

        let entries = sqlx::query_as::<_, LogEntryDetail>(
            "SELECT
                 le.id,
                 le.kcal_snapshot,
                 le.amount_g,
                 le.amount_ml,
                 le.nutrient_snapshot,
                 fi.canonical_name AS food_name,
                 r.name            AS recipe_name
             FROM log_entries le
             LEFT JOIN food_items fi ON fi.id = le.food_id
             LEFT JOIN recipes r ON r.id = le.recipe_id
             WHERE le.meal_log_id = $1",
        )
        .bind(meal_log_id)
        .fetch_all(&self.state.db)
        .await
        .mcp_err()?;

        let total_kcal: f32 = entries.iter().map(|e| e.kcal_snapshot).sum();
        let mut nutrients: std::collections::HashMap<String, f32> = Default::default();
        for entry in &entries {
            if let Some(JsonValue::Object(map)) = &entry.nutrient_snapshot {
                for (k, v) in map {
                    if let Some(f) = v.as_f64() {
                        *nutrients.entry(k.clone()).or_default() += f as f32;
                    }
                }
            }
        }

        let entry_list: Vec<JsonValue> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "log_entry_id": e.id,
                    "food_name":    e.food_name,
                    "recipe_name":  e.recipe_name,
                    "amount_g":     e.amount_g,
                    "amount_ml":    e.amount_ml,
                    "kcal":         e.kcal_snapshot,
                })
            })
            .collect();

        let result = serde_json::json!({
            "meal_log_id": meal_log_id,
            "meal_type":   meal.meal_type,
            "logged_at":   meal.logged_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "total_kcal":  total_kcal,
            "nutrients":   nutrients,
            "entries":     entry_list,
        });

        serde_json::to_string_pretty(&result).mcp_err()
    }

    // ─── Goals ────────────────────────────────────────────────────────────────

    /// Set or update daily nutrition targets for a user.
    #[tool("Set daily nutrition targets (kcal, macros, fiber, water). Goals are versioned by date \u{2014} omit effective_from to apply from today. Only supplied fields are updated; existing values are preserved.")]
    async fn set_goals(
        &self,
        #[description("API key for authentication; omit when LIBERADO_DEFAULT_API_KEY is set on the server")]
        api_key: Option<String>,
        #[description("Daily calorie target in kcal (e.g. 2000)")]
        kcal_target: Option<f32>,
        #[description("Daily protein target in grams")]
        protein_g: Option<f32>,
        #[description("Daily fat target in grams")]
        fat_g: Option<f32>,
        #[description("Daily carbohydrates target in grams")]
        carbs_g: Option<f32>,
        #[description("Daily fiber target in grams")]
        fiber_g: Option<f32>,
        #[description("Daily water intake target in ml")]
        water_ml: Option<f32>,
        #[description("Date these goals take effect (YYYY-MM-DD). Defaults to today. Prior goals are preserved for historical summaries.")]
        effective_from: Option<String>,
    ) -> McpResult<String> {
        let user = self.resolve_user(api_key.as_deref().unwrap_or("")).await?;

        let date = effective_from
            .as_deref()
            .map(|s| parse_date(s, "effective_from"))
            .transpose()?
            .unwrap_or_else(|| Utc::now().date_naive());

        // On conflict, only update fields that were explicitly provided (COALESCE preserves existing)
        sqlx::query(
            "INSERT INTO user_goals
                 (user_id, effective_from, kcal_target, protein_g, fat_g, carbs_g, fiber_g, water_ml)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (user_id, effective_from) DO UPDATE SET
                 kcal_target = COALESCE(EXCLUDED.kcal_target, user_goals.kcal_target),
                 protein_g   = COALESCE(EXCLUDED.protein_g,   user_goals.protein_g),
                 fat_g       = COALESCE(EXCLUDED.fat_g,       user_goals.fat_g),
                 carbs_g     = COALESCE(EXCLUDED.carbs_g,     user_goals.carbs_g),
                 fiber_g     = COALESCE(EXCLUDED.fiber_g,     user_goals.fiber_g),
                 water_ml    = COALESCE(EXCLUDED.water_ml,    user_goals.water_ml)",
        )
        .bind(user.id)
        .bind(date)
        .bind(kcal_target)
        .bind(protein_g)
        .bind(fat_g)
        .bind(carbs_g)
        .bind(fiber_g)
        .bind(water_ml)
        .execute(&self.state.db)
        .await
        .mcp_err()?;

        Ok(format!("Goals set effective from {date}."))
    }
}

// ─── Private helpers ──────────────────────────────────────────────────────────

impl LiberadoServer {
    /// Resolves an API key to a User. Falls back to `config.default_api_key`
    /// when the caller supplies an empty key and the env var is set.
    ///
    /// Fast path: direct WHERE lookup on the sha256 hash (microseconds, no blocking).
    /// Slow path: argon2 scan for any user whose hash hasn't been migrated yet;
    ///            on success, upgrades the stored hash so future calls take the fast path.
    async fn resolve_user(
        &self,
        api_key: &str,
    ) -> McpResult<liberado_core::models::User> {
        let key = if api_key.is_empty() {
            self.state.config.default_api_key.as_str()
        } else {
            api_key
        };

        if key.is_empty() {
            return Err(McpError::internal("unauthorized: no API key provided"));
        }

        let sha256_hash = hex::encode(sha2::Sha256::digest(key.as_bytes()));

        // Fast path: direct index lookup, no blocking (microseconds).
        if let Some(user) = sqlx::query_as::<_, liberado_core::models::User>(
            "SELECT id, username, api_key_hash, timezone, created_at \
             FROM users WHERE api_key_hash = $1",
        )
        .bind(&sha256_hash)
        .fetch_optional(&self.state.db)
        .await
        .mcp_err()?
        {
            return Ok(user);
        }

        // Slow path: argon2 fallback for hashes not yet migrated to sha256.
        // Self-eliminating: upgrades the stored hash on first successful auth.
        use argon2::{Argon2, PasswordHash, PasswordVerifier};

        let legacy_users = sqlx::query_as::<_, liberado_core::models::User>(
            "SELECT id, username, api_key_hash, timezone, created_at \
             FROM users WHERE api_key_hash LIKE '$argon2%'",
        )
        .fetch_all(&self.state.db)
        .await
        .mcp_err()?;

        if legacy_users.is_empty() {
            return Err(McpError::internal("unauthorized: invalid API key"));
        }

        let key_owned = key.to_owned();
        let matched = tokio::task::spawn_blocking(move || {
            for user in legacy_users {
                if let Ok(hash) = PasswordHash::new(&user.api_key_hash)
                    && Argon2::default()
                        .verify_password(key_owned.as_bytes(), &hash)
                        .is_ok()
                {
                    return Some(user);
                }
            }
            None
        })
        .await
        .map_err(|e| McpError::internal(format!("argon2 verify task panicked: {e}")))?;

        let Some(user) = matched else {
            return Err(McpError::internal("unauthorized: invalid API key"));
        };

        // Upgrade the stored hash to sha256 so future auths skip this slow path.
        let _ = sqlx::query("UPDATE users SET api_key_hash = $1 WHERE id = $2")
            .bind(&sha256_hash)
            .bind(user.id)
            .execute(&self.state.db)
            .await;

        Ok(user)
    }
}

// ─── Free helper functions ────────────────────────────────────────────────────

/// Parses an optional timestamp string (RFC 3339 or YYYY-MM-DD). Defaults to now().
fn parse_logged_at(s: Option<&str>) -> McpResult<DateTime<Utc>> {
    let Some(s) = s else { return Ok(Utc::now()) };

    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(date.and_hms_opt(0, 0, 0).unwrap().and_utc());
    }
    Err(McpError::internal(format!(
        "invalid logged_at '{s}': expected RFC 3339 (2024-01-15T12:00:00Z) or YYYY-MM-DD"
    )))
}

/// Parses a YYYY-MM-DD date string, attributing errors to `field` in the message.
fn parse_date(s: &str, field: &str) -> McpResult<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| McpError::internal(format!("invalid {field} '{s}': {e}")))
}

/// Parses an optional YYYY-MM-DD date string. Defaults to today in UTC.
fn parse_date_param(s: Option<&str>) -> McpResult<NaiveDate> {
    match s {
        Some(s) => parse_date(s, "date"),
        None => Ok(Utc::now().date_naive()),
    }
}

/// Finds an existing meal_log for the same user/meal_type/calendar-date or creates one.
/// Date comparison is done in the user's stored timezone via PostgreSQL AT TIME ZONE.
async fn find_or_create_meal_log(
    pool: &sqlx::PgPool,
    user_id: i32,
    ts: &DateTime<Utc>,
    meal_type: &str,
    tz: &str,
) -> McpResult<i32> {
    let existing = sqlx::query_scalar::<_, i32>(
        "SELECT id FROM meal_logs
         WHERE user_id = $1
           AND meal_type = $2
           AND (logged_at AT TIME ZONE $3)::date = ($4 AT TIME ZONE $3)::date
         LIMIT 1",
    )
    .bind(user_id)
    .bind(meal_type)
    .bind(tz)
    .bind(*ts)
    .fetch_optional(pool)
    .await
    .mcp_err()?;

    if let Some(id) = existing {
        return Ok(id);
    }

    sqlx::query_scalar::<_, i32>(
        "INSERT INTO meal_logs (user_id, logged_at, meal_type) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(user_id)
    .bind(*ts)
    .bind(meal_type)
    .fetch_one(pool)
    .await
    .mcp_err()
}

/// Fetches all nutrient values for a food, scales them to the logged amount,
/// and returns a JSONB-ready map of { nutrient_name: scaled_value }.
async fn build_nutrient_snapshot(
    pool: &sqlx::PgPool,
    food_id: i32,
    resolved: &ParsedAmount,
    basis: &str,
) -> McpResult<JsonValue> {
    let rows: Vec<(String, f32)> = sqlx::query_as::<_, (String, f32)>(
        "SELECT n.name, fnv.value
         FROM food_nutrient_values fnv
         JOIN nutrients n ON n.id = fnv.nutrient_id
         WHERE fnv.food_id = $1",
    )
    .bind(food_id)
    .fetch_all(pool)
    .await
    .mcp_err()?;

    let mut map = serde_json::Map::new();
    for (name, value_per_100) in rows {
        let scaled = units::scale_nutrient(value_per_100, resolved, basis);
        if scaled > 0.0 {
            map.insert(name, serde_json::json!(scaled));
        }
    }
    Ok(JsonValue::Object(map))
}

/// Splits a resolved ParsedAmount into (amount_g, amount_ml) for DB storage.
fn resolved_to_db_amounts(resolved: &ParsedAmount) -> (Option<f32>, Option<f32>) {
    match resolved {
        ParsedAmount::Grams(g)       => (Some(*g), None),
        ParsedAmount::Milliliters(ml) => (None, Some(*ml)),
        ParsedAmount::Named { .. }   => (None, None), // should never reach DB
    }
}

// ─── Local helper structs ─────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct IngredientInput {
    food_id: i32,
    amount:  f32,
    unit:    String,
}

#[derive(serde::Deserialize)]
struct PortionInput {
    unit:   String,
    grams:  Option<f32>,
    ml:     Option<f32>,
}

#[derive(sqlx::FromRow)]
struct LogEntryDetail {
    id:                i32,
    kcal_snapshot:     f32,
    amount_g:          Option<f32>,
    amount_ml:         Option<f32>,
    nutrient_snapshot: Option<JsonValue>,
    food_name:         Option<String>,
    recipe_name:       Option<String>,
}

#[derive(sqlx::FromRow)]
struct RecentLogRow {
    entry_id:      i32,
    meal_log_id:   i32,
    meal_type:     String,
    logged_at:     DateTime<Utc>,
    food_name:     Option<String>,
    recipe_name:   Option<String>,
    amount_g:      Option<f32>,
    amount_ml:     Option<f32>,
    kcal_snapshot: f32,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_logged_at_rfc3339() {
        let dt = parse_logged_at(Some("2024-01-15T12:30:00Z")).unwrap();
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2024-01-15");
    }

    #[test]
    fn parse_logged_at_date_only() {
        let dt = parse_logged_at(Some("2024-03-20")).unwrap();
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2024-03-20");
        // Should be midnight UTC
        assert_eq!(dt.format("%H:%M:%S").to_string(), "00:00:00");
    }

    #[test]
    fn parse_logged_at_none_returns_now() {
        let before = Utc::now();
        let dt = parse_logged_at(None).unwrap();
        let after = Utc::now();
        assert!(dt >= before && dt <= after);
    }

    #[test]
    fn parse_logged_at_invalid_errors() {
        assert!(parse_logged_at(Some("not-a-date")).is_err());
        assert!(parse_logged_at(Some("2024/01/15")).is_err());
    }

    #[test]
    fn parse_date_valid() {
        let d = parse_date("2024-06-01", "foo").unwrap();
        assert_eq!(d.to_string(), "2024-06-01");
    }

    #[test]
    fn parse_date_invalid_includes_field_name() {
        let err = parse_date("not-a-date", "start_date").unwrap_err();
        assert!(format!("{err:?}").contains("start_date"));
    }

    #[test]
    fn parse_date_invalid_includes_value() {
        let err = parse_date("2024-13-01", "end_date").unwrap_err();
        assert!(format!("{err:?}").contains("2024-13-01"));
    }

    #[test]
    fn parse_date_param_valid() {
        let d = parse_date_param(Some("2024-06-01")).unwrap();
        assert_eq!(d.to_string(), "2024-06-01");
    }

    #[test]
    fn parse_date_param_none_returns_today() {
        let today = Utc::now().date_naive();
        let d = parse_date_param(None).unwrap();
        assert_eq!(d, today);
    }

    #[test]
    fn parse_date_param_invalid_errors() {
        assert!(parse_date_param(Some("2024-13-01")).is_err());
    }

    #[test]
    fn resolved_to_db_amounts_grams() {
        let (g, ml) = resolved_to_db_amounts(&ParsedAmount::Grams(150.0));
        assert_eq!(g, Some(150.0));
        assert_eq!(ml, None);
    }

    #[test]
    fn resolved_to_db_amounts_milliliters() {
        let (g, ml) = resolved_to_db_amounts(&ParsedAmount::Milliliters(250.0));
        assert_eq!(g, None);
        assert_eq!(ml, Some(250.0));
    }

    #[test]
    fn ingredient_input_deserializes() {
        let json = r#"[{"food_id":123,"amount":200.0,"unit":"g"},{"food_id":456,"amount":1.5,"unit":"cup"}]"#;
        let inputs: Vec<IngredientInput> = serde_json::from_str(json).unwrap();
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].food_id, 123);
        assert!((inputs[0].amount - 200.0).abs() < 0.01);
        assert_eq!(inputs[1].unit, "cup");
    }

    #[test]
    fn ingredient_input_empty_array() {
        let inputs: Vec<IngredientInput> = serde_json::from_str("[]").unwrap();
        assert!(inputs.is_empty());
    }

    #[test]
    fn ingredient_input_invalid_json_errors() {
        let result: Result<Vec<IngredientInput>, _> = serde_json::from_str("not json");
        assert!(result.is_err());
    }

    #[test]
    fn portion_input_deserializes_grams_and_ml() {
        let json = r#"[{"unit":"cup","grams":90.0},{"unit":"tbsp","ml":15.0}]"#;
        let inputs: Vec<PortionInput> = serde_json::from_str(json).unwrap();
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].unit, "cup");
        assert_eq!(inputs[0].grams, Some(90.0));
        assert!(inputs[0].ml.is_none());
        assert_eq!(inputs[1].unit, "tbsp");
        assert_eq!(inputs[1].ml, Some(15.0));
        assert!(inputs[1].grams.is_none());
    }

    #[test]
    fn portion_input_empty_array() {
        let inputs: Vec<PortionInput> = serde_json::from_str("[]").unwrap();
        assert!(inputs.is_empty());
    }

    #[test]
    fn resolved_to_db_amounts_named() {
        let (g, ml) = resolved_to_db_amounts(&ParsedAmount::Named {
            label: "cup".into(),
            count: 1.0,
        });
        assert_eq!(g, None);
        assert_eq!(ml, None);
    }

    #[test]
    fn parse_logged_at_rfc3339_with_positive_offset() {
        // 12:30 at UTC+5 \u{2192} 07:30 UTC
        let dt = parse_logged_at(Some("2024-01-15T12:30:00+05:00")).unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M:%S").to_string(), "2024-01-15 07:30:00");
    }

    #[test]
    fn parse_logged_at_rfc3339_with_negative_offset() {
        // 12:00 at UTC-8 \u{2192} 20:00 UTC
        let dt = parse_logged_at(Some("2024-06-01T12:00:00-08:00")).unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M:%S").to_string(), "2024-06-01 20:00:00");
    }

    #[test]
    fn parse_date_param_rejects_wrong_format() {
        assert!(parse_date_param(Some("01/15/2024")).is_err());
        assert!(parse_date_param(Some("20240115")).is_err());
        assert!(parse_date_param(Some("Jan 15, 2024")).is_err());
    }
}
