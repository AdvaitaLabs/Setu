//! Storage backend traits
//!
//! This module defines the abstract interfaces for storage backends.
//! Implementations are provided in `memory/` and `rocks/` modules.

pub mod anchor;
pub mod cf;
pub mod event;
pub mod object;

pub use anchor::AnchorStoreBackend;
pub use cf::CFStoreBackend;
pub use event::EventStoreBackend;
pub use object::ObjectStore;
