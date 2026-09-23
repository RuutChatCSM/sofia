//! Read-path helpers for Sofia memories.
//!
//! This crate owns memory injection, memory citation parsing, and telemetry
//! classification for read access to the memory folder. It intentionally does
//! not depend on the memory write pipeline.

pub mod citations;
mod metrics;
pub mod usage;

use sofia_utils_absolute_path::AbsolutePathBuf;

pub fn memory_root(sofia_home: &AbsolutePathBuf) -> AbsolutePathBuf {
    sofia_home.join("memories")
}
