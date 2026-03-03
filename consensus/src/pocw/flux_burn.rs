//! Flux burn calculation for all transaction types.
//!
//! Two paths:
//! - Transfers: flat fee from `PoCWConfig.transfer_fee`
//! - Tasks: formula-based `α*C + β*R + γ*S` using `EventMetrics`

use setu_types::pocw::{EventMetrics, PoCWConfig};

/// Calculate the Flux burn for a FluxTransfer (flat fee).
pub fn calculate_transfer_burn(config: &PoCWConfig) -> u64 {
    if config.enabled {
        config.transfer_fee
    } else {
        0
    }
}

/// Complexity score: compute_time + gas*10 + writes*1000
fn compute_complexity(metrics: &EventMetrics) -> f64 {
    metrics.compute_time_us as f64
        + metrics.gas_used as f64 * 10.0
        + metrics.write_count as f64 * 1000.0
}

/// Risk score: value at stake
fn compute_risk(metrics: &EventMetrics) -> f64 {
    metrics.value_transferred as f64
}

/// Structural tension score: dag_depth*100 + writes*500
fn compute_structural_tension(metrics: &EventMetrics) -> f64 {
    metrics.dag_depth as f64 * 100.0
        + metrics.write_count as f64 * 500.0
}

/// Calculate the Flux burn for a TaskSubmit event (formula-based).
///
/// burn = round(α*C + β*R + γ*S), minimum 1.
/// Returns 0 if PoCW is disabled.
pub fn calculate_task_burn(metrics: &EventMetrics, config: &PoCWConfig) -> u64 {
    if !config.enabled {
        return 0;
    }

    let c = compute_complexity(metrics);
    let r = compute_risk(metrics);
    let s = compute_structural_tension(metrics);

    let burn = config.alpha * c + config.beta * r + config.gamma * s;

    (burn.round() as u64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Transfer burn (existing) --

    #[test]
    fn test_transfer_enabled_returns_fixed_fee() {
        let config = PoCWConfig {
            enabled: true,
            ..Default::default()
        };
        assert_eq!(calculate_transfer_burn(&config), 21_000);
    }

    #[test]
    fn test_transfer_disabled_returns_zero() {
        let config = PoCWConfig::default();
        assert_eq!(calculate_transfer_burn(&config), 0);
    }

    #[test]
    fn test_transfer_custom_fee() {
        let config = PoCWConfig {
            enabled: true,
            transfer_fee: 50_000,
            ..Default::default()
        };
        assert_eq!(calculate_transfer_burn(&config), 50_000);
    }
}
