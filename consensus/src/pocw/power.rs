//! Power drain calculation for FluxTransfer transactions.
//!
//! Returns the configured flat drain when PoCW is enabled, 0 otherwise.

use setu_types::pocw::PoCWConfig;

/// Calculate the power drain for a FluxTransfer transaction.
pub fn calculate_transfer_power_drain(config: &PoCWConfig) -> u64 {
    if config.enabled {
        config.transfer_power_drain
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enabled_returns_flat_drain() {
        let config = PoCWConfig {
            enabled: true,
            ..Default::default()
        };
        assert_eq!(calculate_transfer_power_drain(&config), 1);
    }

    #[test]
    fn test_disabled_returns_zero() {
        let config = PoCWConfig::default(); // enabled: false
        assert_eq!(calculate_transfer_power_drain(&config), 0);
    }

    #[test]
    fn test_custom_drain() {
        let config = PoCWConfig {
            enabled: true,
            transfer_power_drain: 10,
            ..Default::default()
        };
        assert_eq!(calculate_transfer_power_drain(&config), 10);
    }
}
