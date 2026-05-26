use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{debug, warn};

use crate::error::{Error, Result};

// ─── Public response types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct FoodMatch {
    pub food_id: i32,
    pub name: String,
    /// "per_100g" or "per_100ml"
    pub basis: String,
    pub kcal_per_100: f32,
    pub confidence: String,
    pub source: String,
    pub score: f32,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    /// True when a single result exceeds the strong-match threshold and can be
    /// used without asking the user to confirm. False means the LLM should
    /// present candidates and ask which one to use.
    pub auto_selected: bool,
    pub matches: Vec<FoodMatch>,
    pub query: String,
    /// True when no match was found anywhere; the LLM should prompt the user
    /// for nutrition data or fall back to the advisor LLM estimator.
    pub fallback_required: bool,
    pub message: String,
}

// ─── Internal DB row types ─────────────────────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct LocalSearchRow {
    pub id: i32,
    pub canonical_name: String,
    pub basis: String,
    pub source: String,
    pub confidence: String,
    #[allow(dead_code)]
    created_at: DateTime<Utc>,
    #[allow(dead_code)]
    updated_at: DateTime<Utc>,
    pub score: f32,
    pub kcal_per_100: f32,
}

// ─── USDA FoodData Central API types ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct UsdaSearchResponse {
    foods: Vec<UsdaFood>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsdaFood {
    pub fdc_id: i32,
    pub description: String,
    #[serde(default)]
    pub food_nutrients: Vec<UsdaNutrient>,
    /// Present only when deserialised from the search endpoint; empty otherwise.
    #[serde(default)]
    pub food_portions: Vec<UsdaPortion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsdaNutrient {
    pub nutrient_id: i32,
    pub value: Option<f64>,
}

/// Named serving size from the USDA detail endpoint (e.g. "1 cup = 81g").
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsdaPortion {
    pub amount: Option<f64>,
    pub gram_weight: Option<f64>,
    pub modifier: Option<String>,
}

// ─── USDA detail-endpoint types ───────────────────────────────────────────────
// The detail endpoint returns nutrients in a different schema than the search
// endpoint, so we use a separate set of structs and expose a conversion helper.

#[derive(Debug, Deserialize)]
struct UsdaDetailNutrient {
    nutrient: UsdaDetailNutrientInner,
    amount: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct UsdaDetailNutrientInner {
    id: i32,
}

/// Full response from `/fdc/v1/food/{fdc_id}`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsdaDetailResponse {
    pub fdc_id: i32,
    pub description: String,
    #[serde(default)]
    food_nutrients: Vec<UsdaDetailNutrient>,
    #[serde(default)]
    pub food_portions: Vec<UsdaPortion>,
}

impl UsdaDetailResponse {
    /// Converts detail-format nutrients into the canonical form used by
    /// `insert_usda_nutrients`.
    pub fn to_usda_nutrients(&self) -> Vec<UsdaNutrient> {
        self.food_nutrients
            .iter()
            .map(|n| UsdaNutrient { nutrient_id: n.nutrient.id, value: n.amount })
            .collect()
    }
}

// ─── Open Food Facts API types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OffSearchResponse {
    products: Vec<OffProduct>,
}

#[derive(Debug, Deserialize)]
pub struct OffProduct {
    pub id: Option<String>,
    pub product_name: Option<String>,
    pub nutriments: Option<OffNutriments>,
    /// Serving size quantity (e.g. 60 for "60 g").
    pub serving_quantity: Option<f64>,
    /// Unit for `serving_quantity`: "g" or "ml".
    pub serving_quantity_unit: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct OffNutriments {
    #[serde(rename = "energy-kcal_100g")]
    pub energy_kcal_100g: Option<f64>,
    #[serde(rename = "proteins_100g")]
    pub proteins_100g: Option<f64>,
    #[serde(rename = "fat_100g")]
    pub fat_100g: Option<f64>,
    #[serde(rename = "carbohydrates_100g")]
    pub carbohydrates_100g: Option<f64>,
    #[serde(rename = "fiber_100g")]
    pub fiber_100g: Option<f64>,
    #[serde(rename = "sugars_100g")]
    pub sugars_100g: Option<f64>,
    #[serde(rename = "sodium_100g")]
    pub sodium_100g: Option<f64>,
    #[serde(rename = "calcium_100g")]
    pub calcium_100g: Option<f64>,
    #[serde(rename = "iron_100g")]
    pub iron_100g: Option<f64>,
    #[serde(rename = "caffeine_100g")]
    pub caffeine_100g: Option<f64>,
}

// ─── Local DB search ──────────────────────────────────────────────────────────

/// Searches the local food_items cache using pg_trgm similarity against both
/// canonical names and user aliases. Returns rows ordered by score descending.
pub(crate) async fn search_local(
    pool: &PgPool,
    query: &str,
    limit: u32,
    min_score: f32,
) -> Result<Vec<LocalSearchRow>> {
    sqlx::query_as::<_, LocalSearchRow>(
        r#"
        SELECT
            f.id,
            f.canonical_name,
            f.basis,
            f.source,
            f.confidence,
            f.created_at,
            f.updated_at,
            GREATEST(
                similarity(f.canonical_name, $1),
                COALESCE(
                    (SELECT MAX(similarity(fa.alias, $1))
                     FROM food_aliases fa
                     WHERE fa.food_id = f.id),
                    0.0::real
                )
            ) AS score,
            COALESCE(
                (SELECT fnv.value
                 FROM food_nutrient_values fnv
                 JOIN nutrients n ON n.id = fnv.nutrient_id
                 WHERE fnv.food_id = f.id AND n.name = 'energy'
                 LIMIT 1),
                0.0::real
            ) AS kcal_per_100
        FROM food_items f
        WHERE
            similarity(f.canonical_name, $1) > $3
            OR EXISTS (
                SELECT 1 FROM food_aliases fa
                WHERE fa.food_id = f.id AND similarity(fa.alias, $1) > $3
            )
        ORDER BY score DESC
        LIMIT $2
        "#,
    )
    .bind(query)
    .bind(i64::from(limit))
    .bind(min_score)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Core(liberado_core::CoreError::Database(e)))
}

