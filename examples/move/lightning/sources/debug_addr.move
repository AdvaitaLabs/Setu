/// Minimal debug module to verify TxContext sender matches pure address arg.
module lightning::debug_addr {
    use setu::object::{Self, UID};
    use setu::transfer;
    use setu::tx_context::{Self, TxContext};

    struct AddrCheck has key, store {
        id: UID,
        sender_addr: address,
        arg_addr: address,
        are_equal: bool,
    }

    /// Create an AddrCheck object comparing sender vs pure arg address.
    public entry fun check_addr(addr_arg: address, ctx: &mut TxContext) {
        let s = tx_context::sender(ctx);
        let obj = AddrCheck {
            id: object::new(ctx),
            sender_addr: s,
            arg_addr: addr_arg,
            are_equal: (s == addr_arg),
        };
        transfer::transfer(obj, s);
    }
}
