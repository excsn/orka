// orka/src/pipeline/mod.rs

//! Defines the `Pipeline<T>` struct, its construction, modification, and execution logic.

pub mod definition;
pub mod execution;
pub mod hooks;
// Optional: pub mod merge; // If merge features are implemented

// Re-export the main Pipeline struct
pub use definition::Pipeline;