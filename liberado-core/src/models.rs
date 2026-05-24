use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: i32,
    pub username: String,
    pub api_key_hash: String,
    pub timezone: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserGoal {
    pub id: i32,
    pub user_id: i32,
    pub effective_from: NaiveDate,
    pub kcal_target: Option<f32>,
    pub protein_g: Option<f32>,
    pub fat_g: Option<f32>,
    pub carbs_g: Option<f32>,
    pub fiber_g: Option<f32>,
    pub water_ml: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Nutrient {
    pub id: i32,
    pub name: String,
    pub display_name: String,
    pub unit: String,
    pub usda_nutrient_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FoodItem {
    pub id: i32,
    pub canonical_name: String,
    pub basis: String,
    pub source: String,
    pub source_id: Option<String>,
    pub confidence: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FoodNutrientValue {
    pub food_id: i32,
    pub nutrient_id: i32,
    pub value: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FoodPortion {
    pub id: i32,
    pub food_id: i32,
    pub unit_label: String,
    pub gram_equivalent: Option<f32>,
    pub ml_equivalent: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FoodAlias {
    pub id: i32,
    pub food_id: i32,
    pub alias: String,
    pub user_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Recipe {
    pub id: i32,
    pub user_id: i32,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RecipeIngredient {
    pub recipe_id: i32,
    pub food_id: i32,
    pub amount_g: Option<f32>,
    pub amount_ml: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MealLog {
    pub id: i32,
    pub user_id: i32,
    pub logged_at: DateTime<Utc>,
    pub meal_type: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct LogEntry {
    pub id: i32,
    pub meal_log_id: i32,
    pub food_id: Option<i32>,
    pub recipe_id: Option<i32>,
    pub amount_g: Option<f32>,
    pub amount_ml: Option<f32>,
    pub kcal_snapshot: f32,
    pub nutrient_snapshot: Option<JsonValue>,
    pub idempotency_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ExerciseLog {
    pub id: i32,
    pub user_id: i32,
    pub logged_at: DateTime<Utc>,
    pub description: String,
    pub calories_burned: f32,
    pub source: String,
    pub idempotency_key: String,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct WeightLog {
    pub id: i32,
    pub user_id: i32,
    pub logged_at: DateTime<Utc>,
    pub weight_kg: f32,
    pub note: Option<String>,
}

// ─── Rich response types returned by tool handlers ───────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoodSearchResult {
    pub food: FoodItem,
    pub similarity: f64,
    pub kcal_per_100: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailySummary {
    pub date: NaiveDate,
    pub kcal_consumed: f32,
    pub kcal_burned: f32,
    pub kcal_net: f32,
    pub nutrients: std::collections::HashMap<String, f32>,
    pub goal: Option<UserGoal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NutrientTotals {
    pub kcal: f32,
    pub nutrients: std::collections::HashMap<String, f32>,
}
