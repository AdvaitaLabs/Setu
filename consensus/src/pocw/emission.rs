//! Flux minting (Stage 3) and emission adjustment (Stage 5).
//!
//! Stage 3: FluxMinted = κ × ΔPower_total
//! Stage 5: κ(k+1) = clamp(κ(k) × (1 + dampening × (target/real − 1)), κ_min, κ_max)

use setu_types::pocw::{EmissionState, PoCWConfig};

/// Stage 3: compute Flux minted this fold.
///
/// FluxMinted = round(κ × total_power_consumed)
pub fn compute_flux_minted(total_power_consumed: u64, kappa: f64) -> u64 {
    (kappa * total_power_consumed as f64).round() as u64
}

/// Stage 5: adjust κ based on observed vs target velocity.
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
