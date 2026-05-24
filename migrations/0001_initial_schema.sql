CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS vector;

-- ─── Users ───────────────────────────────────────────────────────────────────

CREATE TABLE users (
    id           SERIAL PRIMARY KEY,
    username     TEXT NOT NULL UNIQUE,
    api_key_hash TEXT NOT NULL,
    timezone     TEXT NOT NULL DEFAULT 'UTC',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ─── User goals ──────────────────────────────────────────────────────────────

CREATE TABLE user_goals (
    id             SERIAL PRIMARY KEY,
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    effective_from DATE NOT NULL,
    kcal_target    REAL,
    protein_g      REAL,
    fat_g          REAL,
    carbs_g        REAL,
    fiber_g        REAL,
    water_ml       REAL,
    UNIQUE (user_id, effective_from)
);

-- ─── Nutrients ────────────────────────────────────────────────────────────────

CREATE TABLE nutrients (
    id               SERIAL PRIMARY KEY,
    name             TEXT NOT NULL UNIQUE,
    display_name     TEXT NOT NULL,
    unit             TEXT NOT NULL,
    usda_nutrient_id INTEGER
);

-- ─── Food items ──────────────────────────────────────────────────────────────

CREATE TABLE food_items (
    id             SERIAL PRIMARY KEY,
    canonical_name TEXT NOT NULL,
    basis          TEXT NOT NULL CHECK (basis IN ('per_100g', 'per_100ml')),
    source         TEXT NOT NULL CHECK (source IN ('usda', 'off', 'user', 'llm')),
    source_id      TEXT,
    confidence     TEXT NOT NULL CHECK (confidence IN ('exact_api', 'user_confirmed', 'llm_estimated')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Trigram index for fuzzy name matching (handles typos, partial matches)
CREATE INDEX food_items_name_trgm ON food_items USING gin(canonical_name gin_trgm_ops);
-- Full-text index for keyword search with stemming
CREATE INDEX food_items_name_fts ON food_items USING gin(to_tsvector('english', canonical_name));

-- ─── Food nutrient values ─────────────────────────────────────────────────────

CREATE TABLE food_nutrient_values (
    food_id     INTEGER NOT NULL REFERENCES food_items(id) ON DELETE CASCADE,
    nutrient_id INTEGER NOT NULL REFERENCES nutrients(id),
    value       REAL NOT NULL,
    PRIMARY KEY (food_id, nutrient_id)
);

-- ─── Food portions ────────────────────────────────────────────────────────────

CREATE TABLE food_portions (
    id              SERIAL PRIMARY KEY,
    food_id         INTEGER NOT NULL REFERENCES food_items(id) ON DELETE CASCADE,
    unit_label      TEXT NOT NULL,
    gram_equivalent REAL,
    ml_equivalent   REAL,
    CHECK (gram_equivalent IS NOT NULL OR ml_equivalent IS NOT NULL),
    UNIQUE (food_id, unit_label)
);

-- ─── Food aliases ─────────────────────────────────────────────────────────────

CREATE TABLE food_aliases (
    id      SERIAL PRIMARY KEY,
    food_id INTEGER NOT NULL REFERENCES food_items(id) ON DELETE CASCADE,
    alias   TEXT NOT NULL,
    user_id INTEGER REFERENCES users(id) ON DELETE CASCADE
);

-- Partial unique indexes handle the nullable user_id case cleanly
CREATE UNIQUE INDEX food_aliases_global_unique ON food_aliases (food_id, alias)
    WHERE user_id IS NULL;
CREATE UNIQUE INDEX food_aliases_user_unique ON food_aliases (food_id, alias, user_id)
    WHERE user_id IS NOT NULL;
CREATE INDEX food_aliases_trgm ON food_aliases USING gin(alias gin_trgm_ops);

-- ─── Food item tags ───────────────────────────────────────────────────────────

CREATE TABLE food_item_tags (
    id      SERIAL PRIMARY KEY,
    food_id INTEGER NOT NULL REFERENCES food_items(id) ON DELETE CASCADE,
    tag     TEXT NOT NULL,
    user_id INTEGER REFERENCES users(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX food_item_tags_global_unique ON food_item_tags (food_id, tag)
    WHERE user_id IS NULL;
CREATE UNIQUE INDEX food_item_tags_user_unique ON food_item_tags (food_id, tag, user_id)
    WHERE user_id IS NOT NULL;

-- ─── Recipes ──────────────────────────────────────────────────────────────────

CREATE TABLE recipes (
    id         SERIAL PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE recipe_ingredients (
    recipe_id INTEGER NOT NULL REFERENCES recipes(id) ON DELETE CASCADE,
    food_id   INTEGER NOT NULL REFERENCES food_items(id),
    amount_g  REAL,
    amount_ml REAL,
    PRIMARY KEY (recipe_id, food_id),
    CHECK (amount_g IS NOT NULL OR amount_ml IS NOT NULL)
);

-- ─── Meal logs ────────────────────────────────────────────────────────────────

CREATE TABLE meal_logs (
    id        SERIAL PRIMARY KEY,
    user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    logged_at TIMESTAMPTZ NOT NULL,
    meal_type TEXT NOT NULL CHECK (meal_type IN ('breakfast', 'lunch', 'dinner', 'snack', 'other')),
    note      TEXT
);

CREATE INDEX meal_logs_user_date ON meal_logs (user_id, logged_at);

-- ─── Log entries ─────────────────────────────────────────────────────────────

CREATE TABLE log_entries (
    id                SERIAL PRIMARY KEY,
    meal_log_id       INTEGER NOT NULL REFERENCES meal_logs(id) ON DELETE CASCADE,
    food_id           INTEGER REFERENCES food_items(id),
    recipe_id         INTEGER REFERENCES recipes(id),
    amount_g          REAL,
    amount_ml         REAL,
    -- Snapshotted at write time; editing a recipe never changes historical entries
    kcal_snapshot     REAL NOT NULL,
    nutrient_snapshot JSONB,
    idempotency_key   TEXT NOT NULL UNIQUE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (food_id IS NOT NULL OR recipe_id IS NOT NULL),
    CHECK (amount_g IS NOT NULL OR amount_ml IS NOT NULL)
);

CREATE TABLE log_entry_tags (
    log_entry_id INTEGER NOT NULL REFERENCES log_entries(id) ON DELETE CASCADE,
    tag          TEXT NOT NULL,
    PRIMARY KEY (log_entry_id, tag)
);

-- ─── Exercise logs ────────────────────────────────────────────────────────────

CREATE TABLE exercise_logs (
    id               SERIAL PRIMARY KEY,
    user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    logged_at        TIMESTAMPTZ NOT NULL,
    description      TEXT NOT NULL,
    calories_burned  REAL NOT NULL,
    source           TEXT NOT NULL CHECK (source IN ('user', 'llm_estimated', 'device')),
    idempotency_key  TEXT NOT NULL UNIQUE,
    note             TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX exercise_logs_user_date ON exercise_logs (user_id, logged_at);

-- ─── Weight logs ──────────────────────────────────────────────────────────────

CREATE TABLE weight_logs (
    id         SERIAL PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    logged_at  TIMESTAMPTZ NOT NULL,
    weight_kg  REAL NOT NULL,
    note       TEXT
);

CREATE INDEX weight_logs_user_date ON weight_logs (user_id, logged_at);

-- ─── Nutrient definitions ─────────────────────────────────────────────────────
-- Values stored in food_nutrient_values are always in these units.
-- usda_nutrient_id maps to USDA FoodData Central nutrient IDs for bulk import.

INSERT INTO nutrients (name, display_name, unit, usda_nutrient_id) VALUES
    -- Energy
    ('energy',              'Energy',                    'kcal', 1008),

    -- Macros
    ('water',               'Water',                     'g',    1051),
    ('protein',             'Protein',                   'g',    1003),
    ('fat_total',           'Total Fat',                 'g',    1004),
    ('fat_saturated',       'Saturated Fat',             'g',    1258),
    ('fat_trans',           'Trans Fat',                 'g',    1257),
    ('carbohydrates',       'Carbohydrates',             'g',    1005),
    ('fiber',               'Dietary Fiber',             'g',    1079),
    ('sugars',              'Sugars',                    'g',    2000),
    ('cholesterol',         'Cholesterol',               'mg',   1253),
    ('alcohol',             'Alcohol',                   'g',    1018),

    -- Vitamins
    ('vitamin_a',           'Vitamin A',                 'mcg',  1106),
    ('vitamin_c',           'Vitamin C',                 'mg',   1162),
    ('vitamin_d',           'Vitamin D',                 'mcg',  1114),
    ('vitamin_e',           'Vitamin E',                 'mg',   1109),
    ('vitamin_k',           'Vitamin K',                 'mcg',  1185),
    ('vitamin_b1',          'Vitamin B1 (Thiamine)',     'mg',   1165),
    ('vitamin_b2',          'Vitamin B2 (Riboflavin)',   'mg',   1166),
    ('vitamin_b3',          'Vitamin B3 (Niacin)',       'mg',   1167),
    ('vitamin_b5',          'Vitamin B5 (Pantothenic)',  'mg',   1170),
    ('vitamin_b6',          'Vitamin B6',                'mg',   1175),
    ('vitamin_b7',          'Vitamin B7 (Biotin)',       'mcg',  1176),
    ('vitamin_b9',          'Vitamin B9 (Folate)',       'mcg',  1177),
    ('vitamin_b12',         'Vitamin B12',               'mcg',  1178),

    -- Minerals
    ('calcium',             'Calcium',                   'mg',   1087),
    ('iron',                'Iron',                      'mg',   1089),
    ('magnesium',           'Magnesium',                 'mg',   1090),
    ('phosphorus',          'Phosphorus',                'mg',   1091),
    ('potassium',           'Potassium',                 'mg',   1092),
    ('sodium',              'Sodium',                    'mg',   1093),
    ('zinc',                'Zinc',                      'mg',   1095),
    ('copper',              'Copper',                    'mg',   1098),
    ('manganese',           'Manganese',                 'mg',   1101),
    ('selenium',            'Selenium',                  'mcg',  1103),
    ('iodine',              'Iodine',                    'mcg',  NULL),
    ('chromium',            'Chromium',                  'mcg',  NULL),

    -- Other
    ('caffeine',            'Caffeine',                  'mg',   1057);
