// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Static OpenAI model metadata for AI Agent configuration.
//!
//! Counterpart of [`super::bedrock_models`]: the registry the server exposes
//! for model pickers, and the catalog the per-provider defaults in
//! [`crate::defaults`] are validated against.

use serde_json::{Value, json};

pub fn catalog_json() -> Value {
    json!({
        "generatedAt": null,
        "source": "static OpenAI AI Agent defaults",
        "models": [
            {"provider": "OpenAI", "modelName": "GPT-4.1", "modelId": "gpt-4.1", "recommendedForAiAgent": true},
            {"provider": "OpenAI", "modelName": "GPT-4.1 Mini", "modelId": "gpt-4.1-mini", "recommendedForAiAgent": true},
            {"provider": "OpenAI", "modelName": "GPT-4o", "modelId": "gpt-4o", "recommendedForAiAgent": true},
            {"provider": "OpenAI", "modelName": "GPT-4o Mini", "modelId": "gpt-4o-mini", "recommendedForAiAgent": true}
        ]
    })
}
