//! Power drain calculation for all transaction types.
//!
//! Two paths:
//! - Transfers: flat drain from `PoCWConfig.transfer_power_drain`
//! - Tasks: proportional drain `solver_power_rate * gas_used`, minimum 1

use setu_types::pocw::{EventMetrics, PoCWConfig};

/// Calculate the power drain for a FluxTransfer (flat).
pub fn calculate_transfer_power_drain(config: &PoCWConfig) -> u64 {
    if config.enabled {
        config.transfer_power_drain
    } else {
        0
    }
}

/// Calculate the power drain for a TaskSubmit event (proportional to gas).
///
/// ΔP = max(1, round(solver_power_rate * gas_used))
/// Returns 0 if PoCW is disabled.
pub fn calculate_task_power_drain(metrics: &EventMetrics, config: &PoCWConfig) -> u64 {
    if !config.enabled {
        return 0;
    }

    let drain = config.solver_power_rate * metrics.gas_used as f64;

    (drain.round() as u64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Transfer power drain (existing) --

    #[test]
    fn test_transfer_enabled_returns_flat_drain() {
        let config = PoCWConfig {
            enabled: true,
            ..Default::default()
        };
        assert_eq!(calculate_transfer_power_drain(&config), 1);
    }

    #[test]
    fn test_transfer_disabled_returns_zero() {
        let config = PoCWConfig::default();
        assert_eq!(calculate_transfer_power_drain(&config), 0);
    }

    #[test]
    fn test_transfer_custom_drain() {
        let config = PoCWConfig {
            enabled: true,
            transfer_power_drain: 10,
            ..Default::default()
        };
        assert_eq!(calculate_transfer_power_drain(&config), 10);
    }

    // -- Task power drain --

    fn make_metrics(gas_used: u64) -> EventMetrics {
        EventMetrics {
            solver_id: "solver-1".to_string(),
            compute_time_us: 0,
            gas_used,
            write_count: 0,
            read_count: 0,
            value_transferred: 0,
            dag_depth: 0,
            flux_burn: 0,
            power_delta: 0,
        }
    }

    #[test]
    fn test_task_power_disabled_returns_zero() {
        let config = PoCWConfig::default(); // enabled: false
        let metrics = make_metrics(10_000);
        assert_eq!(calculate_task_power_drain(&metrics, &config), 0);
    }

    #[test]
    fn test_task_power_known_input() {
        let config = PoCWConfig {
            enabled: true,
            ..Default::default()
        };
        // solver_power_rate default = 0.001
        // drain = 0.001 * 10_000 = 10.0 → 10
        let metrics = make_metrics(10_000);
        assert_eq!(calculate_task_power_drain(&metrics, &config), 10);
    }

    #[test]
    fn test_task_power_minimum_one() {
        let config = PoCWConfig {
            enabled: true,
            ..Default::default()
        };
        // drain = 0.001 * 1 = 0.001 → rounds to 0 → clamped to 1
        let metrics = make_metrics(1);
        assert_eq!(calculate_task_power_drain(&metrics, &config), 1);
    }

    #[test]
    fn test_task_power_zero_gas_still_minimum_one() {
        let config = PoCWConfig {
            enabled: true,
            ..Default::default()
        };
        // drain = 0.001 * 0 = 0.0 → rounds to 0 → clamped to 1
        let metrics = make_metrics(0);
        assert_eq!(calculate_task_power_drain(&metrics, &config), 1);
    }

    #[test]
    fn test_task_power_rounding() {
        let config = PoCWConfig {
            enabled: true,
            solver_power_rate: 0.003,
            ..Default::default()
        };
        // drain = 0.003 * 1500 = 4.5 → rounds to 4 (banker's rounding)
        // Actually f64::round() rounds 4.5 → 5 (round half away from zero)
        let metrics = make_metrics(1500);
        assert_eq!(calculate_task_power_drain(&metrics, &config), 5);
    }
}
