// ===== setu-framework/sources/option.move =====
// Standard Option type — wraps a value that may or may not be present.
// Implementation: vector of 0 or 1 elements (same pattern as Sui move-stdlib).
// Pure Move, no native functions needed.
module std::option {
    use std::vector;

    // ── Error codes ──
    const EOPTION_IS_SET: u64 = 0x40000;
    const EOPTION_NOT_SET: u64 = 0x40001;

    /// Abstraction of a value that may or may not be present.
    /// Implemented as a vector of size zero or one.
    struct Option<Element> has copy, drop, store {
        vec: vector<Element>
    }

    // ── Constructors ──

    /// Return an empty `Option` (no value).
    public fun none<Element>(): Option<Element> {
        Option { vec: vector::empty() }
    }

    /// Return an `Option` containing `e`.
    public fun some<Element>(e: Element): Option<Element> {
        Option { vec: vector::singleton(e) }
    }

    // ── Queries ──

    /// Return true if `t` does not hold a value.
    public fun is_none<Element>(t: &Option<Element>): bool {
        vector::is_empty(&t.vec)
    }

    /// Return true if `t` holds a value.
    public fun is_some<Element>(t: &Option<Element>): bool {
        !vector::is_empty(&t.vec)
    }

    /// Return true if the value in `t` is equal to `e_ref`.
    /// Always returns false if `t` does not hold a value.
    public fun contains<Element>(t: &Option<Element>, e_ref: &Element): bool {
        vector::contains(&t.vec, e_ref)
    }

    // ── Borrowing ──

    /// Return an immutable reference to the value inside `t`.
    /// Aborts if `t` does not hold a value.
    public fun borrow<Element>(t: &Option<Element>): &Element {
        assert!(is_some(t), EOPTION_NOT_SET);
        vector::borrow(&t.vec, 0)
    }

    /// Return a mutable reference to the value inside `t`.
    /// Aborts if `t` does not hold a value.
    public fun borrow_mut<Element>(t: &mut Option<Element>): &mut Element {
        assert!(is_some(t), EOPTION_NOT_SET);
        vector::borrow_mut(&mut t.vec, 0)
    }

    /// Return a reference to the value inside `t`, or `default_ref` if `t` is empty.
    public fun borrow_with_default<Element>(t: &Option<Element>, default_ref: &Element): &Element {
        if (is_some(t)) {
            vector::borrow(&t.vec, 0)
        } else {
            default_ref
        }
    }

    // ── Mutations ──

    /// Convert a `none` to `some(e)` by adding `e`.
    /// Aborts if `t` already holds a value.
    public fun fill<Element>(t: &mut Option<Element>, e: Element) {
        assert!(is_none(t), EOPTION_IS_SET);
        vector::push_back(&mut t.vec, e)
    }

    /// Extract the value inside `t`, leaving `t` empty.
    /// Aborts if `t` does not hold a value.
    public fun extract<Element>(t: &mut Option<Element>): Element {
        assert!(is_some(t), EOPTION_NOT_SET);
        vector::pop_back(&mut t.vec)
    }

    /// Swap the old value inside `t` for `e`, returning the old value.
    /// Aborts if `t` does not hold a value.
    public fun swap<Element>(t: &mut Option<Element>, e: Element): Element {
        assert!(is_some(t), EOPTION_NOT_SET);
        let old = vector::pop_back(&mut t.vec);
        vector::push_back(&mut t.vec, e);
        old
    }

    /// If `t` holds a value, swap it for `e` and return the old value.
    /// Otherwise, fill `t` with `e` and return none.
    public fun swap_or_fill<Element>(t: &mut Option<Element>, e: Element): Option<Element> {
        if (is_some(t)) {
            some(swap(t, e))
        } else {
            fill(t, e);
            none()
        }
    }

    // ── Destructors ──

    /// Destroy `t` if it holds no value. Aborts if `t` holds a value.
    public fun destroy_none<Element>(t: Option<Element>) {
        let Option { vec } = t;
        assert!(vector::is_empty(&vec), EOPTION_IS_SET);
        vector::destroy_empty(vec)
    }

    /// Destroy `t` and extract its value. Aborts if `t` holds no value.
    public fun destroy_some<Element>(t: Option<Element>): Element {
        let Option { vec } = t;
        assert!(!vector::is_empty(&vec), EOPTION_NOT_SET);
        let elem = vector::pop_back(&mut vec);
        vector::destroy_empty(vec);
        elem
    }

    /// Destroy `t`. If it holds a value, return it. Otherwise return `default`.
    public fun destroy_with_default<Element: drop>(t: Option<Element>, default: Element): Element {
        let Option { vec } = t;
        if (vector::is_empty(&vec)) {
            vector::destroy_empty(vec);
            default
        } else {
            vector::pop_back(&mut vec)
        }
    }

    /// Return the value inside `t` if present, otherwise return `default`.
    public fun get_with_default<Element: copy + drop>(t: &Option<Element>, default: Element): Element {
        if (is_some(t)) {
            *vector::borrow(&t.vec, 0)
        } else {
            default
        }
    }
}
