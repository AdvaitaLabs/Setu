// ===== setu-framework/sources/vec_map.move =====
// A map data structure backed by a vector.
// Preserves insertion order. O(n) lookup.
// Suitable for small collections (< 100 entries).
// Pure Move, no native functions needed.
module setu::vec_map {
    use std::vector;
    use std::option::{Self, Option};

    // ── Error codes ──
    const EKEY_ALREADY_EXISTS: u64 = 0;
    const EKEY_DOES_NOT_EXIST: u64 = 1;

    /// A map entry.
    struct Entry<K: copy, V> has copy, drop, store {
        key: K,
        value: V,
    }

    /// A map backed by a vector of entries.
    struct VecMap<K: copy, V> has copy, drop, store {
        contents: vector<Entry<K, V>>,
    }

    // ── Constructors ──

    /// Create an empty `VecMap`.
    public fun empty<K: copy, V>(): VecMap<K, V> {
        VecMap { contents: vector::empty() }
    }

    // ── Queries ──

    /// Return the number of entries in `self`.
    public fun size<K: copy, V>(self: &VecMap<K, V>): u64 {
        vector::length(&self.contents)
    }

    /// Return true if `self` contains no entries.
    public fun is_empty<K: copy, V>(self: &VecMap<K, V>): bool {
        vector::is_empty(&self.contents)
    }

    /// Return true if `self` contains an entry for `key`.
    public fun contains<K: copy, V>(self: &VecMap<K, V>, key: &K): bool {
        let (found, _) = get_idx(self, key);
        found
    }

    // ── Access ──

    /// Return a reference to the value associated with `key`.
    /// Aborts if `key` is not in the map.
    public fun get<K: copy, V>(self: &VecMap<K, V>, key: &K): &V {
        let (found, idx) = get_idx(self, key);
        assert!(found, EKEY_DOES_NOT_EXIST);
        let entry = vector::borrow(&self.contents, idx);
        &entry.value
    }

    /// Return a mutable reference to the value associated with `key`.
    /// Aborts if `key` is not in the map.
    public fun get_mut<K: copy, V>(self: &mut VecMap<K, V>, key: &K): &mut V {
        let (found, idx) = get_idx(self, key);
        assert!(found, EKEY_DOES_NOT_EXIST);
        let entry = vector::borrow_mut(&mut self.contents, idx);
        &mut entry.value
    }

    /// Try to get a reference to the value associated with `key`.
    /// Returns `Option::some(&value)` if found, `Option::none()` if not.
    public fun try_get<K: copy, V: copy>(self: &VecMap<K, V>, key: &K): Option<V> {
        let (found, idx) = get_idx(self, key);
        if (found) {
            option::some(*&vector::borrow(&self.contents, idx).value)
        } else {
            option::none()
        }
    }

    // ── Mutations ──

    /// Insert a new entry `(key, value)`.
    /// Aborts if `key` is already present.
    public fun insert<K: copy, V>(self: &mut VecMap<K, V>, key: K, value: V) {
        let (found, _) = get_idx(self, &key);
        assert!(!found, EKEY_ALREADY_EXISTS);
        vector::push_back(&mut self.contents, Entry { key, value });
    }

    /// Remove the entry for `key` and return its value.
    /// Aborts if `key` is not in the map.
    public fun remove<K: copy, V>(self: &mut VecMap<K, V>, key: &K): (K, V) {
        let (found, idx) = get_idx(self, key);
        assert!(found, EKEY_DOES_NOT_EXIST);
        let Entry { key, value } = vector::remove(&mut self.contents, idx);
        (key, value)
    }

    // ── Index-based access ──

    /// Return the index of `key` in the map, or (false, 0) if not found.
    public fun get_idx<K: copy, V>(self: &VecMap<K, V>, key: &K): (bool, u64) {
        let len = vector::length(&self.contents);
        let i = 0;
        while (i < len) {
            if (&vector::borrow(&self.contents, i).key == key) {
                return (true, i)
            };
            i = i + 1;
        };
        (false, 0)
    }

    /// Return a reference to the (key, value) entry at index `idx`.
    public fun get_entry_by_idx<K: copy, V>(self: &VecMap<K, V>, idx: u64): (&K, &V) {
        let entry = vector::borrow(&self.contents, idx);
        (&entry.key, &entry.value)
    }

    /// Return a mutable reference to the value at index `idx`, plus a ref to the key.
    public fun get_entry_by_idx_mut<K: copy, V>(self: &mut VecMap<K, V>, idx: u64): (&K, &mut V) {
        let entry = vector::borrow_mut(&mut self.contents, idx);
        (&entry.key, &mut entry.value)
    }

    // ── Destructors ──

    /// Return all keys in insertion order.
    public fun keys<K: copy, V>(self: &VecMap<K, V>): vector<K> {
        let result = vector::empty();
        let len = vector::length(&self.contents);
        let i = 0;
        while (i < len) {
            vector::push_back(&mut result, vector::borrow(&self.contents, i).key);
            i = i + 1;
        };
        result
    }

    /// Unpack into separate key and value vectors, in insertion order.
    public fun into_keys_values<K: copy, V>(self: VecMap<K, V>): (vector<K>, vector<V>) {
        let VecMap { contents } = self;
        let keys = vector::empty();
        let values = vector::empty();
        let len = vector::length(&contents);
        while (len > 0) {
            let Entry { key, value } = vector::pop_back(&mut contents);
            vector::push_back(&mut keys, key);
            vector::push_back(&mut values, value);
            len = len - 1;
        };
        vector::destroy_empty(contents);
        vector::reverse(&mut keys);
        vector::reverse(&mut values);
        (keys, values)
    }
}
