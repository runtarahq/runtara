// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Static Google Gemini model metadata for AI Agent configuration.
//!
//! Counterpart of [`super::bedrock_models`] and [`super::openai_models`]. A
//! connection's live list comes from the provider through the `models`
//! connection resource; this is the fallback the defaults are checked against.

use serde_json::{Value, json};

pub fn catalog_json() -> Value {
    json!({
        "generatedAt": null,
        "source": "static Gemini AI Agent defaults",
        "models": [
            {"provider": "Google", "modelName": "Gemini 3.8 Flash", "modelId": "gemini-3.8-flash", "recommendedForAiAgent": true},
            {"provider": "Google", "modelName": "Gemini 3.5 Flash-Lite", "modelId": "gemini-3.5-flash-lite", "recommendedForAiAgent": true}
        ]
    })
}
