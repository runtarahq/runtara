// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Per-provider default model identifiers.
//!
//! Single source of truth for the model used when a workflow or capability
//! omits `model`. Every runtime fallback in this crate and in the agent
//! crates references these constants; the catalog-membership tests below
//! keep them from drifting away from the model registries served to the UI.

/// Default Amazon Bedrock model for AI Agent and agent-capability calls.
pub const DEFAULT_BEDROCK_MODEL: &str = "anthropic.claude-sonnet-4-6";

/// Default OpenAI model for text/chat completion calls.
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o";

/// Default OpenAI model for structured-output calls, where a smaller,
/// cheaper model is the deliberate choice.
pub const DEFAULT_OPENAI_MINI_MODEL: &str = "gpt-4o-mini";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{bedrock_models, openai_models};
    use serde_json::Value;

    fn assert_recommended(catalog: &Value, model_id: &str) {
        let entry = catalog["models"]
            .as_array()
            .expect("catalog has a models array")
            .iter()
            .find(|m| m["modelId"] == model_id)
            .unwrap_or_else(|| panic!("default model {model_id} is missing from the catalog"));
        assert_eq!(
            entry["recommendedForAiAgent"], true,
            "default model {model_id} is in the catalog but not recommended"
        );
    }

    #[test]
    fn bedrock_default_is_in_catalog_and_recommended() {
        assert_recommended(&bedrock_models::catalog_json(), DEFAULT_BEDROCK_MODEL);
    }

    #[test]
    fn openai_defaults_are_in_catalog_and_recommended() {
        let catalog = openai_models::catalog_json();
        assert_recommended(&catalog, DEFAULT_OPENAI_MODEL);
        assert_recommended(&catalog, DEFAULT_OPENAI_MINI_MODEL);
    }
}
