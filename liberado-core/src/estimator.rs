use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimatedNutrition {
    /// Nutrient name → value per 100g (using canonical nutrient names from the nutrients table)
    pub nutrients: HashMap<String, f32>,
    /// Human-readable note surfaced to the user via the serving LLM
    pub confidence_note: String,
}

#[async_trait]
pub trait NutritionEstimator: Send + Sync {
    async fn estimate(&self, description: &str, amount_g: f32) -> Result<EstimatedNutrition>;
}

/// Returned when LIBERADO_ESTIMATOR_PROVIDER=none or is unset.
pub struct NoopEstimator;

#[async_trait]
impl NutritionEstimator for NoopEstimator {
    async fn estimate(&self, _description: &str, _amount_g: f32) -> Result<EstimatedNutrition> {
        Err(CoreError::EstimationUnavailable(
            "nutrition estimation is disabled; set LIBERADO_ESTIMATOR_PROVIDER to enable"
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_estimator_returns_estimation_unavailable() {
        let est = NoopEstimator;
        let err = est.estimate("scrambled eggs", 150.0).await.unwrap_err();
        assert!(matches!(err, CoreError::EstimationUnavailable(_)));
        assert!(err.to_string().contains("disabled"));
    }

    #[tokio::test]
    async fn noop_estimator_ignores_inputs() {
        let est = NoopEstimator;
        let e1 = est.estimate("", 0.0).await.unwrap_err();
        let e2 = est.estimate("pizza", 9999.0).await.unwrap_err();
        assert!(matches!(e1, CoreError::EstimationUnavailable(_)));
        assert!(matches!(e2, CoreError::EstimationUnavailable(_)));
    }
}
