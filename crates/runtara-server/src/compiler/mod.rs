//! Workflow compilation module
//!
//! This module provides DB-dependent operations for workflow compilation
//! (loading child workflows from the database). The actual compilation logic
//! is in the runtara-workflows crate, driven by the compilation service.

pub mod child_workflows;

// Re-export for convenience
pub use runtara_workflows::ChildDependency;