// ─── USDA API ─────────────────────────────────────────────────────────────────

const USDA_SEARCH_URL: &str = "https://api.nal.usda.gov/fdc/v1/foods/search";

/// Calls the USDA FoodData Central search API. Filters to Foundation and SR
/// Legacy data types to ensure values are reliably per 100g.
pub async fn fetch_usda(
    client: &Client,
    api_key: &str,
    query: &str,
    page_size: u32,
) -> Result<Vec<UsdaFood>> {
    if api_key.is_empty() {
        warn!("LIBERADO_USDA_API_KEY is not set; skipping USDA lookup");
        return Ok(vec![]);
    }

    let resp = client
        .get(USDA_SEARCH_URL)
        .query(&[
            ("query", query),
            ("dataType", "Foundation,SR Legacy"),
            ("pageSize", &page_size.to_string()),
            ("api_key", api_key),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<UsdaSearchResponse>()
        .await?;

    debug!(count = resp.foods.len(), query, "USDA API response");
    Ok(resp.foods)
}

const USDA_DETAIL_BASE_URL: &str = "https://api.nal.usda.gov/fdc/v1/food";

/// Fetches the full USDA FoodData Central detail record for a single food item,
/// including named portions (e.g. "1 cup = 81g").
pub async fn fetch_usda_detail(
    client: &Client,
    api_key: &str,
    fdc_id: i32,
) -> Result<UsdaDetailResponse> {
    let url = format!("{USDA_DETAIL_BASE_URL}/{fdc_id}");
    let detail = client
        .get(&url)
        .query(&[("api_key", api_key)])
        .send()
        .await?
        .error_for_status()?
        .json::<UsdaDetailResponse>()
        .await?;
    debug!(fdc_id, portions = detail.food_portions.len(), "USDA detail fetched");
    Ok(detail)
}

// ─── Open Food Facts API ──────────────────────────────────────────────────────

const OFF_SEARCH_URL: &str = "https://world.openfoodfacts.org/cgi/search.pl";

/// Calls the Open Food Facts search API. Good for branded / packaged foods that
/// USDA may not cover. Values ending in `_100g` are reliable per-100g data.
pub async fn fetch_off(
    client: &Client,
    query: &str,
    page_size: u32,
) -> Result<Vec<OffProduct>> {
    let resp = client
        .get(OFF_SEARCH_URL)
        .query(&[
            ("search_terms", query),
            ("json", "true"),
            ("page_size", &page_size.to_string()),
            ("fields", "id,product_name,nutriments"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<OffSearchResponse>()
        .await?;

    debug!(count = resp.products.len(), query, "OFF API response");
    Ok(resp.products)
}

// ─── Caching ──────────────────────────────────────────────────────────────────

/// Maps a USDA nutrient_id to the canonical nutrient name used in the nutrients
/// table (seeded by migration 0002). Returns None for unmapped nutrient IDs.
pub fn usda_id_to_nutrient_name(usda_id: i32) -> Option<&'static str> {
    match usda_id {
        1008 => Some("energy"),
        1051 => Some("water"),
        1003 => Some("protein"),
        1004 => Some("fat_total"),
        1258 => Some("fat_saturated"),
        1257 => Some("fat_trans"),
        1005 => Some("carbohydrates"),
        1079 => Some("fiber"),
        2000 => Some("sugars"),
        1253 => Some("cholesterol"),
        1018 => Some("alcohol"),
        1106 => Some("vitamin_a"),
        1162 => Some("vitamin_c"),
        1114 => Some("vitamin_d"),
        1109 => Some("vitamin_e"),
        1185 => Some("vitamin_k"),
        1165 => Some("vitamin_b1"),
        1166 => Some("vitamin_b2"),
        1167 => Some("vitamin_b3"),
        1170 => Some("vitamin_b5"),
        1175 => Some("vitamin_b6"),
        1176 => Some("vitamin_b7"),
        1177 => Some("vitamin_b9"),
        1178 => Some("vitamin_b12"),
        1087 => Some("calcium"),
        1089 => Some("iron"),
        1090 => Some("magnesium"),
        1091 => Some("phosphorus"),
        1092 => Some("potassium"),
        1093 => Some("sodium"),
        1095 => Some("zinc"),
        1098 => Some("copper"),
        1101 => Some("manganese"),
        1103 => Some("selenium"),
        1057 => Some("caffeine"),
        _ => None,
    }
}

/// Inserts a single food portion (named serving size). Upserts on conflict.
pub async fn insert_food_portion(
    pool: &PgPool,
    food_id: i32,
    unit_label: &str,
    gram_equivalent: Option<f32>,
    ml_equivalent: Option<f32>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO food_portions (food_id, unit_label, gram_equivalent, ml_equivalent)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (food_id, unit_label) DO UPDATE
             SET gram_equivalent = EXCLUDED.gram_equivalent,
                 ml_equivalent   = EXCLUDED.ml_equivalent",
    )
    .bind(food_id)
    .bind(unit_label.trim().to_lowercase())
    .bind(gram_equivalent)
    .bind(ml_equivalent)
    .execute(pool)
    .await
    .map_err(|e| Error::Core(liberado_core::CoreError::Database(e)))?;
    Ok(())
}

/// Inserts nutrient values by canonical name. Skips entries where `value` is None
/// or zero. Uses the same upsert SQL as `insert_usda_nutrients`.
pub async fn insert_named_nutrients(
    pool: &PgPool,
    food_id: i32,
    pairs: &[(&str, Option<f32>)],
) -> Result<()> {
    for (name, value) in pairs {
        let Some(v) = value.filter(|v| *v != 0.0) else { continue };
        sqlx::query(
            "INSERT INTO food_nutrient_values (food_id, nutrient_id, value)
             SELECT $1, id, $3 FROM nutrients WHERE name = $2
             ON CONFLICT (food_id, nutrient_id) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(food_id)
        .bind(*name)
        .bind(v)
        .execute(pool)
        .await
        .map_err(|e| Error::Core(liberado_core::CoreError::Database(e)))?;
    }
    Ok(())
}

/// Inserts named portions for a USDA food item, normalising gram_equivalent to
/// "per 1 unit" (USDA sometimes stores e.g. 0.33 cup = 27g; we store 1 cup = 81g).
/// Skips entries with a missing modifier or gramWeight. Upserts on conflict.
pub async fn insert_usda_portions(
    pool: &PgPool,
    food_id: i32,
    portions: &[UsdaPortion],
) -> Result<()> {
    for p in portions {
        let (Some(modifier), Some(gram_weight)) = (&p.modifier, p.gram_weight) else {
            continue;
        };
        let label = modifier.trim();
        if label.is_empty() || gram_weight <= 0.0 {
            continue;
        }
        let amount = p.amount.unwrap_or(1.0).max(f64::EPSILON);
        let grams_per_unit = (gram_weight / amount) as f32;

        insert_food_portion(pool, food_id, label, Some(grams_per_unit), None).await?;
    }
    Ok(())
}

/// Stores a USDA food in the local cache. Returns the food_id.
/// If the food (by source_id) is already cached, returns the existing id.
pub async fn cache_usda_food(pool: &PgPool, food: &UsdaFood) -> Result<i32> {
    let source_id = food.fdc_id.to_string();

    // Return existing id if already cached
    if let Some(id) = sqlx::query_scalar::<_, i32>(
        "SELECT id FROM food_items WHERE source = 'usda' AND source_id = $1 LIMIT 1",
    )
    .bind(&source_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Core(liberado_core::CoreError::Database(e)))?
    {
        return Ok(id);
    }

    let food_id = sqlx::query_scalar::<_, i32>(
        "INSERT INTO food_items (canonical_name, basis, source, source_id, confidence)
         VALUES ($1, 'per_100g', 'usda', $2, 'exact_api')
         RETURNING id",
    )
    .bind(&food.description)
    .bind(&source_id)
    .fetch_one(pool)
    .await
    .map_err(|e| Error::Core(liberado_core::CoreError::Database(e)))?;

    insert_usda_nutrients(pool, food_id, &food.food_nutrients).await?;

    Ok(food_id)
}

pub async fn insert_usda_nutrients(
    pool: &PgPool,
    food_id: i32,
    nutrients: &[UsdaNutrient],
) -> Result<()> {
    for n in nutrients {
        let Some(name) = usda_id_to_nutrient_name(n.nutrient_id) else {
            continue;
        };
        let Some(value) = n.value else { continue };

        sqlx::query(
            "INSERT INTO food_nutrient_values (food_id, nutrient_id, value)
             SELECT $1, id, $3 FROM nutrients WHERE name = $2
             ON CONFLICT (food_id, nutrient_id) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(food_id)
        .bind(name)
        .bind(value as f32)
        .execute(pool)
        .await
        .map_err(|e| Error::Core(liberado_core::CoreError::Database(e)))?;
    }
    Ok(())
}

/// Stores an Open Food Facts product in the local cache. Returns the food_id.
/// If the product is already cached, returns the existing id.
pub async fn cache_off_food(pool: &PgPool, product: &OffProduct) -> Result<i32> {
    let name = product
        .product_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::NotFound("OFF product has no name".into()))?;

    let source_id = product.id.as_deref().unwrap_or("");

    if !source_id.is_empty()
        && let Some(id) = sqlx::query_scalar::<_, i32>(
            "SELECT id FROM food_items WHERE source = 'off' AND source_id = $1 LIMIT 1",
        )
        .bind(source_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| Error::Core(liberado_core::CoreError::Database(e)))?
    {
        return Ok(id);
    }

    let food_id = sqlx::query_scalar::<_, i32>(
        "INSERT INTO food_items (canonical_name, basis, source, source_id, confidence)
         VALUES ($1, 'per_100g', 'off', $2, 'exact_api')
         RETURNING id",
    )
    .bind(name)
    .bind(if source_id.is_empty() { None } else { Some(source_id) })
    .fetch_one(pool)
    .await
    .map_err(|e| Error::Core(liberado_core::CoreError::Database(e)))?;

    if let Some(n) = &product.nutriments {
        insert_off_nutrients(pool, food_id, n).await?;
    }

    // Insert the labelled serving size as a named portion if present.
    if let Some(qty) = product.serving_quantity
        && qty > 0.0 {
        let unit = product.serving_quantity_unit.as_deref().unwrap_or("g");
        let (gram_eq, ml_eq): (Option<f32>, Option<f32>) = if unit == "ml" {
            (None, Some(qty as f32))
        } else {
            (Some(qty as f32), None)
        };
        insert_food_portion(pool, food_id, "serving", gram_eq, ml_eq).await?;
    }

    Ok(food_id)
}

async fn insert_off_nutrients(
    pool: &PgPool,
    food_id: i32,
    n: &OffNutriments,
) -> Result<()> {
    let pairs: &[(&str, Option<f64>)] = &[
        ("energy", n.energy_kcal_100g),
        ("protein", n.proteins_100g),
        ("fat_total", n.fat_100g),
        ("carbohydrates", n.carbohydrates_100g),
        ("fiber", n.fiber_100g),
        ("sugars", n.sugars_100g),
        ("sodium", n.sodium_100g.map(|v| v * 1000.0)), // OFF stores sodium in g, we use mg
        ("calcium", n.calcium_100g.map(|v| v * 1000.0)),
        ("iron", n.iron_100g.map(|v| v * 1000.0)),
        ("caffeine", n.caffeine_100g.map(|v| v * 1000.0)),
    ];

    for (name, value) in pairs {
        let Some(v) = value else { continue };
        sqlx::query(
            "INSERT INTO food_nutrient_values (food_id, nutrient_id, value)
             SELECT $1, id, $3 FROM nutrients WHERE name = $2
             ON CONFLICT (food_id, nutrient_id) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(food_id)
        .bind(*name)
        .bind(*v as f32)
        .execute(pool)
        .await
        .map_err(|e| Error::Core(liberado_core::CoreError::Database(e)))?;
    }
    Ok(())
}

// ─── Kcal helper for cached foods ─────────────────────────────────────────────

/// Fetches the kcal per 100g/ml for a cached food item.
pub async fn get_kcal(pool: &PgPool, food_id: i32) -> Result<Option<f32>> {
    let v = sqlx::query_scalar::<_, f32>(
        "SELECT fnv.value
         FROM food_nutrient_values fnv
         JOIN nutrients n ON n.id = fnv.nutrient_id
         WHERE fnv.food_id = $1 AND n.name = 'energy'
         LIMIT 1",
    )
    .bind(food_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Core(liberado_core::CoreError::Database(e)))?;
    Ok(v)
}

// ─── Orchestrator ─────────────────────────────────────────────────────────────

/// Full food search: local pg_trgm → USDA API → OFF API.
/// Returns a `SearchResponse` that tells the LLM whether to auto-select or
/// present candidates, and whether a manual fallback is required.
pub async fn search(
    pool: &PgPool,
    http_client: &Client,
    usda_api_key: &str,
    strong_threshold: f32,
    max_weak_results: usize,
    query: &str,
) -> Result<SearchResponse> {
    // ── Step 1: local cache ───────────────────────────────────────────────────
    let local = search_local(pool, query, max_weak_results as u32 + 1, 0.15).await?;

    if let Some(best) = local.first() {
        if best.score >= strong_threshold {
            // Strong match — auto-select
            return Ok(SearchResponse {
                auto_selected: true,
                matches: vec![row_to_match(best)],
                query: query.to_string(),
                fallback_required: false,
                message: format!(
                    "Found '{}' in local cache (score {:.2}).",
                    best.canonical_name, best.score
                ),
            });
        }

        // Weak matches — return candidates for user confirmation
        let candidates: Vec<FoodMatch> = local
            .iter()
            .take(max_weak_results)
            .map(row_to_match)
            .collect();
        return Ok(SearchResponse {
            auto_selected: false,
            matches: candidates,
            query: query.to_string(),
            fallback_required: false,
            message: "Multiple partial matches found. Please confirm which one to use.".into(),
        });
    }

    // ── Step 2: parallel USDA + OFF lookup ───────────────────────────────────
    debug!(query, "No local match; querying external APIs");
    let (usda_result, off_result) = tokio::join!(
        fetch_usda(http_client, usda_api_key, query, 5),
        fetch_off(http_client, query, 5),
    );

    // Prefer USDA (higher data quality for generics).
    // Re-rank the returned candidates by trigram similarity to the query so we
    // pick e.g. "chicken breast, skinless" over "chicken breast, with skin".
    if let Ok(usda_foods) = usda_result
        && let Some((food, score)) = best_usda_match(usda_foods, query) {
        let food_id = cache_usda_food(pool, &food).await?;
        // Fetch named portions from the detail endpoint (best-effort).
        if !usda_api_key.is_empty()
            && let Ok(detail) = fetch_usda_detail(http_client, usda_api_key, food.fdc_id).await {
            let _ = insert_usda_portions(pool, food_id, &detail.food_portions).await;
        }
        let kcal = get_kcal(pool, food_id).await?.unwrap_or(0.0);
        return Ok(SearchResponse {
            auto_selected: true,
            matches: vec![FoodMatch {
                food_id,
                name: food.description.clone(),
                basis: "per_100g".into(),
                kcal_per_100: kcal,
                confidence: "exact_api".into(),
                source: "usda".into(),
                score,
            }],
            query: query.to_string(),
            fallback_required: false,
            message: format!("Found '{}' via USDA FoodData Central.", food.description),
        });
    }

    // Fall back to OFF, also re-ranked by similarity.
    if let Ok(off_products) = off_result
        && let Some((product, score)) = best_off_match(off_products, query) {
        let name = product.product_name.clone().unwrap_or_default();
        let food_id = cache_off_food(pool, &product).await?;
        let kcal = get_kcal(pool, food_id).await?.unwrap_or(0.0);
        return Ok(SearchResponse {
            auto_selected: true,
            matches: vec![FoodMatch {
                food_id,
                name: name.clone(),
                basis: "per_100g".into(),
                kcal_per_100: kcal,
                confidence: "exact_api".into(),
                source: "off".into(),
                score,
            }],
            query: query.to_string(),
            fallback_required: false,
            message: format!("Found '{name}' via Open Food Facts."),
        });
    }

    // ── Step 3: nothing found ─────────────────────────────────────────────────
    Ok(SearchResponse {
        auto_selected: false,
        matches: vec![],
        query: query.to_string(),
        fallback_required: true,
        message: format!(
            "Could not find '{query}' in the local cache or any external database. \
             Ask the user for the nutrition information per 100g, or use the \
             confirm_food tool once they provide it."
        ),
    })
}

// ─── Similarity helpers ───────────────────────────────────────────────────────

/// Jaccard similarity over character trigrams, matching pg_trgm's algorithm.
/// Used to re-rank external API results against the user's query.
fn trigram_similarity(a: &str, b: &str) -> f32 {
    use std::collections::HashSet;

    fn make_trigrams(s: &str) -> HashSet<[char; 3]> {
        let padded: Vec<char> = format!("  {} ", s.to_lowercase()).chars().collect();
        padded.windows(3).map(|w| [w[0], w[1], w[2]]).collect()
    }

    let ta = make_trigrams(a);
    let tb = make_trigrams(b);
    let intersection = ta.intersection(&tb).count();
    let union = ta.len() + tb.len() - intersection;
    if union == 0 { 0.0 } else { intersection as f32 / union as f32 }
}

/// Picks the USDA result most similar to `query`. Returns `None` if no candidate
/// clears the minimum similarity threshold.
fn best_usda_match(foods: Vec<UsdaFood>, query: &str) -> Option<(UsdaFood, f32)> {
    foods
        .into_iter()
        .map(|f| {
            let score = trigram_similarity(query, &f.description);
            (f, score)
        })
        .filter(|(_, s)| *s >= 0.10)
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

/// Picks the OFF product most similar to `query`. Filters out unnamed products
/// and those below the minimum similarity threshold.
fn best_off_match(products: Vec<OffProduct>, query: &str) -> Option<(OffProduct, f32)> {
    products
        .into_iter()
        .filter(|p| p.product_name.as_deref().is_some_and(|n| !n.is_empty()))
        .map(|p| {
            let score = trigram_similarity(query, p.product_name.as_deref().unwrap_or(""));
            (p, score)
        })
        .filter(|(_, s)| *s >= 0.10)
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

fn row_to_match(row: &LocalSearchRow) -> FoodMatch {
    FoodMatch {
        food_id:     row.id,
        name:        row.canonical_name.clone(),
        basis:       row.basis.clone(),
        kcal_per_100: row.kcal_per_100,
        confidence:  row.confidence.clone(),
        source:      row.source.clone(),
        score:       row.score,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Unit tests (no DB / network) ──────────────────────────────────────────

    #[test]
    fn usda_id_mapping_covers_energy() {
        assert_eq!(usda_id_to_nutrient_name(1008), Some("energy"));
    }

    #[test]
    fn usda_id_mapping_covers_all_seeded_nutrients() {
        let known_ids = [
            1008, 1051, 1003, 1004, 1258, 1257, 1005, 1079, 2000, 1253, 1018,
            1106, 1162, 1114, 1109, 1185, 1165, 1166, 1167, 1170, 1175, 1176,
            1177, 1178, 1087, 1089, 1090, 1091, 1092, 1093, 1095, 1098, 1101,
            1103, 1057,
        ];
        for id in known_ids {
            assert!(
                usda_id_to_nutrient_name(id).is_some(),
                "USDA nutrient_id {id} has no mapping"
            );
        }
    }

    #[test]
    fn usda_id_mapping_returns_none_for_unknown() {
        assert_eq!(usda_id_to_nutrient_name(9999), None);
        assert_eq!(usda_id_to_nutrient_name(0), None);
    }

    // ── trigram_similarity ────────────────────────────────────────────────────

    #[test]
    fn trigram_similarity_identical_strings() {
        assert!((trigram_similarity("chicken breast", "chicken breast") - 1.0).abs() < 0.001);
    }

    #[test]
    fn trigram_similarity_completely_disjoint() {
        assert_eq!(trigram_similarity("aaa", "zzz"), 0.0);
    }

    #[test]
    fn trigram_similarity_is_symmetric() {
        let ab = trigram_similarity("chicken breast", "chicken, cooked");
        let ba = trigram_similarity("chicken, cooked", "chicken breast");
        assert!((ab - ba).abs() < 0.001);
    }

    #[test]
    fn trigram_similarity_prefers_closer_match() {
        let close = trigram_similarity("chicken breast", "Chicken, breast, cooked, roasted");
        let far   = trigram_similarity("chicken breast", "Pork, ground, raw");
        assert!(close > far, "close={close}, far={far}");
    }

    #[test]
    fn trigram_similarity_case_insensitive() {
        let lower = trigram_similarity("oats", "oats rolled");
        let upper = trigram_similarity("OATS", "OATS ROLLED");
        assert!((lower - upper).abs() < 0.001);
    }

    // ── UsdaDetailResponse deserialization ────────────────────────────────────

    #[test]
    fn parse_usda_detail_response_deserializes_portions() {
        let json = r#"{
            "fdcId": 173904,
            "description": "Cereals, oats, regular and quick, not fortified, dry",
            "foodNutrients": [
                { "nutrient": { "id": 1008, "name": "Energy", "unitName": "kcal" }, "amount": 389.0 }
            ],
            "foodPortions": [
                { "amount": 1.0, "modifier": "cup", "gramWeight": 81.0 },
                { "amount": 0.5, "modifier": "cup", "gramWeight": 40.5 }
            ]
        }"#;

        let resp: UsdaDetailResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.fdc_id, 173904);
        assert_eq!(resp.food_portions.len(), 2);
        assert_eq!(resp.food_portions[0].modifier.as_deref(), Some("cup"));
        assert!((resp.food_portions[0].gram_weight.unwrap() - 81.0).abs() < 0.01);

        let nutrients = resp.to_usda_nutrients();
        assert_eq!(nutrients.len(), 1);
        assert_eq!(nutrients[0].nutrient_id, 1008);
        assert!((nutrients[0].value.unwrap() - 389.0).abs() < 0.01);
    }

    #[test]
    fn parse_usda_detail_response_empty_portions() {
        let json = r#"{ "fdcId": 1, "description": "Test" }"#;
        let resp: UsdaDetailResponse = serde_json::from_str(json).unwrap();
        assert!(resp.food_portions.is_empty());
        assert!(resp.to_usda_nutrients().is_empty());
    }

    // ── insert_usda_portions normalisation ────────────────────────────────────

    #[test]
    fn insert_usda_portions_normalises_fractional_amount() {
        // 0.33 cup = 27g  →  1 cup = 81.8g (approx)
        let portions = [UsdaPortion {
            amount: Some(0.33),
            gram_weight: Some(27.0),
            modifier: Some("cup".to_string()),
        }];
        // We can't run the DB insert here, but we can verify the math that
        // insert_usda_portions would use: gramWeight / amount.
        let grams_per_unit = (portions[0].gram_weight.unwrap()
            / portions[0].amount.unwrap()) as f32;
        assert!((grams_per_unit - 81.8).abs() < 0.5);
    }

    #[test]
    fn parse_usda_response_deserializes_correctly() {
        let json = r#"{
            "totalHits": 1,
            "currentPage": 1,
            "totalPages": 1,
            "pageList": [1],
            "foods": [
                {
                    "fdcId": 171705,
                    "description": "Chicken, breast, cooked, roasted",
                    "dataType": "SR Legacy",
                    "foodNutrients": [
                        { "nutrientId": 1008, "nutrientName": "Energy", "unitName": "KCAL", "value": 165.0 },
                        { "nutrientId": 1003, "nutrientName": "Protein", "unitName": "G", "value": 31.02 },
                        { "nutrientId": 1004, "nutrientName": "Total lipid (fat)", "unitName": "G", "value": 3.57 }
                    ]
                }
            ]
        }"#;

        let resp: UsdaSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.foods.len(), 1);
        let food = &resp.foods[0];
        assert_eq!(food.fdc_id, 171705);
        assert_eq!(food.description, "Chicken, breast, cooked, roasted");
        assert_eq!(food.food_nutrients.len(), 3);

        let energy = food.food_nutrients.iter().find(|n| n.nutrient_id == 1008).unwrap();
        assert!((energy.value.unwrap() - 165.0).abs() < 0.01);
    }

    #[test]
    fn parse_usda_response_handles_missing_nutrient_value() {
        let json = r#"{ "foods": [{ "fdcId": 1, "description": "Test", "foodNutrients": [
            { "nutrientId": 1008, "nutrientName": "Energy" }
        ]}] }"#;
        let resp: UsdaSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.foods[0].food_nutrients[0].value, None);
    }

    #[test]
    fn parse_off_response_deserializes_correctly() {
        let json = r#"{
            "count": 1,
            "page": 1,
            "page_size": 5,
            "products": [
                {
                    "id": "3760020507350",
                    "product_name": "Chobani Greek Yogurt",
                    "nutriments": {
                        "energy-kcal_100g": 100.0,
                        "proteins_100g": 17.0,
                        "fat_100g": 0.5,
                        "carbohydrates_100g": 6.0
                    }
                }
            ]
        }"#;

        let resp: OffSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.products.len(), 1);
        let product = &resp.products[0];
        assert_eq!(product.product_name.as_deref(), Some("Chobani Greek Yogurt"));
        let n = product.nutriments.as_ref().unwrap();
        assert_eq!(n.energy_kcal_100g, Some(100.0));
        assert_eq!(n.proteins_100g, Some(17.0));
    }

    #[test]
    fn parse_off_response_handles_missing_fields() {
        let json = r#"{ "products": [{ "id": "abc" }] }"#;
        let resp: OffSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.products[0].product_name, None);
        assert!(resp.products[0].nutriments.is_none());
    }

    #[test]
    fn parse_usda_response_empty_foods_array() {
        let json = r#"{ "foods": [] }"#;
        let resp: UsdaSearchResponse = serde_json::from_str(json).unwrap();
        assert!(resp.foods.is_empty());
    }

    #[test]
    fn parse_off_response_product_no_id() {
        let json = r#"{ "products": [{ "product_name": "Mystery Product" }] }"#;
        let resp: OffSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.products[0].product_name.as_deref(), Some("Mystery Product"));
        assert!(resp.products[0].id.is_none());
    }

    #[test]
    fn off_nutriments_default_is_all_none() {
        let n = OffNutriments::default();
        assert!(n.energy_kcal_100g.is_none());
        assert!(n.proteins_100g.is_none());
        assert!(n.fat_100g.is_none());
        assert!(n.carbohydrates_100g.is_none());
        assert!(n.fiber_100g.is_none());
        assert!(n.sodium_100g.is_none());
        assert!(n.calcium_100g.is_none());
        assert!(n.iron_100g.is_none());
        assert!(n.caffeine_100g.is_none());
    }

    #[test]
    fn usda_id_mapping_returns_expected_names() {
        assert_eq!(usda_id_to_nutrient_name(1003), Some("protein"));
        assert_eq!(usda_id_to_nutrient_name(1004), Some("fat_total"));
        assert_eq!(usda_id_to_nutrient_name(1258), Some("fat_saturated"));
        assert_eq!(usda_id_to_nutrient_name(1257), Some("fat_trans"));
        assert_eq!(usda_id_to_nutrient_name(1005), Some("carbohydrates"));
        assert_eq!(usda_id_to_nutrient_name(1079), Some("fiber"));
        assert_eq!(usda_id_to_nutrient_name(2000), Some("sugars"));
        assert_eq!(usda_id_to_nutrient_name(1093), Some("sodium"));
        assert_eq!(usda_id_to_nutrient_name(1057), Some("caffeine"));
        assert_eq!(usda_id_to_nutrient_name(1051), Some("water"));
        assert_eq!(usda_id_to_nutrient_name(1018), Some("alcohol"));
    }

    #[tokio::test]
    async fn fetch_usda_returns_empty_when_key_is_empty() {
        let client = reqwest::Client::new();
        let result = fetch_usda(&client, "", "chicken breast", 5).await.unwrap();
        assert!(result.is_empty());
    }

    // ── Integration tests (require PostgreSQL) ─────────────────────────────────

    async fn test_pool() -> sqlx::PgPool {
        let url = std::env::var("LIBERADO_TEST_DATABASE_URL").expect(
            "LIBERADO_TEST_DATABASE_URL must be set to run integration tests",
        );
        let pool = liberado_core::db::create_pool(&url, 2).await.unwrap();
        sqlx::migrate!("../migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    #[ignore = "requires running PostgreSQL; set LIBERADO_TEST_DATABASE_URL"]
    async fn search_local_returns_empty_on_fresh_db() {
        let pool = test_pool().await;
        // Clean up any leftover test data
        sqlx::query("DELETE FROM food_items WHERE source = 'usda' AND source_id = '999999999'")
            .execute(&pool)
            .await
            .unwrap();

        let results = search_local(&pool, "nonexistentfoodxyz", 5, 0.1).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires running PostgreSQL; set LIBERADO_TEST_DATABASE_URL"]
    async fn cache_and_search_round_trip() {
        let pool = test_pool().await;

        let test_food = UsdaFood {
            fdc_id: 999999999,
            description: "Test Chicken Breast Cached".to_string(),
            food_nutrients: vec![
                UsdaNutrient { nutrient_id: 1008, value: Some(165.0) },
                UsdaNutrient { nutrient_id: 1003, value: Some(31.0) },
                UsdaNutrient { nutrient_id: 1004, value: Some(3.6) },
            ],
            food_portions: vec![],
        };

        // Clean up before
        sqlx::query("DELETE FROM food_items WHERE source = 'usda' AND source_id = '999999999'")
            .execute(&pool)
            .await
            .unwrap();

        let food_id = cache_usda_food(&pool, &test_food).await.unwrap();
        assert!(food_id > 0);

        // Calling again returns the same id (idempotent)
        let food_id2 = cache_usda_food(&pool, &test_food).await.unwrap();
        assert_eq!(food_id, food_id2);

        // Energy should be stored
        let kcal = get_kcal(&pool, food_id).await.unwrap();
        assert!(kcal.is_some());
        assert!((kcal.unwrap() - 165.0).abs() < 0.1);

        // Should now appear in search results
        let results = search_local(&pool, "Test Chicken Breast Cached", 5, 0.1)
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].id, food_id);

        // Clean up after
        sqlx::query("DELETE FROM food_items WHERE id = $1")
            .bind(food_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires running PostgreSQL; set LIBERADO_TEST_DATABASE_URL"]
    async fn cache_usda_food_stores_all_mapped_nutrients() {
        let pool = test_pool().await;

        let test_food = UsdaFood {
            fdc_id: 999999998,
            description: "Nutrient Test Food".to_string(),
            food_nutrients: vec![
                UsdaNutrient { nutrient_id: 1008, value: Some(200.0) }, // energy
                UsdaNutrient { nutrient_id: 1003, value: Some(20.0) },  // protein
                UsdaNutrient { nutrient_id: 9999, value: Some(1.0) },   // unknown — should be skipped
            ],
            food_portions: vec![],
        };

        sqlx::query("DELETE FROM food_items WHERE source = 'usda' AND source_id = '999999998'")
            .execute(&pool)
            .await
            .unwrap();

        let food_id = cache_usda_food(&pool, &test_food).await.unwrap();

        let stored_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM food_nutrient_values WHERE food_id = $1",
        )
        .bind(food_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(stored_count, 2, "should store 2 known nutrients, skip unknown 9999");

        sqlx::query("DELETE FROM food_items WHERE id = $1")
            .bind(food_id)
            .execute(&pool)
            .await
            .unwrap();
    }
}
