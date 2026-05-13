// ===== setu-framework/sources/string.move =====
// UTF-8 encoded string type, wrapping vector<u8>.
// Simplified version: no native UTF-8 validation (Phase 5e).
// Pure Move, no native functions needed.
module std::string {
    use std::vector;
    use std::option::{Self, Option};

    /// A `String` holds a UTF-8 encoded byte sequence.
    struct String has copy, drop, store {
        bytes: vector<u8>,
    }

    // ── Constructors ──

    /// Creates a new `String` from raw bytes.
    /// Note: Does NOT validate UTF-8 encoding (simplified version).
    public fun utf8(bytes: vector<u8>): String {
        String { bytes }
    }

    // ── Conversions ──

    /// Returns a reference to the underlying byte vector.
    public fun as_bytes(s: &String): &vector<u8> {
        &s.bytes
    }

    /// Unpack `String` into its underlying byte vector.
    public fun into_bytes(s: String): vector<u8> {
        let String { bytes } = s;
        bytes
    }

    // ── Queries ──

    /// Returns the length of this string, in bytes.
    public fun length(s: &String): u64 {
        vector::length(&s.bytes)
    }

    /// Returns whether this string is empty.
    public fun is_empty(s: &String): bool {
        vector::is_empty(&s.bytes)
    }

    // ── Mutations ──

    /// Appends the content of `other` to the end of `s`.
    public fun append(s: &mut String, other: String) {
        let String { bytes } = other;
        vector::append(&mut s.bytes, bytes);
    }

    /// Appends raw bytes to the end of `s`.
    public fun append_utf8(s: &mut String, bytes: vector<u8>) {
        vector::append(&mut s.bytes, bytes);
    }

    // ── Substrings ──

    /// Returns a sub-string from byte index `start` (inclusive) to `end` (exclusive).
    /// Aborts if indices are out of bounds.
    public fun sub_string(s: &String, start: u64, end: u64): String {
        let len = vector::length(&s.bytes);
        assert!(start <= end, 1); // E_INVALID_RANGE
        assert!(end <= len, 2);   // E_OUT_OF_BOUNDS
        let result = vector::empty<u8>();
        let i = start;
        while (i < end) {
            vector::push_back(&mut result, *vector::borrow(&s.bytes, i));
            i = i + 1;
        };
        String { bytes: result }
    }

    /// Returns the byte index of the first occurrence of `sub` in `s`.
    /// Returns `Option::none()` if not found.
    public fun index_of(s: &String, sub: &String): Option<u64> {
        let s_len = vector::length(&s.bytes);
        let sub_len = vector::length(&sub.bytes);
        if (sub_len > s_len) {
            return option::none()
        };
        if (sub_len == 0) {
            return option::some(0)
        };
        let i = 0;
        while (i <= s_len - sub_len) {
            let matched = true;
            let j = 0;
            while (j < sub_len) {
                if (*vector::borrow(&s.bytes, i + j) != *vector::borrow(&sub.bytes, j)) {
                    matched = false;
                    break
                };
                j = j + 1;
            };
            if (matched) {
                return option::some(i)
            };
            i = i + 1;
        };
        option::none()
    }
}
