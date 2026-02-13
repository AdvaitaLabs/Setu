//! Flux burn calculation for FluxTransfer transactions.
//!
//! Returns the configured fixed fee when PoCW is enabled, 0 otherwise.

use setu_types::pocw::PoCWConfig;

/// Calculate the Flux burn for a FluxTransfer transaction.
pub fn calculate_transfer_burn(config: &PoCWConfig) -> u64 {
    if config.enabled {
        config.transfer_fixed_fee
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enabled_returns_fixed_fee() {
        let config = PoCWConfig {
            enabled: true,
            ..Default::default()
        };
        assert_eq!(calculate_transfer_burn(&config), 21_000);
    }

    #[test]
    fn test_disabled_returns_zero() {
        let config = PoCWConfig::default(); // enabled: false
        assert_eq!(calculate_transfer_burn(&config), 0);
    }

    #[test]
    fn test_custom_fee() {
        let config = PoCWConfig {
            enabled: true,
            transfer_fixed_fee: 50_000,
            ..Default::default()
        };
        assert_eq!(calculate_transfer_burn(&config), 50_000);
    }
}
