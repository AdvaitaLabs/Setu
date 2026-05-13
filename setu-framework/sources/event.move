// ===== setu-framework/sources/event.move =====
// Structured event emission for Move contracts.
// Events are recorded in the transaction output (not stored as objects).
// Used by off-chain indexers and frontends to observe contract execution.
module setu::event {
    /// Emit a structured event.
    /// T must have `copy + drop` — `copy` ensures emit doesn't consume the value,
    /// `drop` ensures the event type doesn't hold resources.
    /// Note: T should be a struct type; primitive types will cause a runtime error.
    public fun emit<T: copy + drop>(event: T) {
        emit_internal(event);
    }

    native fun emit_internal<T: copy + drop>(event: T);
}
