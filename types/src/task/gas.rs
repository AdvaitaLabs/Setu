//! Gas budget and usage types.

use serde::{Deserialize, Serialize};

/// Gas budget for execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasBudget {
    /// Maximum gas units allowed
    pub max_gas_units: u64,

    /// Gas price per unit (optional, for fee calculation)
    pub gas_price: Option<u64>,

    /// Estimated total fee
    pub estimated_fee: u64,
}

impl Default for GasBudget {
    fn default() -> Self {
        Self {
            // MVP: unlimited gas (no actual metering)
            max_gas_units: u64::MAX,
            gas_price: None,
            estimated_fee: 0,
        }
    }
}

impl GasBudget {
    /// Create a gas budget with specific limits
    pub fn new(max_gas_units: u64, gas_price: u64) -> Self {
        Self {
            max_gas_units,
            gas_price: Some(gas_price),
            estimated_fee: max_gas_units.saturating_mul(gas_price),
        }
    }

    /// Create unlimited gas budget (for testing/MVP)
    pub fn unlimited() -> Self {
        Self::default()
    }
}

/// Gas usage report
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GasUsage {
    /// Gas units consumed
    pub gas_used: u64,

    /// Actual fee charged (gas_used * gas_price)
    pub fee_charged: u64,
}

impl GasUsage {
    pub fn new(gas_used: u64, gas_price: Option<u64>) -> Self {
        Self {
            gas_used,
            fee_charged: gas_used.saturating_mul(gas_price.unwrap_or(0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_budget_default() {
        let budget = GasBudget::default();
        assert_eq!(budget.max_gas_units, u64::MAX);
        assert_eq!(budget.estimated_fee, 0);
    }

    #[test]
    fn test_gas_budget_new() {
        let budget = GasBudget::new(1000, 10);
        assert_eq!(budget.max_gas_units, 1000);
        assert_eq!(budget.estimated_fee, 10000);
    }

    #[test]
    fn test_gas_usage() {
        let usage = GasUsage::new(500, Some(10));
        assert_eq!(usage.gas_used, 500);
        assert_eq!(usage.fee_charged, 5000);
    }
}
