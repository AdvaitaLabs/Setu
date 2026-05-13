// ===== stdlib_test.move =====
// Integration test contract: exercises all Phase 5a stdlib additions.
// Each test_* entry function asserts correctness internally.
// If any assertion fails, the function aborts. Success = created TestResult object.
module examples::stdlib_test {
    use std::option::{Self, Option};
    use std::vector;
    use std::string::Self;
    use setu::vec_map::{Self, VecMap};
    use setu::vec_set::{Self, VecSet};
    use setu::object::{Self, UID};
    use setu::transfer;
    use setu::tx_context::{Self, TxContext};

    /// Result object — created only if all assertions pass.
    struct TestResult has key, store {
        id: UID,
        test_name: vector<u8>,
        passed: u64,
    }

    fun emit_result(name: vector<u8>, passed: u64, ctx: &mut TxContext) {
        transfer::transfer(
            TestResult { id: object::new(ctx), test_name: name, passed },
            tx_context::sender(ctx),
        );
    }

    // ═══════════════════════════════════════════════════════════
    // Test: std::option
    // ═══════════════════════════════════════════════════════════
    public entry fun test_option(ctx: &mut TxContext) {
        let count = 0u64;

        // none / is_none
        let opt: Option<u64> = option::none();
        assert!(option::is_none(&opt), 100);
        assert!(!option::is_some(&opt), 101);
        count = count + 1;

        // some / is_some
        let opt2 = option::some(42u64);
        assert!(option::is_some(&opt2), 102);
        assert!(!option::is_none(&opt2), 103);
        count = count + 1;

        // borrow
        assert!(*option::borrow(&opt2) == 42, 104);
        count = count + 1;

        // contains
        assert!(option::contains(&opt2, &42), 105);
        assert!(!option::contains(&opt2, &99), 106);
        count = count + 1;

        // extract
        let val = option::extract(&mut opt2);
        assert!(val == 42, 107);
        assert!(option::is_none(&opt2), 108);
        count = count + 1;

        // fill
        option::fill(&mut opt2, 100);
        assert!(*option::borrow(&opt2) == 100, 109);
        count = count + 1;

        // swap
        let old = option::swap(&mut opt2, 200);
        assert!(old == 100, 110);
        assert!(*option::borrow(&opt2) == 200, 111);
        count = count + 1;

        // destroy_some
        let v = option::destroy_some(opt2);
        assert!(v == 200, 112);
        count = count + 1;

        // destroy_none
        option::destroy_none(opt);
        count = count + 1;

        // get_with_default
        let empty_opt: Option<u64> = option::none();
        assert!(option::get_with_default(&empty_opt, 55) == 55, 113);
        let full_opt = option::some(77u64);
        assert!(option::get_with_default(&full_opt, 55) == 77, 114);
        count = count + 1;

        // destroy_with_default
        let v2 = option::destroy_with_default(empty_opt, 88);
        assert!(v2 == 88, 115);
        count = count + 1;

        // swap_or_fill on empty
        let mut_opt: Option<u64> = option::none();
        let result = option::swap_or_fill(&mut mut_opt, 33);
        assert!(option::is_none(&result), 116);
        assert!(*option::borrow(&mut_opt) == 33, 117);
        option::destroy_none(result);
        count = count + 1;

        // swap_or_fill on filled
        let result2 = option::swap_or_fill(&mut mut_opt, 44);
        assert!(option::is_some(&result2), 118);
        assert!(option::destroy_some(result2) == 33, 119);
        assert!(*option::borrow(&mut_opt) == 44, 120);
        count = count + 1;

        // cleanup
        option::destroy_some(mut_opt);
        option::destroy_some(full_opt);

        emit_result(b"test_option", count, ctx);
    }

