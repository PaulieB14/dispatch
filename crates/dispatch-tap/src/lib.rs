//! TAP v2 (GraphTally) — shared types, EIP-712 hashing, and receipt signing.
//!
//! Used by both `dispatch-service` (validates incoming receipts) and
//! `dispatch-gateway` (creates and signs outgoing receipts).

pub mod eip712;
pub mod rav;
pub mod sign;
pub mod types;

pub use eip712::{address_from_key, domain_separator, eip712_hash, recover_signer};
pub use rav::{collection_id, sign_rav, Rav, SignedRav};
pub use sign::create_receipt;
pub use types::{payer_from_metadata, Receipt, SignedReceipt};
