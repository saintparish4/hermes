pub mod authority;
pub mod classify;
pub mod slots;
pub mod store;

pub use authority::{AuthorityKind, AuthorityProbe, Confidence, MAX_DEPTH, Resolution, resolve};
pub use classify::{Classified, ProxyKind, SlotReads, classify};
pub use slots::{ADMIN_SLOT, BEACON_SLOT, IMPL_SLOT, PROXIABLE_SLOT, slot_key, word_to_address};
pub use store::{ProxyRecord, Store};
