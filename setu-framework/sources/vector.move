/// Standard vector module — native operations provided by the Move VM.
///
/// This is a minimal subset of move-stdlib's vector module,
/// providing only the essential native operations that the VM supports
/// as bytecode instructions.
module std::vector {
    /// Create an empty vector.
    native public fun empty<Element>(): vector<Element>;

    /// Return the length of the vector.
    native public fun length<Element>(v: &vector<Element>): u64;

    /// Acquire an immutable reference to the `i`th element of the vector.
    /// Aborts if `i` is out of bounds.
    native public fun borrow<Element>(v: &vector<Element>, i: u64): &Element;

    /// Add element `e` to the end of the vector.
    native public fun push_back<Element>(v: &mut vector<Element>, e: Element);

    /// Acquire a mutable reference to the `i`th element of the vector.
    /// Aborts if `i` is out of bounds.
    native public fun borrow_mut<Element>(v: &mut vector<Element>, i: u64): &mut Element;

    /// Pop an element from the end of vector.
    /// Aborts if the vector is empty.
    native public fun pop_back<Element>(v: &mut vector<Element>): Element;

    /// Destroy the vector. Aborts if it is not empty.
    native public fun destroy_empty<Element>(v: vector<Element>);

    /// Swap the `i`th and `j`th elements in the vector.
    native public fun swap<Element>(v: &mut vector<Element>, i: u64, j: u64);

    // ═══════════════════════════════════════════════════════════
    // Utility functions (pure Move, built on the 8 natives above)
    // ═══════════════════════════════════════════════════════════

    /// Return true if the vector is empty.
    public fun is_empty<Element>(v: &vector<Element>): bool {
        length(v) == 0
    }

    /// Create a vector with a single element `e`.
    public fun singleton<Element>(e: Element): vector<Element> {
        let v = empty();
        push_back(&mut v, e);
        v
    }

    /// Return true if `v` contains `e`.
    public fun contains<Element>(v: &vector<Element>, e: &Element): bool {
        let (found, _) = index_of(v, e);
        found
    }

    /// Return `(true, index)` if `e` is in the vector, `(false, 0)` otherwise.
    public fun index_of<Element>(v: &vector<Element>, e: &Element): (bool, u64) {
        let len = length(v);
        let i = 0;
        while (i < len) {
            if (borrow(v, i) == e) {
                return (true, i)
            };
            i = i + 1;
        };
        (false, 0)
    }

    /// Remove the element at index `i`, shifting subsequent elements left.
    /// Returns the removed element.
    /// Aborts if `i` is out of bounds.
    public fun remove<Element>(v: &mut vector<Element>, i: u64): Element {
        let len = length(v);
        assert!(i < len, 0); // E_INDEX_OUT_OF_BOUNDS
        // Shift elements left
        let j = i;
        while (j < len - 1) {
            swap(v, j, j + 1);
            j = j + 1;
        };
        pop_back(v)
    }

    /// Append all elements of `other` to `lhs`, destroying `other`.
    public fun append<Element>(lhs: &mut vector<Element>, other: vector<Element>) {
        reverse(&mut other);
        let len = length(&other);
        while (len > 0) {
            push_back(lhs, pop_back(&mut other));
            len = len - 1;
        };
        destroy_empty(other);
    }

    /// Reverse the order of elements in the vector, in place.
    public fun reverse<Element>(v: &mut vector<Element>) {
        let len = length(v);
        if (len <= 1) return;
        let i = 0;
        let j = len - 1;
        while (i < j) {
            swap(v, i, j);
            i = i + 1;
            j = j - 1;
        };
    }
}
