//! Report DTOs — re-exported from `runtara-report-dsl`.
//!
//! Phase 1 of the reports refactor moved the report-specific types into the
//! `runtara-report-dsl` crate so the FE WASM bundle (Phase 2) can use them
//! without depending on the server. This module exists for backwards
//! compatibility with the existing `use crate::api::dto::reports::*` import
//! sites; the canonical definitions live in `runtara_report_dsl::types`.

pub use runtara_report_dsl::*;