    // ═══════════════════════════════════════════════════════════
    // Test: std::vector utilities
    // ═══════════════════════════════════════════════════════════
    public entry fun test_vector(ctx: &mut TxContext) {
        let count = 0u64;

        // is_empty
        let v = vector::empty<u64>();
        assert!(vector::is_empty(&v), 200);
        count = count + 1;

        // singleton
        let v2 = vector::singleton(42u64);
        assert!(vector::length(&v2) == 1, 201);
        assert!(*vector::borrow(&v2, 0) == 42, 202);
        count = count + 1;

        // push_back + contains
        vector::push_back(&mut v, 10);
        vector::push_back(&mut v, 20);
        vector::push_back(&mut v, 30);
        assert!(vector::contains(&v, &20), 203);
        assert!(!vector::contains(&v, &99), 204);
        count = count + 1;

        // index_of
        let (found, idx) = vector::index_of(&v, &20);
        assert!(found, 205);
        assert!(idx == 1, 206);
        let (not_found, _) = vector::index_of(&v, &99);
        assert!(!not_found, 207);
        count = count + 1;

        // remove (from middle)
        let removed = vector::remove(&mut v, 1); // remove 20
        assert!(removed == 20, 208);
        assert!(vector::length(&v) == 2, 209);
        assert!(*vector::borrow(&v, 0) == 10, 210);
        assert!(*vector::borrow(&v, 1) == 30, 211);
        count = count + 1;

        // reverse
        vector::reverse(&mut v);
        assert!(*vector::borrow(&v, 0) == 30, 212);
        assert!(*vector::borrow(&v, 1) == 10, 213);
        count = count + 1;

        // append
        let other = vector::empty<u64>();
        vector::push_back(&mut other, 50);
        vector::push_back(&mut other, 60);
        vector::append(&mut v, other);
        assert!(vector::length(&v) == 4, 214);
        assert!(*vector::borrow(&v, 2) == 50, 215);
        assert!(*vector::borrow(&v, 3) == 60, 216);
        count = count + 1;

        // cleanup
        let _ = vector::pop_back(&mut v);
        let _ = vector::pop_back(&mut v);
        let _ = vector::pop_back(&mut v);
        let _ = vector::pop_back(&mut v);
        vector::destroy_empty(v);

        let _ = vector::pop_back(&mut v2);
        vector::destroy_empty(v2);

        emit_result(b"test_vector", count, ctx);
    }

    // ═══════════════════════════════════════════════════════════
    // Test: std::string
    // ═══════════════════════════════════════════════════════════
    public entry fun test_string(ctx: &mut TxContext) {
        let count = 0u64;

        // utf8 / length / is_empty
        let s = string::utf8(b"hello");
        assert!(string::length(&s) == 5, 300);
        assert!(!string::is_empty(&s), 301);
        count = count + 1;

        let empty = string::utf8(b"");
        assert!(string::is_empty(&empty), 302);
        count = count + 1;

        // as_bytes
        let bytes = string::as_bytes(&s);
        assert!(vector::length(bytes) == 5, 303);
        count = count + 1;

        // append
        let s2 = string::utf8(b"hello");
        string::append(&mut s2, string::utf8(b" world"));
        assert!(string::length(&s2) == 11, 304);
        count = count + 1;

        // append_utf8
        string::append_utf8(&mut s2, b"!");
        assert!(string::length(&s2) == 12, 305);
        count = count + 1;

        // sub_string
        let sub = string::sub_string(&s2, 0, 5);
        assert!(string::length(&sub) == 5, 306);
        // verify "hello" bytes
        let sub_bytes = string::as_bytes(&sub);
        assert!(*vector::borrow(sub_bytes, 0) == 104, 307); // 'h'
        assert!(*vector::borrow(sub_bytes, 4) == 111, 308); // 'o'
        count = count + 1;

        // into_bytes
        let raw = string::into_bytes(s);
        assert!(vector::length(&raw) == 5, 309);
        count = count + 1;

        // index_of
        let haystack = string::utf8(b"hello world");
        let needle = string::utf8(b"world");
        let result = string::index_of(&haystack, &needle);
        assert!(option::is_some(&result), 310);
        assert!(*option::borrow(&result) == 6, 311);
        count = count + 1;

        let not_found = string::index_of(&haystack, &string::utf8(b"xyz"));
        assert!(option::is_none(&not_found), 312);
        count = count + 1;

        emit_result(b"test_string", count, ctx);
    }

