use sqlx::PgPool;

use crate::error::Result;

/// The result of resolving an amount + unit string into a canonical base unit.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedAmount {
    Grams(f32),
    Milliliters(f32),
    /// Unit could not be resolved without a food-specific portion lookup.
    /// Caller must call `resolve_named_portion` with the food_id.
    Named { label: String, count: f32 },
}

/// Converts an amount + unit string into a canonical base unit where possible.
/// Unambiguous mass/volume units are converted immediately.
/// Cooking measures (cup, tbsp, tsp) and count units (piece, medium, slice) return
/// `Named` and require a food_portions DB lookup via `resolve_named_portion`.
pub fn parse_amount(amount: f32, unit: &str) -> ParsedAmount {
    match unit.trim().to_lowercase().as_str() {
        // Mass → grams
        "g" | "gram" | "grams" => ParsedAmount::Grams(amount),
        "kg" | "kilogram" | "kilograms" => ParsedAmount::Grams(amount * 1_000.0),
        "mg" | "milligram" | "milligrams" => ParsedAmount::Grams(amount / 1_000.0),
        "lb" | "lbs" | "pound" | "pounds" => ParsedAmount::Grams(amount * 453.592),
        "oz" | "ounce" | "ounces" => ParsedAmount::Grams(amount * 28.3495),

        // Volume → milliliters
        "ml" | "milliliter" | "milliliters" | "millilitre" | "millilitres" => {
            ParsedAmount::Milliliters(amount)
        }
        "l" | "liter" | "liters" | "litre" | "litres" => ParsedAmount::Milliliters(amount * 1_000.0),
        "fl oz" | "fl_oz" | "fluid ounce" | "fluid ounces" => {
            ParsedAmount::Milliliters(amount * 29.5735)
        }

        // Everything else (cup, tbsp, tsp, piece, medium, large, slice, serving…)
        // requires a food-specific portion lookup.
        other => ParsedAmount::Named {
            label: other.to_string(),
            count: amount,
        },
    }
}

/// Resolves a `Named` portion to grams or milliliters using the food_portions table.
/// Returns `None` if no matching portion is found for the food + unit combination.
pub async fn resolve_named_portion(
    pool: &PgPool,
    food_id: i32,
    label: &str,
    count: f32,
) -> Result<Option<ParsedAmount>> {
    let row = sqlx::query_as::<_, (Option<f32>, Option<f32>)>(
        "SELECT gram_equivalent, ml_equivalent
         FROM food_portions
         WHERE food_id = $1 AND lower(unit_label) = lower($2)
         LIMIT 1",
    )
    .bind(food_id)
    .bind(label)
    .fetch_optional(pool)
    .await
    .map_err(liberado_core::CoreError::Database)?;

    Ok(row.map(|(g, ml)| {
        if let Some(grams) = g {
            ParsedAmount::Grams(grams * count)
        } else if let Some(mils) = ml {
            ParsedAmount::Milliliters(mils * count)
        } else {
            // Row exists but both columns are null — shouldn't happen given CHECK constraint
            ParsedAmount::Grams(0.0)
        }
    }))
}

