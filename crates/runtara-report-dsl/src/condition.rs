//! Wire-shape `Condition` used by `ReportSource`.
//!
//! Object Model conditions on the request boundary look like
//! `{ "op": "EQ", "arguments": [...] }`. This is intentionally the same
//! shape as `runtara_server::api::dto::object_model::Condition` — that type
//! re-exports from here once the server takes a dependency on this crate,
//! so there is one definition.

use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<serde_json::Value>>,
}
