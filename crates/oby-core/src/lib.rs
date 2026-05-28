//! oby-core: the trait + types that every oby crate depends on.

pub mod entry;
pub mod hook;
pub use entry::{DiffHunk, DisplayEntry, DisplayEntryUpdate, EntryBody, EntryStatus};
pub use hook::{EffortLevel, HookContext, HookEvent};
