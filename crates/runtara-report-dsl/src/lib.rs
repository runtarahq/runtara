//! `runtara-report-dsl` — schema, validators, template engine, and row-condition
//! evaluator for the runtara reports DSL.
//!
//! Compiles native (server) and `wasm32` (frontend). Phase 1 of the reports
//! refactor extracts this crate from `runtara-server`; later phases switch
//! the server to use only this crate's evaluators, and Phase 2 ships the
//! WASM build to the frontend so FE and BE share one validation truth.

pub mod condition;
pub mod format;
pub mod row_condition;
pub mod template;
pub mod types;

#[cfg(feature = "wasm")]
mod wasm_bindings;

pub use condition::Condition;
pub use format::{FormatSpec, Formatter, RenderContext, SimpleAsciiFormatter};
pub use row_condition::{RowConditionError, evaluate_row_condition};
pub use template::{
    TemplateError, format_value, make_environment, register_report_filters, render_template,
    render_template_with_extras, validate_template,
};
pub use types::*;

/// Library version, set from the workspace `Cargo.toml`. Useful for FE↔BE
/// drift detection: the WASM bundle and the server should report identical
/// versions.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
