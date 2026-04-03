//! SQL utilities for Object Store
//!
//! Provides SQL generation, sanitization, and query building utilities.

pub mod condition;
pub mod ddl;
pub mod sanitize;

pub use condition::{build_condition_clause, build_order_by_clause};
pub use ddl::DdlGenerator;
pub use sanitize::{POSTGRES_RESERVED_WORDS, quote_identifier, validate_identifier};