/// Scales nutrition per-100 values to the logged amount.
/// `basis` is either "per_100g" or "per_100ml".
pub fn scale_nutrient(value_per_100: f32, parsed: &ParsedAmount, basis: &str) -> f32 {
    let base_amount = match (parsed, basis) {
        (ParsedAmount::Grams(g), "per_100g") => *g,
        (ParsedAmount::Milliliters(ml), "per_100ml") => *ml,
        // Mismatched basis — caller should have caught this; return 0 rather than panic
        _ => return 0.0,
    };
    value_per_100 * base_amount / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grams_passthrough() {
        assert_eq!(parse_amount(150.0, "g"), ParsedAmount::Grams(150.0));
        assert_eq!(parse_amount(150.0, "gram"), ParsedAmount::Grams(150.0));
        assert_eq!(parse_amount(150.0, "grams"), ParsedAmount::Grams(150.0));
    }

    #[test]
    fn kg_to_grams() {
        assert_eq!(parse_amount(1.0, "kg"), ParsedAmount::Grams(1_000.0));
        assert_eq!(parse_amount(0.5, "kg"), ParsedAmount::Grams(500.0));
    }

    #[test]
    fn lb_to_grams() {
        let ParsedAmount::Grams(g) = parse_amount(1.0, "lb") else { panic!() };
        assert!((g - 453.592).abs() < 0.01);
    }

    #[test]
    fn oz_to_grams() {
        let ParsedAmount::Grams(g) = parse_amount(1.0, "oz") else { panic!() };
        assert!((g - 28.3495).abs() < 0.01);
    }

    #[test]
    fn ml_passthrough() {
        assert_eq!(parse_amount(250.0, "ml"), ParsedAmount::Milliliters(250.0));
    }

    #[test]
    fn liters_to_ml() {
        assert_eq!(parse_amount(1.0, "l"), ParsedAmount::Milliliters(1_000.0));
        assert_eq!(parse_amount(1.0, "liter"), ParsedAmount::Milliliters(1_000.0));
    }

    #[test]
    fn fl_oz_to_ml() {
        let ParsedAmount::Milliliters(ml) = parse_amount(1.0, "fl oz") else { panic!() };
        assert!((ml - 29.5735).abs() < 0.01);
    }

    #[test]
    fn named_units_require_lookup() {
        assert_eq!(
            parse_amount(1.0, "cup"),
            ParsedAmount::Named { label: "cup".into(), count: 1.0 }
        );
        assert_eq!(
            parse_amount(2.0, "slice"),
            ParsedAmount::Named { label: "slice".into(), count: 2.0 }
        );
        assert_eq!(
            parse_amount(1.0, "medium"),
            ParsedAmount::Named { label: "medium".into(), count: 1.0 }
        );
        assert_eq!(
            parse_amount(3.0, "serving"),
            ParsedAmount::Named { label: "serving".into(), count: 3.0 }
        );
    }

    #[test]
    fn unit_matching_is_case_insensitive() {
        assert_eq!(parse_amount(1.0, "G"), ParsedAmount::Grams(1.0));
        assert_eq!(parse_amount(1.0, "Grams"), ParsedAmount::Grams(1.0));
        assert_eq!(parse_amount(1.0, "ML"), ParsedAmount::Milliliters(1.0));
    }

    #[test]
    fn unit_matching_trims_whitespace() {
        assert_eq!(parse_amount(1.0, "  g  "), ParsedAmount::Grams(1.0));
    }

    #[test]
    fn scale_nutrient_grams() {
        // 165 kcal/100g, user logged 200g → 330 kcal
        let result = scale_nutrient(165.0, &ParsedAmount::Grams(200.0), "per_100g");
        assert!((result - 330.0).abs() < 0.01);
    }

    #[test]
    fn scale_nutrient_ml() {
        // 42 kcal/100ml, user logged 250ml → 105 kcal
        let result = scale_nutrient(42.0, &ParsedAmount::Milliliters(250.0), "per_100ml");
        assert!((result - 105.0).abs() < 0.01);
    }

    #[test]
    fn scale_nutrient_mismatched_basis_returns_zero() {
        let result = scale_nutrient(165.0, &ParsedAmount::Milliliters(200.0), "per_100g");
        assert_eq!(result, 0.0);
    }

    #[test]
    fn scale_nutrient_named_returns_zero() {
        let result = scale_nutrient(
            165.0,
            &ParsedAmount::Named { label: "cup".into(), count: 1.0 },
            "per_100g",
        );
        assert_eq!(result, 0.0);
    }

    #[test]
    fn mg_to_grams() {
        let ParsedAmount::Grams(g) = parse_amount(500.0, "mg") else { panic!() };
        assert!((g - 0.5).abs() < 0.0001);
        let ParsedAmount::Grams(g2) = parse_amount(1.0, "milligram") else { panic!() };
        assert!((g2 - 0.001).abs() < 0.000001);
        let ParsedAmount::Grams(g3) = parse_amount(1.0, "milligrams") else { panic!() };
        assert!((g3 - 0.001).abs() < 0.000001);
    }

    #[test]
    fn alternative_mass_spellings() {
        assert_eq!(parse_amount(1.0, "kilogram"), ParsedAmount::Grams(1_000.0));
        assert_eq!(parse_amount(1.0, "kilograms"), ParsedAmount::Grams(1_000.0));
        let ParsedAmount::Grams(g1) = parse_amount(1.0, "pound") else { panic!() };
        assert!((g1 - 453.592).abs() < 0.01);
        let ParsedAmount::Grams(g2) = parse_amount(1.0, "pounds") else { panic!() };
        assert!((g2 - 453.592).abs() < 0.01);
        let ParsedAmount::Grams(g3) = parse_amount(1.0, "lbs") else { panic!() };
        assert!((g3 - 453.592).abs() < 0.01);
        let ParsedAmount::Grams(g4) = parse_amount(1.0, "ounce") else { panic!() };
        assert!((g4 - 28.3495).abs() < 0.01);
        let ParsedAmount::Grams(g5) = parse_amount(1.0, "ounces") else { panic!() };
        assert!((g5 - 28.3495).abs() < 0.01);
    }

    #[test]
    fn alternative_volume_spellings() {
        for alias in &["milliliter", "milliliters", "millilitre", "millilitres"] {
            assert_eq!(
                parse_amount(1.0, alias),
                ParsedAmount::Milliliters(1.0),
                "failed for '{alias}'"
            );
        }
        for alias in &["liters", "litre", "litres"] {
            assert_eq!(
                parse_amount(1.0, alias),
                ParsedAmount::Milliliters(1_000.0),
                "failed for '{alias}'"
            );
        }
        for alias in &["fl_oz", "fluid ounce", "fluid ounces"] {
            let ParsedAmount::Milliliters(ml) = parse_amount(1.0, alias) else {
                panic!("expected Milliliters for '{alias}'")
            };
            assert!((ml - 29.5735).abs() < 0.01, "wrong value for '{alias}': {ml}");
        }
    }
}
