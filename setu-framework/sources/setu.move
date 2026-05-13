// ===== setu-framework/sources/setu.move =====
// Native token type identifier for Coin<SETU>.
//
// Phase 2 (simplified): Only defines the SETU type.
// Phase 4+: Will add OTW init() mechanism for TreasuryCap creation.
module setu::setu {
    /// Native token type identifier
    /// ALL-CAPS + only `drop` ability = One-Time-Witness pattern (Sui convention)
    struct SETU has drop {}
}
