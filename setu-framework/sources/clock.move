// ===== setu-framework/sources/clock.move =====
// On-chain time access for Move contracts.
// Returns the epoch timestamp injected by consensus (not host clock).
module setu::clock {
    /// Return the current epoch timestamp in milliseconds (Unix epoch).
    ///
    /// Granularity: per-epoch (all transactions within the same epoch
    /// observe the same value).
    ///
    /// Determinism: value is injected by consensus, not read from host clock.
    public fun timestamp_ms(): u64 {
        timestamp_ms_internal()
    }

    native fun timestamp_ms_internal(): u64;
}