    // ═══════════════════════════════════════════════════════════
    // Test: setu::vec_map
    // ═══════════════════════════════════════════════════════════
    public entry fun test_vec_map(ctx: &mut TxContext) {
        let count = 0u64;

        // empty / is_empty
        let map: VecMap<u64, u64> = vec_map::empty();
        assert!(vec_map::is_empty(&map), 400);
        assert!(vec_map::size(&map) == 0, 401);
        count = count + 1;

        // insert + size
        vec_map::insert(&mut map, 1, 100);
        vec_map::insert(&mut map, 2, 200);
        vec_map::insert(&mut map, 3, 300);
        assert!(vec_map::size(&map) == 3, 402);
        assert!(!vec_map::is_empty(&map), 403);
        count = count + 1;

        // contains
        assert!(vec_map::contains(&map, &2), 404);
        assert!(!vec_map::contains(&map, &99), 405);
        count = count + 1;

        // get
        assert!(*vec_map::get(&map, &1) == 100, 406);
        assert!(*vec_map::get(&map, &3) == 300, 407);
        count = count + 1;

        // get_mut
        let val_ref = vec_map::get_mut(&mut map, &2);
        *val_ref = 222;
        assert!(*vec_map::get(&map, &2) == 222, 408);
        count = count + 1;

        // try_get
        let found = vec_map::try_get(&map, &1);
        assert!(option::is_some(&found), 409);
        assert!(option::destroy_some(found) == 100, 410);
        let missing = vec_map::try_get(&map, &99);
        assert!(option::is_none(&missing), 411);
        option::destroy_none(missing);
        count = count + 1;

        // remove
        let (k, v) = vec_map::remove(&mut map, &2);
        assert!(k == 2, 412);
        assert!(v == 222, 413);
        assert!(vec_map::size(&map) == 2, 414);
        count = count + 1;

        // keys
        let ks = vec_map::keys(&map);
        assert!(vector::length(&ks) == 2, 415);
        count = count + 1;

        // get_entry_by_idx
        let (key_ref, val_ref) = vec_map::get_entry_by_idx(&map, 0);
        assert!(*key_ref == 1, 416);
        assert!(*val_ref == 100, 417);
        count = count + 1;

        // into_keys_values
        let (keys, values) = vec_map::into_keys_values(map);
        assert!(vector::length(&keys) == 2, 418);
        assert!(vector::length(&values) == 2, 419);
        count = count + 1;

        emit_result(b"test_vec_map", count, ctx);
    }

    // ═══════════════════════════════════════════════════════════
    // Test: setu::vec_set
    // ═══════════════════════════════════════════════════════════
    public entry fun test_vec_set(ctx: &mut TxContext) {
        let count = 0u64;

        // empty / is_empty
        let set: VecSet<u64> = vec_set::empty();
        assert!(vec_set::is_empty(&set), 500);
        assert!(vec_set::size(&set) == 0, 501);
        count = count + 1;

        // singleton
        let set2 = vec_set::singleton(42u64);
        assert!(vec_set::size(&set2) == 1, 502);
        assert!(vec_set::contains(&set2, &42), 503);
        count = count + 1;

        // insert + contains
        vec_set::insert(&mut set, 10);
        vec_set::insert(&mut set, 20);
        vec_set::insert(&mut set, 30);
        assert!(vec_set::size(&set) == 3, 504);
        assert!(vec_set::contains(&set, &20), 505);
        assert!(!vec_set::contains(&set, &99), 506);
        count = count + 1;

        // remove
        vec_set::remove(&mut set, &20);
        assert!(vec_set::size(&set) == 2, 507);
        assert!(!vec_set::contains(&set, &20), 508);
        count = count + 1;

        // into_keys
        let keys = vec_set::into_keys(set);
        assert!(vector::length(&keys) == 2, 509);
        count = count + 1;

        // cleanup set2
        let _ = vec_set::into_keys(set2);

        emit_result(b"test_vec_set", count, ctx);
    }

    // ═══════════════════════════════════════════════════════════
    // Run all tests
    // ═══════════════════════════════════════════════════════════
    public entry fun test_all(ctx: &mut TxContext) {
        test_option(ctx);
        test_vector(ctx);
        test_string(ctx);
        test_vec_map(ctx);
        test_vec_set(ctx);
    }
}
