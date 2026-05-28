//! oby-core: the trait + types that every oby crate depends on.
//! No I/O. Pure data and contracts.

pub mod hook;
pub use hook::{EffortLevel, HookContext, HookEvent};
