// Backward compatibility re-exports for metrics module
// This file maintains the old import path while everything has been moved to the new structure
//
// Old structure: src/api/metrics.rs (DTOs + handlers in one file)
// New structure:
//   - DTOs: src/api/dto/metrics.rs
//   - Handlers: src/api/handlers/metrics.rs
//
// To maintain backward compatibility during migration, we re-export everything here

// Re-export DTOs from the new location
pub use crate::api::dto::metrics::*;

// Re-export handlers from the new location
pub use crate::api::handlers::metrics::*;
