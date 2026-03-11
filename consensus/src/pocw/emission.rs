//! Flux minting and emission adjustment.
//!
//! Minting: FluxMinted = κ × ΔPower_total
//! Adjustment: κ(k+1) = clamp(κ(k) × (1 + dampening × (target/real − 1)), κ_min, κ_max)

use setu_types::pocw::{EmissionState, PoCWConfig};

/// Compute Flux minted this fold.
///
/// FluxMinted = round(κ × total_power_consumed)
pub fn compute_flux_minted(total_power_consumed: u64, kappa: f64) -> u64 {
    (kappa * total_power_consumed as f64).round() as u64
}

/// Adjust κ based on observed vs target velocity.
///
/// Records `flux_minted` in the velocity history, then adjusts κ:
///   real_velocity = mean(velocity_history)
///   κ_new = κ × (1 + dampening × (target / real − 1))
///   κ_new = clamp(κ_new, κ_min, κ_max)
///
/// Returns the new κ value.
pub fn adjust_kappa(state: &mut EmissionState, flux_minted: u64, config: &PoCWConfig) -> f64 {
    state.velocity_history.push(flux_minted);
    if state.velocity_history.len() > config.observation_window {
        state.velocity_history.remove(0);
    }

    let real_velocity = state.velocity_history.iter().sum::<u64>() as f64
        / state.velocity_history.len() as f64;

    if real_velocity == 0.0 {
        return state.kappa;
    }

    let ratio = config.target_velocity / real_velocity;
    let dampened = 1.0 + config.dampening * (ratio - 1.0);
    state.kappa = (state.kappa * dampened).clamp(config.kappa_min, config.kappa_max);
    state.kappa
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Flux minting --

    #[test]
    fn test_minting_basic() {
        // κ=1.0, power=100 → minted=100
        assert_eq!(compute_flux_minted(100, 1.0), 100);
    }

    #[test]
    fn test_minting_with_kappa() {
        // κ=2.5, power=100 → minted=250
        assert_eq!(compute_flux_minted(100, 2.5), 250);
    }

    #[test]
    fn test_minting_zero_power() {
        assert_eq!(compute_flux_minted(0, 5.0), 0);
    }

    #[test]
    fn test_minting_rounding() {
        // κ=0.3, power=10 → 3.0 → 3
        assert_eq!(compute_flux_minted(10, 0.3), 3);
        // κ=0.7, power=3 → 2.1 → 2
        assert_eq!(compute_flux_minted(3, 0.7), 2);
    }

    // -- Kappa adjustment --

    fn default_emission_config() -> PoCWConfig {
        PoCWConfig {
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_adjust_kappa_at_target() {
        let config = default_emission_config();
        // target_velocity=1000, minted=1000 → ratio=1.0 → no change
        let mut state = EmissionState::new(1.0);
        let new_kappa = adjust_kappa(&mut state, 1000, &config);
        assert!((new_kappa - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_adjust_kappa_below_target() {
        let config = default_emission_config();
        // target=1000, minted=500 → ratio=2.0
        // dampened = 1 + 0.5*(2.0-1.0) = 1.5
        // κ_new = 1.0 * 1.5 = 1.5
        let mut state = EmissionState::new(1.0);
        let new_kappa = adjust_kappa(&mut state, 500, &config);
        assert!((new_kappa - 1.5).abs() < 1e-10);
    }

    #[test]
    fn test_adjust_kappa_above_target() {
        let config = default_emission_config();
        // target=1000, minted=2000 → ratio=0.5
        // dampened = 1 + 0.5*(0.5-1.0) = 0.75
        // κ_new = 1.0 * 0.75 = 0.75
        let mut state = EmissionState::new(1.0);
        let new_kappa = adjust_kappa(&mut state, 2000, &config);
        assert!((new_kappa - 0.75).abs() < 1e-10);
    }

    #[test]
    fn test_adjust_kappa_clamped_to_max() {
        let config = PoCWConfig {
            enabled: true,
            kappa_max: 2.0,
            ..Default::default()
        };
        // Start at κ=1.8, minted=100 (way below target=1000)
        // ratio=10.0, dampened=1+0.5*9=5.5, κ_new=1.8*5.5=9.9 → clamped to 2.0
        let mut state = EmissionState::new(1.8);
        let new_kappa = adjust_kappa(&mut state, 100, &config);
        assert!((new_kappa - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_adjust_kappa_clamped_to_min() {
        let config = PoCWConfig {
            enabled: true,
            kappa_min: 0.5,
            ..Default::default()
        };
        // Start at κ=0.6, minted=100_000 (way above target=1000)
        // ratio=0.01, dampened=1+0.5*(0.01-1)=0.505, κ_new=0.6*0.505=0.303 → clamped to 0.5
        let mut state = EmissionState::new(0.6);
        let new_kappa = adjust_kappa(&mut state, 100_000, &config);
        assert!((new_kappa - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_adjust_kappa_zero_minted_no_change() {
        let config = default_emission_config();
        let mut state = EmissionState::new(1.5);
        let new_kappa = adjust_kappa(&mut state, 0, &config);
        assert!((new_kappa - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_velocity_history_window() {
        let config = PoCWConfig {
            enabled: true,
            observation_window: 3,
            ..Default::default()
        };
        let mut state = EmissionState::new(1.0);

        adjust_kappa(&mut state, 100, &config);
        adjust_kappa(&mut state, 200, &config);
        adjust_kappa(&mut state, 300, &config);
        assert_eq!(state.velocity_history.len(), 3);

        // 4th push should evict the first (100)
        adjust_kappa(&mut state, 400, &config);
        assert_eq!(state.velocity_history.len(), 3);
        assert_eq!(state.velocity_history[0], 200);
    }
}
