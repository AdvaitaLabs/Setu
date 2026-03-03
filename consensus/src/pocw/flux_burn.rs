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

    // -- Task burn --

    fn make_metrics(compute_time_us: u64, gas_used: u64, write_count: usize, value_transferred: u64, dag_depth: u64) -> EventMetrics {
        EventMetrics {
            solver_id: "solver-1".to_string(),
            compute_time_us,
            gas_used,
            write_count,
            read_count: 0,
            value_transferred,
            dag_depth,
            flux_burn: 0,
            power_delta: 0,
        }
    }

    #[test]
    fn test_task_burn_disabled_returns_zero() {
        let config = PoCWConfig::default(); // enabled: false
        let metrics = make_metrics(100, 500, 2, 0, 3);
        assert_eq!(calculate_task_burn(&metrics, &config), 0);
    }

    #[test]
    fn test_task_burn_known_input() {
        let config = PoCWConfig {
            enabled: true,
            ..Default::default()
        };
        // C = 100 + 500*10 + 2*1000 = 7100
        // R = 0
        // S = 3*100 + 2*500 = 1300
        // burn = 0.4*7100 + 0.35*0 + 0.25*1300 = 2840 + 0 + 325 = 3165
        let metrics = make_metrics(100, 500, 2, 0, 3);
        assert_eq!(calculate_task_burn(&metrics, &config), 3165);
    }

    #[test]
    fn test_task_burn_with_value_transfer() {
        let config = PoCWConfig {
            enabled: true,
            ..Default::default()
        };
        // C = 0 + 0 + 0 = 0
        // R = 10000
        // S = 0 + 0 = 0
        // burn = 0.4*0 + 0.35*10000 + 0.25*0 = 3500
        let metrics = make_metrics(0, 0, 0, 10_000, 0);
        assert_eq!(calculate_task_burn(&metrics, &config), 3500);
    }

    #[test]
    fn test_task_burn_minimum_one() {
        let config = PoCWConfig {
            enabled: true,
            ..Default::default()
        };
        // All zeros produces burn = 0, but minimum is 1
        let metrics = make_metrics(0, 0, 0, 0, 0);
        assert_eq!(calculate_task_burn(&metrics, &config), 1);
    }

    #[test]
    fn test_task_burn_custom_weights() {
        let config = PoCWConfig {
            enabled: true,
            alpha: 1.0,
            beta: 0.0,
            gamma: 0.0,
            ..Default::default()
        };
        // Only complexity matters: C = 200 + 100*10 + 1*1000 = 2200
        let metrics = make_metrics(200, 100, 1, 50_000, 10);
        assert_eq!(calculate_task_burn(&metrics, &config), 2200);
    }
}
