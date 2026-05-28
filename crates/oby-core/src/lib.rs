//! oby-core: the trait + types that every oby crate depends on.

pub mod capturer;
pub mod entry;
pub mod hook;
pub mod wire;

pub use capturer::{Capturer, RewriteDecision};
pub use entry::{DiffHunk, DisplayEntry, DisplayEntryUpdate, EntryBody, EntryStatus};
pub use hook::{EffortLevel, HookContext, HookEvent};
pub use wire::{ControlMessage, HeaderLine};
