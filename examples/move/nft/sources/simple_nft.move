// ===== examples/move/nft/sources/simple_nft.move =====
// Simple NFT (Non-Fungible Token) — demonstrates unique owned objects.
//
// Demonstrates:
//   - Creating unique objects (NFTs)
//   - Immutable metadata (name, description, creator)
//   - Transferring ownership
//   - Freezing objects (making them immutable/read-only)
//   - Deleting objects
//
// Deploy:
//   POST /api/v1/move/publish { "sender": "alice", "modules": ["<hex bytecode>"] }
//
// Usage:
//   POST /api/v1/move/call {
//     "sender": "alice",
//     "package": "<package_addr>",
//     "module": "simple_nft",
//     "function": "mint",
//     "type_args": [],
//     "args": ["<bcs_encoded_name>", "<bcs_encoded_description>"]
//   }
module examples::simple_nft {
    use setu::object::{Self, UID};
    use setu::transfer;
    use setu::tx_context::{Self, TxContext};

    /// A simple NFT with name and description
    struct NFT has key, store {
        id: UID,
        /// NFT name (set at mint, immutable)
        name: vector<u8>,
        /// Description (set at mint, immutable)
        description: vector<u8>,
        /// Creator address (set at mint, immutable)
        creator: address,
    }

    /// Mint a new NFT and transfer to the sender
    public entry fun mint(
        name: vector<u8>,
        description: vector<u8>,
        ctx: &mut TxContext,
    ) {
        let nft = NFT {
            id: object::new(ctx),
            name,
            description,
            creator: tx_context::sender(ctx),
        };
        transfer::transfer(nft, tx_context::sender(ctx));
    }

    /// Transfer the NFT to a new owner
    public entry fun transfer_to(nft: NFT, recipient: address) {
        transfer::transfer(nft, recipient);
    }

    /// Freeze the NFT — makes it publicly readable and immutable forever
    public entry fun freeze_nft(nft: NFT) {
        transfer::freeze_object(nft);
    }

    /// Destroy the NFT (only the owner can do this)
    public entry fun burn(nft: NFT) {
        let NFT { id, name: _, description: _, creator: _ } = nft;
        object::delete(id);
    }

    // ── Read functions ──

    public fun name(nft: &NFT): &vector<u8> {
        &nft.name
    }

    public fun description(nft: &NFT): &vector<u8> {
        &nft.description
    }

    public fun creator(nft: &NFT): address {
        nft.creator
    }
}
