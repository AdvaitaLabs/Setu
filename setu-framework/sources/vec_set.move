// ===== setu-framework/sources/vec_set.move =====
// A set data structure backed by a vector.
// Preserves insertion order. O(n) lookup.
// Suitable for small collections (< 100 entries).
// Pure Move, no native functions needed.
module setu::vec_set {
    use std::vector;

    // ── Error codes ──
    const EKEY_ALREADY_EXISTS: u64 = 0;
    const EKEY_DOES_NOT_EXIST: u64 = 1;

    /// A set backed by a vector.
    struct VecSet<K: copy + drop> has copy, drop, store {
        contents: vector<K>,
    }

    // ── Constructors ──

    /// Create an empty `VecSet`.
    public fun empty<K: copy + drop>(): VecSet<K> {
        VecSet { contents: vector::empty() }
    }

    /// Create a `VecSet` with a single element.
    public fun singleton<K: copy + drop>(key: K): VecSet<K> {
        VecSet { contents: vector::singleton(key) }
    }

    // ── Queries ──

    /// Return the number of elements in `self`.
    public fun size<K: copy + drop>(self: &VecSet<K>): u64 {
        vector::length(&self.contents)
    }

    /// Return true if `self` contains no elements.
    public fun is_empty<K: copy + drop>(self: &VecSet<K>): bool {
        vector::is_empty(&self.contents)
    }

    /// Return true if `self` contains `key`.
    public fun contains<K: copy + drop>(self: &VecSet<K>, key: &K): bool {
        vector::contains(&self.contents, key)
    }

    // ── Mutations ──

    /// Insert `key` into the set.
    /// Aborts if `key` is already in the set.
    public fun insert<K: copy + drop>(self: &mut VecSet<K>, key: K) {
        assert!(!contains(self, &key), EKEY_ALREADY_EXISTS);
        vector::push_back(&mut self.contents, key);
    }

    /// Remove `key` from the set.
    /// Aborts if `key` is not in the set.
    public fun remove<K: copy + drop>(self: &mut VecSet<K>, key: &K) {
        let (found, idx) = vector::index_of(&self.contents, key);
        assert!(found, EKEY_DOES_NOT_EXIST);
        vector::remove(&mut self.contents, idx);
    }

    // ── Destructors ──

    /// Unpack into the underlying vector of keys.
    public fun into_keys<K: copy + drop>(self: VecSet<K>): vector<K> {
        let VecSet { contents } = self;
        contents
    }
}
