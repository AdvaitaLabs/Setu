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
}
